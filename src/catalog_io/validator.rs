use jsonschema::{options, Registry, Resource, Validator};
use serde_json::Value;
use super::error::ConfigError;

pub struct SchemaValidator {
    compiled: Validator,
}

/// A retriever that fetches nothing.
///
/// Catalogs are user-editable YAML, so a `$ref` in one is untrusted input. Left with
/// `jsonschema`'s default retriever, `$ref: "https://attacker.example/x"` in a catalog
/// would make k2g issue an arbitrary HTTPS request while validating it. Every schema
/// this application needs is registered as a resource below; anything else is a
/// validation error naming the URI. EU CRA Annex I (2)(j).
struct NoRemoteRefs;

impl jsonschema::Retrieve for NoRemoteRefs {
    fn retrieve(
        &self,
        uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err(format!(
            "refusing to fetch the external schema reference '{uri}'. k2g validates \
             against its own bundled schemas only and never retrieves one over the \
             network."
        )
        .into())
    }
}

impl SchemaValidator {
    /// Compile a JSON Schema for reuse.
    ///
    /// `refs` is every schema a `$ref` in `schema` may resolve against, as
    /// `(file name, YAML text)` — in practice the whole embedded set,
    /// [`crate::data::embedded_schemas`]. Each is registered twice, under the bare file
    /// name and under the `json-schema:///` URI form, because `catalog.yaml` writes its
    /// references the first way and the resolver normalizes them to the second.
    ///
    /// **From memory, not from a directory.** `catalog.yaml` references `units.yaml`
    /// and `id.yaml`, and a reference that cannot be resolved fails the compile — so
    /// where those two are found decides whether catalogs load at all. This used to read
    /// them from `schemas/` *relative to the working directory*: the repository root
    /// under `cargo run` and `cargo test`, and a directory with no `schemas/` in it for
    /// every shipped build, where the compile failed, took `CatalogManager::new` down
    /// with it, and left the user's own catalogs silently unread behind the bundled ones
    /// (see [`crate::runtime::catalogs`]). The schemas the user *can* now read are
    /// published separately and never loaded back — see
    /// [`crate::data::schema_export`].
    pub fn new(schema: &Value, refs: &[(&str, &str)]) -> Result<Self, ConfigError> {
        // Since jsonschema 0.50 the resources are collected into a `Registry` up front
        // rather than pushed one at a time onto the options: `with_resource` is gone, and
        // the builders take `self` by value instead of `&mut self`.
        let mut resources = Vec::with_capacity(refs.len() * 2);
        for (file_name, text) in refs {
            let yaml_value: serde_yaml::Value = serde_yaml::from_str(text)
                .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
            let json_value: Value = serde_json::to_value(yaml_value)
                .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
            // `Resource::from_contents` is infallible now — the draft is detected from
            // the contents and falls back to the default rather than erroring.
            let resource = Resource::from_contents(json_value);

            resources.push(((*file_name).to_string(), resource.clone()));
            resources.push((format!("json-schema:///{file_name}"), resource));
        }

        // `NoRemoteRefs` is installed on both halves, because they retrieve at different
        // moments and neither covers the other: the registry resolves the `$ref`s between
        // the embedded schemas, while the options' retriever is what a `$ref` in `schema`
        // — the catalog-supplied, untrusted half — reaches for when it names something the
        // registry does not hold. The test below pins that second one.
        let registry = Registry::new()
            .retriever(NoRemoteRefs)
            .extend(resources)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?
            .prepare()
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;

        let compiled = options()
            .with_registry(&registry)
            .with_retriever(NoRemoteRefs)
            .build(schema)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
        Ok(Self { compiled })
    }

