//! A small JSON Schema subset for model replies. Written here rather than
//! taken from a general validator so that no reply or schema can make the
//! app resolve remote or file references.
use serde_json::{Map, Value};

const STRUCTURAL: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
    "anyOf",
    "description",
];
/// Checked locally but not sent to providers, some of which reject them in strict mode.
const LOCAL_ONLY: &[&str] = &[
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "minimum",
    "maximum",
];
const MAX_DEPTH: usize = 32;

#[derive(Clone, Debug)]
pub struct Schema(Value);

impl Schema {
    /// Rejects keywords the validator does not implement, so a schema can
    /// never appear stricter than it is.
    pub fn new(schema: Value) -> Result<Self, String> {
        check_keywords(&schema, "$", 0)?;
        Ok(Self(schema))
    }

    pub fn validate(&self, value: &Value) -> Result<(), String> {
        validate(&self.0, value, "$", 0)
    }

    /// The schema as sent to a provider: structural keywords only.
    pub fn provider_view(&self) -> Value {
        strip(&self.0)
    }

    pub fn raw(&self) -> &Value {
        &self.0
    }
}

fn check_keywords(schema: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: schema is nested too deeply"));
    }
    let obj = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema must be an object"))?;
    for (key, value) in obj {
        if !STRUCTURAL.contains(&key.as_str()) && !LOCAL_ONLY.contains(&key.as_str()) {
            return Err(format!("{path}: unsupported keyword {key}"));
        }
        match key.as_str() {
            "properties" => {
                for (name, sub) in value.as_object().ok_or(format!("{path}: bad properties"))? {
                    check_keywords(sub, &format!("{path}.{name}"), depth + 1)?;
                }
            }
            "items" => check_keywords(value, &format!("{path}[]"), depth + 1)?,
            "anyOf" => {
                for (i, sub) in value
                    .as_array()
                    .ok_or(format!("{path}: bad anyOf"))?
                    .iter()
                    .enumerate()
                {
                    check_keywords(sub, &format!("{path}|{i}"), depth + 1)?;
                }
            }
            "additionalProperties" if value != &Value::Bool(false) => {
                return Err(format!("{path}: additionalProperties must be false"));
            }
            _ => {}
        }
    }
    if obj.get("type") == Some(&Value::String("object".into())) && !obj.contains_key("additionalProperties") {
        return Err(format!("{path}: objects must set additionalProperties to false"));
    }
    Ok(())
}

fn strip(schema: &Value) -> Value {
    let Some(obj) = schema.as_object() else {
        return schema.clone();
    };
    let mut out = Map::new();
    for (key, value) in obj {
        if LOCAL_ONLY.contains(&key.as_str()) {
            continue;
        }
        let value = match key.as_str() {
            "properties" => Value::Object(
                value
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (k.clone(), strip(v)))
                    .collect(),
            ),
            "items" => strip(value),
            "anyOf" => Value::Array(value.as_array().into_iter().flatten().map(strip).collect()),
            _ => value.clone(),
        };
        out.insert(key.clone(), value);
    }
    Value::Object(out)
}

fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => false,
    }
}

