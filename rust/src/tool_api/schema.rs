use serde_json::{Map, Value};

use crate::tool_api::error::ToolCallError;

pub fn apply_defaults(schema: &Value, args: &mut Value) {
    if !is_object_schema(schema) {
        return;
    }

    let Some(args_obj) = args.as_object_mut() else {
        return;
    };

    apply_defaults_object(schema, args_obj);
}

pub fn validate_required(
    tool_name: &str,
    schema: &Value,
    args: &Value,
) -> Result<(), ToolCallError> {
    if !is_object_schema(schema) {
        return Ok(());
    }

    let Some(args_obj) = args.as_object() else {
        return Err(ToolCallError::InvalidArguments(format!(
            "Tool '{tool_name}' expects JSON object arguments"
        )));
    };

    validate_required_object(tool_name, schema, args_obj, "$")
}

fn apply_defaults_object(schema: &Value, args: &mut Map<String, Value>) {
    let Some(schema_obj) = schema.as_object() else {
        return;
    };

    let Some(properties) = schema_obj.get("properties").and_then(Value::as_object) else {
        return;
    };

    for (key, property_schema) in properties {
        if !args.contains_key(key) {
            if let Some(default_value) = property_schema.get("default") {
                args.insert(key.clone(), default_value.clone());
            }
        }

        if let Some(current_value) = args.get_mut(key) {
            if is_object_schema(property_schema) {
                if let Some(current_obj) = current_value.as_object_mut() {
                    apply_defaults_object(property_schema, current_obj);
                }
            }
        }
    }
}

fn validate_required_object(
    tool_name: &str,
    schema: &Value,
    args: &Map<String, Value>,
    path: &str,
) -> Result<(), ToolCallError> {
    let Some(schema_obj) = schema.as_object() else {
        return Ok(());
    };

    if let Some(required) = schema_obj.get("required").and_then(Value::as_array) {
        for key_value in required {
            let Some(key) = key_value.as_str() else {
                continue;
            };
            if !args.contains_key(key) {
                return Err(ToolCallError::MissingRequired {
                    tool: tool_name.to_string(),
                    path: path.to_string(),
                    key: key.to_string(),
                });
            }
        }
    }

    let Some(properties) = schema_obj.get("properties").and_then(Value::as_object) else {
        return Ok(());
    };

    for (key, property_schema) in properties {
        if !is_object_schema(property_schema) {
            continue;
        }

        let Some(value) = args.get(key) else {
            continue;
        };

        let Some(value_obj) = value.as_object() else {
            continue;
        };

        let next_path = format!("{path}.{key}");
        validate_required_object(tool_name, property_schema, value_obj, &next_path)?;
    }

    Ok(())
}

fn is_object_schema(schema: &Value) -> bool {
    let Some(schema_obj) = schema.as_object() else {
        return false;
    };

    match schema_obj.get("type") {
        Some(Value::String(t)) => t == "object",
        Some(_) => false,
        None => schema_obj.contains_key("properties") || schema_obj.contains_key("required"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_defaults_sets_top_level_and_nested_defaults() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "city": { "type": "string" },
                "units": { "type": "string", "default": "metric" },
                "prefs": {
                    "type": "object",
                    "properties": {
                        "lang": { "type": "string", "default": "en" }
                    }
                }
            }
        });

        let mut args = serde_json::json!({"city": "Paris", "prefs": {}});
        apply_defaults(&schema, &mut args);

        assert_eq!(
            args.get("units"),
            Some(&Value::String("metric".to_string()))
        );
        assert_eq!(
            args.pointer("/prefs/lang"),
            Some(&Value::String("en".to_string()))
        );
    }

    #[test]
    fn apply_defaults_does_not_override_existing_values() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "units": { "type": "string", "default": "metric" }
            }
        });

        let mut args = serde_json::json!({"units": "imperial"});
        apply_defaults(&schema, &mut args);

        assert_eq!(
            args.get("units"),
            Some(&Value::String("imperial".to_string()))
        );
    }

    #[test]
    fn validate_required_accepts_default_fulfilled_values() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "units": { "type": "string", "default": "metric" }
            },
            "required": ["units"]
        });

        let mut args = serde_json::json!({});
        apply_defaults(&schema, &mut args);

        let result = validate_required("weather", &schema, &args);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_required_reports_missing_nested_key_with_path() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "filters": {
                    "type": "object",
                    "required": ["city"]
                }
            },
            "required": ["filters"]
        });

        let args = serde_json::json!({"filters": {}});
        let err = validate_required("weather", &schema, &args).expect_err("expected missing key");

        match err {
            ToolCallError::MissingRequired { path, key, .. } => {
                assert_eq!(path, "$.filters");
                assert_eq!(key, "city");
            }
            other => panic!("unexpected error: {other}"),
        }
    }
}
