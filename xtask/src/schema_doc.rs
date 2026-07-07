use anyhow::{anyhow, Context, Result};
use regex::Regex;
use serde_json::Value as JsonValue;
use serde_yaml::Value as YamlValue;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path};
use walkdir::WalkDir;

pub fn generate_schema_docs(
    schema_dir: &std::path::Path,
    output_md: &std::path::Path,
) -> anyhow::Result<()> {
    let schemas = load_schemas(schema_dir)?;
    let markdown = render_markdown(&schemas)?;

    if let Some(parent) = output_md.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| {
                    format!("failed to create output directory {}", parent.display())
                })?;
        }
    }

    std::fs::write(output_md, markdown)
        .with_context(|| format!("failed to write {}", output_md.display()))?;

    Ok(())
}

#[derive(Debug)]
struct SchemaDoc {
    file_name: String,
    root: JsonValue,
    comments: HashMap<String, String>,
}

fn load_schemas(schema_dir: &Path) -> Result<Vec<SchemaDoc>> {
    if !schema_dir.is_dir() {
        return Err(anyhow!("{} is not a directory", schema_dir.display()));
    }

    let mut files = Vec::new();

    for entry in WalkDir::new(schema_dir).max_depth(1) {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };

        if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") {
            files.push(path.to_path_buf());
        }
    }

    files.sort();

    let mut schemas = Vec::new();

    for path in files {
        let text = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;

        let yaml: YamlValue = serde_yaml::from_str(&text)
            .with_context(|| format!("failed to parse YAML {}", path.display()))?;

        let root: JsonValue = serde_json::to_value(yaml)
            .with_context(|| format!("failed to convert YAML to JSON value {}", path.display()))?;

        let comments = collect_comments(&text)?;

        schemas.push(SchemaDoc {
            file_name: path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("<unknown>")
                .to_string(),
            root,
            comments,
        });
    }

    Ok(schemas)
}

