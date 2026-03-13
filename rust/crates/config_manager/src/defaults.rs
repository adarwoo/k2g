use serde_json::{Map, Value};

/// Recursively populate default values from a JSON Schema node.
/// Mirrors Python's YamlConfigManager._populate_defaults()
pub fn populate_defaults(schema: &Value) -> Option<Value> {
    // Direct default
    if let Some(default) = schema.get("default") {
        return Some(default.clone());
    }

    // Const value
    if let Some(const_val) = schema.get("const") {
        return Some(const_val.clone());
    }

    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let mut obj = Map::new();
            if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                for (prop_name, prop_schema) in properties {
                    if let Some(value) = populate_defaults(prop_schema) {
                        obj.insert(prop_name.clone(), value);
                    }
                }
            }
            Some(Value::Object(obj))
        }

        Some("array") => {
            let mut arr = Vec::new();
            if let Some(items_schema) = schema.get("items") {
                if let Some(item) = populate_defaults(items_schema) {
                    arr.push(item);
                }
            }
            Some(Value::Array(arr))
        }

        _ => None,
    }
}

/// Recursively merge `source` values into `reference` defaults.
/// Keys present in `reference` but missing from `source` keep their default.
/// Mirrors Python's synchronize_dicts()
pub fn synchronize(source: &Value, reference: &Value) -> Value {
    match (source, reference) {
        (Value::Object(src_map), Value::Object(ref_map)) => {
            let mut result = ref_map.clone();
            for (key, ref_val) in ref_map {
                if let Some(src_val) = src_map.get(key) {
                    result[key] = synchronize(src_val, ref_val);
                }
            }
            Value::Object(result)
        }
        // Source wins for scalars/arrays
        (src, _) => src.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_populate_defaults_scalar() {
        let schema = json!({ "type": "number", "default": 42.0 });
        assert_eq!(populate_defaults(&schema), Some(json!(42.0)));
    }

    #[test]
    fn test_populate_defaults_object() {
        let schema = json!({
            "type": "object",
            "properties": {
                "x": { "type": "number", "default": 300.0 },
                "y": { "type": "number", "default": 200.0 }
            }
        });
        let result = populate_defaults(&schema).unwrap();
        assert_eq!(result["x"], json!(300.0));
        assert_eq!(result["y"], json!(200.0));
    }

    #[test]
    fn test_populate_defaults_nested() {
        let schema = json!({
            "type": "object",
            "properties": {
                "machine": {
                    "type": "object",
                    "properties": {
                        "max_feed_rate": { "type": "number", "default": 2000.0 }
                    }
                }
            }
        });
        let result = populate_defaults(&schema).unwrap();
        assert_eq!(result["machine"]["max_feed_rate"], json!(2000.0));
    }

    #[test]
    fn test_synchronize_fills_missing() {
        let source = json!({ "x": 100.0 });
        let reference = json!({ "x": 300.0, "y": 200.0 });
        let result = synchronize(&source, &reference);
        assert_eq!(result["x"], json!(100.0)); // source wins
        assert_eq!(result["y"], json!(200.0)); // reference default kept
    }
}