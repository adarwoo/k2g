use serde_yaml::{Mapping, Value};

#[derive(Debug, Clone)]
pub struct TemplateEngine {
    context: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    InvalidRootContext,
    InvalidTestExpression(String),
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRootContext => write!(f, "template context root must be a mapping"),
            Self::InvalidTestExpression(expr) => {
                write!(f, "invalid test expression: '{expr}'")
            }
        }
    }
}

impl std::error::Error for EngineError {}

impl TemplateEngine {
    pub fn new(context: Value) -> Result<Self, EngineError> {
        if !matches!(context, Value::Mapping(_)) {
            return Err(EngineError::InvalidRootContext);
        }

        Ok(Self { context })
    }

    pub fn render_node(&self, node: &Value) -> Result<Vec<String>, EngineError> {
        match node {
            Value::Null => Ok(Vec::new()),
            Value::String(text) => Ok(vec![self.render_template_line(text)]),
            Value::Sequence(items) => {
                let mut out = Vec::new();

                for item in items {
                    out.extend(self.render_node(item)?);
                }

                Ok(out)
            }
            Value::Mapping(mapping) => self.render_mapping(mapping),
            _ => Ok(vec![self.scalar_to_string(node)]),
        }
    }

    fn render_mapping(&self, mapping: &Mapping) -> Result<Vec<String>, EngineError> {
        if let Some(test_value) = mapping.get(Value::String(String::from("test"))) {
            return self.render_test_block(mapping, test_value);
        }

        Ok(Vec::new())
    }

    fn render_test_block(&self, mapping: &Mapping, test_value: &Value) -> Result<Vec<String>, EngineError> {
        let expression = match test_value {
            Value::String(text) => text,
            _ => {
                return Err(EngineError::InvalidTestExpression(String::from(
                    "test value must be a string",
                )))
            }
        };

        let result = self.eval_test(expression)?;

        let branch = if result {
            mapping
                .get(Value::Bool(true))
                .or_else(|| mapping.get(Value::String(String::from("true"))))
                .unwrap_or(&Value::Null)
        } else {
            mapping
                .get(Value::Bool(false))
                .or_else(|| mapping.get(Value::String(String::from("false"))))
                .unwrap_or(&Value::Null)
        };
        self.render_node(branch)
    }

    fn eval_test(&self, expression: &str) -> Result<bool, EngineError> {
        if let Some((left, right)) = expression.split_once("==") {
            let left_value = self.resolve_operand(left.trim())?;
            let right_value = self.resolve_operand(right.trim())?;
            return Ok(left_value == right_value);
        }

        if let Some((left, right)) = expression.split_once("!=") {
            let left_value = self.resolve_operand(left.trim())?;
            let right_value = self.resolve_operand(right.trim())?;
            return Ok(left_value != right_value);
        }

        Err(EngineError::InvalidTestExpression(expression.to_string()))
    }

    fn resolve_operand(&self, token: &str) -> Result<String, EngineError> {
        if let Some(placeholder) = token.strip_prefix('@') {
            return Ok(self.resolve_placeholder(placeholder));
        }

        Ok(token.trim_matches('"').trim_matches('\'').to_string())
    }

    pub fn render_template_line(&self, template: &str) -> String {
        let bytes = template.as_bytes();
        let mut idx = 0usize;
        let mut output = String::with_capacity(template.len());

        while idx < bytes.len() {
            if bytes[idx] == b'@' {
                let start = idx + 1;
                idx = start;

                while idx < bytes.len() {
                    let c = bytes[idx] as char;
                    if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                        idx += 1;
                    } else {
                        break;
                    }
                }

                if idx > start {
                    let token = &template[start..idx];
                    output.push_str(&self.resolve_placeholder(token));
                    continue;
                }

                output.push('@');
                continue;
            }

            output.push(bytes[idx] as char);
            idx += 1;
        }

        output
    }

    fn resolve_placeholder(&self, token: &str) -> String {
        let mut parts: Vec<&str> = token.split("::").filter(|part| !part.is_empty()).collect();
        if parts.is_empty() {
            return String::new();
        }

        let transform = match parts.last().copied() {
            Some("mm") | Some("mm_min") | Some("rpm") => parts.pop(),
            _ => None,
        };

        let Some(value) = self.lookup_path(&parts) else {
            return format!("@{token}");
        };

        self.apply_transform(value, transform)
    }

    fn lookup_path<'a>(&'a self, parts: &[&str]) -> Option<&'a Value> {
        let mut current = &self.context;

        for part in parts {
            current = match current {
                Value::Mapping(map) => map.get(Value::String((*part).to_string()))?,
                _ => return None,
            };
        }

        Some(current)
    }

    fn apply_transform(&self, value: &Value, transform: Option<&str>) -> String {
        let rendered = self.scalar_to_string(value);

        match transform {
            Some("mm") | Some("mm_min") | Some("rpm") | None => rendered,
            Some(_) => rendered,
        }
    }

    fn scalar_to_string(&self, value: &Value) -> String {
        match value {
            Value::Null => String::new(),
            Value::Bool(value) => value.to_string(),
            Value::Number(value) => value.to_string(),
            Value::String(value) => value.clone(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TemplateEngine;
    use serde_yaml::Value;

    fn context() -> Value {
        serde_yaml::from_str(
            r#"
index: 2
last_index: 2
x: 10.5
y: 3
gs:
  z_safe_height: 20
"#,
        )
        .expect("context YAML should parse")
    }

    #[test]
    fn renders_variable_placeholders_with_at_syntax() {
        let engine = TemplateEngine::new(context()).expect("engine should initialize");

        let line = "G0 X@x::mm Y@y::mm Z@gs::z_safe_height::mm";
        let rendered = engine.render_template_line(line);

        assert_eq!(rendered, "G0 X10.5 Y3 Z20");
    }

    #[test]
    fn renders_true_branch_for_test_expression() {
        let engine = TemplateEngine::new(context()).expect("engine should initialize");

        let node: Value = serde_yaml::from_str(
            r#"
test: "@last_index == @index"
true:
  - "G0 X@x::mm Y@y::mm Z@gs::z_safe_height::mm"
false:
  - "G80"
"#,
        )
        .expect("branch YAML should parse");

        let rendered = engine.render_node(&node).expect("render should succeed");
        assert_eq!(rendered, vec![String::from("G0 X10.5 Y3 Z20")]);
    }

    #[test]
    fn renders_false_branch_for_test_expression() {
        let ctx: Value = serde_yaml::from_str(
            r#"
index: 1
last_index: 2
"#,
        )
        .expect("context YAML should parse");
        let engine = TemplateEngine::new(ctx).expect("engine should initialize");

        let node: Value = serde_yaml::from_str(
            r#"
test: "@last_index == @index"
true:
  - "G0 X@x::mm"
false:
  - "G80"
"#,
        )
        .expect("branch YAML should parse");

        let rendered = engine.render_node(&node).expect("render should succeed");
        assert_eq!(rendered, vec![String::from("G80")]);
    }
}