/// Collect comments immediately preceding YAML keys.
///
/// The resulting map is keyed by a best-effort dotted path, for example:
///
/// - `properties.id`
/// - `$defs.tool.properties.diameter`
/// - `properties.tools.items.properties.id`
fn collect_comments(text: &str) -> Result<HashMap<String, String>> {
    let key_re = Regex::new(r#"^(?P<indent>\s*)(?P<key>[A-Za-z0-9_\-$]+):(?:\s|$)"#)?;
    let comment_re = Regex::new(r#"^\s*#(?P<body>.*)$"#)?;

    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut pending_comments: Vec<String> = Vec::new();
    let mut out = HashMap::new();

    for raw_line in text.lines() {
        let line = raw_line.trim_end();

        if line.trim().is_empty() {
            // Preserve paragraph breaks inside a pending comment block, but
            // avoid accumulating arbitrary empty lines before the next key.
            if !pending_comments.is_empty() {
                pending_comments.push(String::new());
            }
            continue;
        }

        if let Some(caps) = comment_re.captures(line) {
            let body = caps
                .name("body")
                .map(|m| m.as_str().trim().to_string())
                .unwrap_or_default();

            // Drop purely decorative separator comments.
            if body.chars().all(|c| c == '-' || c == '=') {
                continue;
            }

            pending_comments.push(body);
            continue;
        }

        if let Some(caps) = key_re.captures(line) {
            let indent = caps.name("indent").map(|m| m.as_str().len()).unwrap_or(0);
            let key = caps.name("key").unwrap().as_str().to_string();

            while let Some((last_indent, _)) = stack.last() {
                if *last_indent >= indent {
                    stack.pop();
                } else {
                    break;
                }
            }

            stack.push((indent, key.clone()));
            let path = stack
                .iter()
                .map(|(_, k)| k.as_str())
                .collect::<Vec<_>>()
                .join(".");

            let comment = normalize_comment_block(&pending_comments);
            if !comment.is_empty() {
                out.insert(path, comment);
            }
            pending_comments.clear();
            continue;
        }

        // Non-comment, non-key content breaks pending comment association.
        pending_comments.clear();
    }

    Ok(out)
}

fn normalize_comment_block(lines: &[String]) -> String {
    let mut cleaned = Vec::new();
    let mut last_blank = true;

    for line in lines {
        let trimmed = line.trim();

        // Skip common section-title repeats that are followed by richer text.
        if trimmed.chars().all(|c| c == '-' || c == '=') {
            continue;
        }

        if trimmed.is_empty() {
            if !last_blank {
                cleaned.push(String::new());
            }
            last_blank = true;
        } else {
            cleaned.push(trimmed.to_string());
            last_blank = false;
        }
    }

    while cleaned.last().map(|s| s.is_empty()).unwrap_or(false) {
        cleaned.pop();
    }

    cleaned.join("\n")
}

fn render_markdown(schemas: &[SchemaDoc]) -> Result<String> {
    let mut out = String::new();

    out.push_str("# Schema Reference\n\n");
    out.push_str("Generated from YAML JSON Schema files. Do not edit this file by hand.\n\n");

    out.push_str("## Schemas\n\n");
    for schema in schemas {
        let title = str_field(&schema.root, "title").unwrap_or(&schema.file_name);
        out.push_str(&format!("- [{}](#{}) (`{}`)\n", title, anchor(title), schema.file_name));
    }
    out.push('\n');

    for schema in schemas {
        render_schema(schema, &mut out)?;
    }

    Ok(out)
}

fn render_schema(schema: &SchemaDoc, out: &mut String) -> Result<()> {
    let title = str_field(&schema.root, "title").unwrap_or(&schema.file_name);
    let id = str_field(&schema.root, "$id").unwrap_or("");

    out.push_str(&format!("## {}\n\n", title));
    out.push_str(&format!("**File:** `{}`\n\n", schema.file_name));

    if !id.is_empty() {
        out.push_str(&format!("**Schema ID:** `{}`\n\n", id));
    }

    if let Some(desc) = str_field(&schema.root, "description") {
        out.push_str(desc.trim());
        out.push_str("\n\n");
    }

    let required = required_set(&schema.root);

    if let Some(props) = object_field(&schema.root, "properties") {
        out.push_str("### Top-level Properties\n\n");
        render_properties(props, "properties", &schema.comments, &required, 0, out)?;
    }

    if let Some(defs) = object_field(&schema.root, "$defs") {
        out.push_str("### Definitions\n\n");
        for (name, def) in defs {
            out.push_str(&format!("#### `{}`\n\n", name));
            render_field_doc(def, &format!("$defs.{}", name), &schema.comments, false, 0, out)?;

            if let Some(props) = object_field(def, "properties") {
                let req = required_set(def);
                render_properties(
                    props,
                    &format!("$defs.{}.properties", name),
                    &schema.comments,
                    &req,
                    1,
                    out,
                )?;
            }
        }
    }

    out.push_str("---\n\n");
    Ok(())
}

fn render_properties(
    props: &serde_json::Map<String, JsonValue>,
    path_prefix: &str,
    comments: &HashMap<String, String>,
    required: &BTreeSet<String>,
    depth: usize,
    out: &mut String,
) -> Result<()> {
    for (name, value) in props {
        let is_required = required.contains(name);
        let path = format!("{}.{}", path_prefix, name);
        out.push_str(&format!("{}- `{}`", indent(depth), name));
        if is_required {
            out.push_str(" **required**");
        }
        out.push('\n');

        render_field_doc(value, &path, comments, is_required, depth + 1, out)?;

        if let Some(child_props) = object_field(value, "properties") {
            let req = required_set(value);
            render_properties(child_props, &format!("{}.properties", path), comments, &req, depth + 1, out)?;
        }

        if let Some(items) = value.get("items") {
            if let Some(item_props) = object_field(items, "properties") {
                let req = required_set(items);
                render_properties(
                    item_props,
                    &format!("{}.items.properties", path),
                    comments,
                    &req,
                    depth + 1,
                    out,
                )?;
            }
        }
    }

    Ok(())
}

fn render_field_doc(
    value: &JsonValue,
    path: &str,
    comments: &HashMap<String, String>,
    _is_required: bool,
    depth: usize,
    out: &mut String,
) -> Result<()> {
    let mut parts = Vec::new();

    if let Some(t) = type_summary(value) {
        parts.push(format!("type: `{}`", t));
    }

    if let Some(r) = str_field(value, "$ref") {
        parts.push(format!("ref: `{}`", r));
    }

    if let Some(c) = value.get("const") {
        parts.push(format!("const: `{}`", literal(c)));
    }

    if let Some(default) = value.get("default") {
        parts.push(format!("default: `{}`", literal(default)));
    }

    if let Some(enum_values) = value.get("enum").and_then(|v| v.as_array()) {
        let vals = enum_values.iter().map(literal).collect::<Vec<_>>().join(", ");
        parts.push(format!("enum: {}", vals));
    }

    if let Some(pattern) = str_field(value, "pattern") {
        parts.push(format!("pattern: `{}`", pattern));
    }

    if !parts.is_empty() {
        out.push_str(&format!("{}  - {}\n", indent(depth), parts.join("; ")));
    }

    if let Some(desc) = str_field(value, "description") {
        write_wrapped_block(out, depth, desc.trim());
    }

    if let Some(comment) = comments.get(path) {
        write_wrapped_block(out, depth, comment.trim());
    }

    if let Some(any_of) = value.get("anyOf").and_then(|v| v.as_array()) {
        out.push_str(&format!("{}  - anyOf:\n", indent(depth)));
        for item in any_of {
            let summary = type_summary(item)
                .or_else(|| str_field(item, "$ref").map(|r| format!("ref {}", r)))
                .unwrap_or_else(|| literal(item));
            out.push_str(&format!("{}    - `{}`\n", indent(depth), summary));
        }
    }

    if let Some(all_of) = value.get("allOf").and_then(|v| v.as_array()) {
        out.push_str(&format!("{}  - allOf constraints: {} rule(s)\n", indent(depth), all_of.len()));
    }

    Ok(())
}

fn write_wrapped_block(out: &mut String, depth: usize, text: &str) {
    if text.is_empty() {
        return;
    }

    let prefix = format!("{}  ", indent(depth));
    for para in text.split("\n\n") {
        for line in para.lines() {
            if !line.trim().is_empty() {
                out.push_str(&prefix);
                out.push_str(line.trim());
                out.push('\n');
            }
        }
        out.push('\n');
    }
}

fn type_summary(value: &JsonValue) -> Option<String> {
    if let Some(t) = value.get("type") {
        match t {
            JsonValue::String(s) => Some(s.clone()),
            JsonValue::Array(arr) => Some(
                arr.iter()
                    .map(literal)
                    .collect::<Vec<_>>()
                    .join(" | "),
            ),
            _ => None,
        }
    } else if value.get("anyOf").is_some() {
        Some("anyOf".to_string())
    } else if value.get("allOf").is_some() {
        Some("allOf".to_string())
    } else {
        None
    }
}

fn literal(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => "null".to_string(),
        JsonValue::Array(a) => format!(
            "[{}]",
            a.iter().map(literal).collect::<Vec<_>>().join(", ")
        ),
        JsonValue::Object(_) => "object".to_string(),
    }
}

fn object_field<'a>(
    value: &'a JsonValue,
    name: &str,
) -> Option<&'a serde_json::Map<String, JsonValue>> {
    value.get(name)?.as_object()
}

fn str_field<'a>(value: &'a JsonValue, name: &str) -> Option<&'a str> {
    value.get(name)?.as_str()
}

fn required_set(value: &JsonValue) -> BTreeSet<String> {
    value
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default()
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn anchor(title: &str) -> String {
    title
        .chars()
        .filter_map(|c| {
            if c.is_ascii_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c.is_whitespace() || c == '-' || c == '_' {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