    /// Validate a document. Returns all errors joined, or Ok(())
    pub fn validate(&self, document: &Value) -> Result<(), ConfigError> {
        if self.compiled.validate(document).is_err() {
            let messages: Vec<String> = self
                .compiled
                .iter_errors(document)
                .map(|e| e.to_string())
                .collect();
            return Err(ConfigError::Validation(messages.join("\n")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog schema, as the loader compiles it.
    fn catalog_schema() -> Value {
        let text = crate::data::embedded_schemas()
            .iter()
            .find(|(name, _)| *name == "catalog.yaml")
            .expect("catalog.yaml is embedded")
            .1;
        serde_json::to_value(serde_yaml::from_str::<serde_yaml::Value>(text).unwrap()).unwrap()
    }

    /// One tool, exercising a reference into each of the two shared files: `id.yaml`
    /// for the UUIDv7 and `units.yaml` for every dimension and rate.
    fn one_tool_catalog() -> Value {
        serde_json::json!({
            "schema_version": 1,
            "name": "Bench drawer",
            "sections": [{
                "name": "Drills",
                "tools": [{
                    "id": "018f3a2b-7c41-7d3e-8b0a-1f2e3d4c5b6a",
                    "type": "drillbit",
                    "diameter": "0.8 mm",
                    "point_angle": "118 deg",
                    "z_min_depth": "0 mm",
                    "spindle_rpm": "24000 rpm",
                    "z_feed": "120 mm/min"
                }]
            }]
        })
    }

    /// Compilation reaches across files with nothing on disk to help it — no `schemas/`
    /// directory, no working directory, no network. That is what a shipped build has,
    /// and a catalog schema it cannot compile is a catalog it cannot read: the failure
    /// takes `CatalogManager::new` with it and every user-written catalog goes unread.
    #[test]
    fn cross_file_references_resolve_from_the_embedded_set() {
        let validator = SchemaValidator::new(&catalog_schema(), crate::data::embedded_schemas())
            .expect("the embedded set resolves catalog.yaml's references");

        validator
            .validate(&one_tool_catalog())
            .expect("a well-formed catalog validates");
    }

    /// And the references genuinely bind rather than being waved through: a diameter
    /// with no unit is what `units.yaml#/$defs/size` exists to reject.
    #[test]
    fn a_resolved_reference_still_rejects_what_it_should() {
        let validator =
            SchemaValidator::new(&catalog_schema(), crate::data::embedded_schemas()).unwrap();

        let mut doc = one_tool_catalog();
        doc["sections"][0]["tools"][0]["diameter"] = Value::String("0.8".into());
        assert!(validator.validate(&doc).is_err(), "a unitless size is not a size");
    }

    /// The other half of the same fact, and the regression guard for the bug this
    /// signature exists to close: when the references are *not* to hand, compiling the
    /// catalog schema fails outright. It is not a degraded validation that lets a few
    /// fields through unchecked — it is no validator at all, and every user-written
    /// catalog goes unread behind the bundled ones. Which is exactly what a shipped
    /// build got while these were looked up in `./schemas`.
    #[test]
    fn without_the_references_the_catalog_schema_does_not_compile_at_all() {
        assert!(
            SchemaValidator::new(&catalog_schema(), &[]).is_err(),
            "catalog.yaml cannot be compiled without id.yaml and units.yaml"
        );
    }

    /// The refusal to fetch, which the embedded set does not make redundant: a `$ref` in
    /// a *catalog* is still untrusted input, and it must fail naming the URI rather than
    /// send k2g to fetch it. EU CRA Annex I (2)(j).
    #[test]
    fn an_off_machine_reference_is_refused_rather_than_fetched() {
        let schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": "https://attacker.example/schema.json"
        });

        // Not `expect_err`: the success arm holds a compiled validator, which is not
        // `Debug` and has nothing worth printing anyway.
        let Err(error) = SchemaValidator::new(&schema, crate::data::embedded_schemas()) else {
            panic!("an external reference must not be retrieved");
        };
        let error = error.to_string();
        assert!(error.contains("attacker.example"), "got: {error}");
    }
}