fn validate(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err(format!("{path}: nested too deeply"));
    }
    let Some(s) = schema.as_object() else {
        return Ok(());
    };
    if let Some(options) = s.get("anyOf").and_then(Value::as_array) {
        if !options
            .iter()
            .any(|o| validate(o, value, path, depth + 1).is_ok())
        {
            return Err(format!("{path}: matches none of the allowed forms"));
        }
    }
    match s.get("type") {
        Some(Value::String(t)) if !type_matches(t, value) => return Err(format!("{path}: expected {t}")),
        Some(Value::Array(ts))
            if !ts
                .iter()
                .filter_map(Value::as_str)
                .any(|t| type_matches(t, value)) =>
        {
            return Err(format!("{path}: wrong type"));
        }
        _ => {}
    }
    if let Some(c) = s.get("const") {
        if c != value {
            return Err(format!("{path}: must be {c}"));
        }
    }
    if let Some(options) = s.get("enum").and_then(Value::as_array) {
        if !options.contains(value) {
            return Err(format!("{path}: not an allowed value"));
        }
    }
    if let Some(text) = value.as_str() {
        let len = text.chars().count() as u64;
        if s.get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|m| len < m)
        {
            return Err(format!("{path}: too short"));
        }
        if s.get("maxLength")
            .and_then(Value::as_u64)
            .is_some_and(|m| len > m)
        {
            return Err(format!("{path}: too long"));
        }
    }
    if let Some(n) = value.as_f64() {
        if s.get("minimum").and_then(Value::as_f64).is_some_and(|m| n < m) {
            return Err(format!("{path}: below minimum"));
        }
        if s.get("maximum").and_then(Value::as_f64).is_some_and(|m| n > m) {
            return Err(format!("{path}: above maximum"));
        }
    }
    if let Some(items) = value.as_array() {
        let len = items.len() as u64;
        if s.get("minItems").and_then(Value::as_u64).is_some_and(|m| len < m) {
            return Err(format!("{path}: too few items"));
        }
        if s.get("maxItems").and_then(Value::as_u64).is_some_and(|m| len > m) {
            return Err(format!("{path}: too many items"));
        }
        if let Some(item_schema) = s.get("items") {
            for (i, item) in items.iter().enumerate() {
                validate(item_schema, item, &format!("{path}[{i}]"), depth + 1)?;
            }
        }
    }
    if let Some(obj) = value.as_object() {
        let props = s.get("properties").and_then(Value::as_object);
        for name in s.get("required").and_then(Value::as_array).into_iter().flatten() {
            let name = name.as_str().unwrap_or_default();
            if !obj.contains_key(name) {
                return Err(format!("{path}: missing {name}"));
            }
        }
        for (key, item) in obj {
            match props.and_then(|p| p.get(key)) {
                Some(sub) => validate(sub, item, &format!("{path}.{key}"), depth + 1)?,
                None if s.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{path}: unexpected field"));
                }
                None => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn person() -> Schema {
        Schema::new(json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 5},
                "kind": {"type": "string", "enum": ["artist", "label"]},
                "score": {"type": "number", "minimum": 0, "maximum": 1},
                "tags": {"type": "array", "items": {"type": "string"}, "maxItems": 2}
            },
            "required": ["name", "kind", "score", "tags"],
            "additionalProperties": false
        }))
        .unwrap()
    }

    #[test]
    fn accepts_valid_and_rejects_each_violation() {
        let s = person();
        s.validate(&json!({"name": "Ab", "kind": "artist", "score": 0.5, "tags": []}))
            .unwrap();
        for bad in [
            json!({"kind": "artist", "score": 0.5, "tags": []}),
            json!({"name": "", "kind": "artist", "score": 0.5, "tags": []}),
            json!({"name": "Abcdef", "kind": "artist", "score": 0.5, "tags": []}),
            json!({"name": "Ab", "kind": "dj", "score": 0.5, "tags": []}),
            json!({"name": "Ab", "kind": "artist", "score": 2, "tags": []}),
            json!({"name": "Ab", "kind": "artist", "score": 0.5, "tags": ["a", "b", "c"]}),
            json!({"name": "Ab", "kind": "artist", "score": 0.5, "tags": [], "url": "x"}),
            json!({"name": 3, "kind": "artist", "score": 0.5, "tags": []}),
            json!("not an object"),
        ] {
            assert!(s.validate(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unsupported_keywords_and_open_objects_are_refused() {
        assert!(Schema::new(json!({"$ref": "https://example.com/s.json"})).is_err());
        assert!(Schema::new(json!({"type": "string", "pattern": "^a"})).is_err());
        assert!(Schema::new(json!({"type": "object", "properties": {}})).is_err());
        assert!(Schema::new(json!({"type": "object", "additionalProperties": true})).is_err());
    }

    #[test]
    fn provider_view_drops_local_bounds_only() {
        let view = person().provider_view();
        let text = view.to_string();
        assert!(!text.contains("maxLength") && !text.contains("minimum") && !text.contains("maxItems"));
        assert!(text.contains("additionalProperties") && text.contains("enum"));
    }

    #[test]
    fn properties_keep_their_written_order() {
        let view = person().provider_view().to_string();
        let order: Vec<usize> = ["\"name\"", "\"kind\"", "\"score\"", "\"tags\""]
            .iter()
            .map(|k| view.find(k).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{view}");
    }

    #[test]
    fn any_of_and_const_select_one_form() {
        let s = Schema::new(json!({
            "anyOf": [
                {"type": "object", "properties": {"tool": {"const": "a"}}, "required": ["tool"], "additionalProperties": false},
                {"type": "object", "properties": {"tool": {"const": "b"}}, "required": ["tool"], "additionalProperties": false}
            ]
        }))
        .unwrap();
        s.validate(&json!({"tool": "b"})).unwrap();
        assert!(s.validate(&json!({"tool": "c"})).is_err());
    }
}
