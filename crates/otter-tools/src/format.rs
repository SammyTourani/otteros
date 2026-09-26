use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use otter_json::{parse_str, Value};

/// Validate that a value matches a JSON schema.
pub fn validate_schema(value: &Value, schema_json: &str) -> Result<(), String> {
    // Parse the schema.
    let schema = parse_str(schema_json).map_err(|_| "schema is not JSON")?;

    // Value must be an object.
    if value.as_object().is_none() {
        return Err("input must be a JSON object".to_string());
    }

    // Get required properties from schema.
    let required = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // Get properties schema.
    let properties = schema.get("properties").and_then(|v| v.as_object()).ok_or("schema missing properties")?;

    // Check that all required properties are present.
    for req in &required {
        if !value.get(req).is_some() {
            return Err(format!("missing required property: {}", req));
        }
    }

    // Check that no extra properties are present.
    let value_obj = value.as_object().unwrap();
    for (key, _val) in value_obj.iter() {
        if !properties.contains_key(key) {
            return Err(format!("unknown property: {}", key));
        }
    }

    // Validate types for each property.
    for (key, val) in value_obj.iter() {
        if let Some(prop_schema) = properties.get(key) {
            validate_property(val, prop_schema)?;
        }
    }

    Ok(())
}

/// Validate that a value matches a property schema.
fn validate_property(value: &Value, schema: &Value) -> Result<(), String> {
    // Check if it's an enum.
    if let Some(enum_values) = schema.get("enum").and_then(|v| v.as_array()) {
        let val_str = value.as_str();
        let valid = enum_values.iter().any(|ev| {
            if let Some(ev_str) = ev.as_str()
                && let Some(vs) = val_str
            {
                return ev_str == vs;
            }
            false
        });
        if !valid {
            return Err("value not in enum".to_string());
        }
        return Ok(());
    }

    // Check type.
    if let Some(type_val) = schema.get("type") {
        match type_val {
            Value::String(t) => {
                match t.as_str() {
                    "string" => {
                        if value.as_str().is_none() {
                            return Err("expected string".to_string());
                        }
                    }
                    "object" => {
                        if value.as_object().is_none() {
                            return Err("expected object".to_string());
                        }
                    }
                    "array" => {
                        if value.as_array().is_none() {
                            return Err("expected array".to_string());
                        }
                    }
                    "number" => {
                        if value.as_number().is_none() {
                            return Err("expected number".to_string());
                        }
                    }
                    "boolean" => {
                        if value.as_bool().is_none() {
                            return Err("expected boolean".to_string());
                        }
                    }
                    "null" if !value.is_null() => {
                        return Err("expected null".to_string());
                    }
                    "null" => {}
                    _ => {}
                }
            }
            Value::Array(types) => {
                // Multi-type like ["string", "null"].
                let valid = types.iter().any(|t| {
                    if let Some(t_str) = t.as_str() {
                        match t_str {
                            "string" => value.as_str().is_some(),
                            "null" => value.is_null(),
                            "number" => value.as_number().is_some(),
                            "boolean" => value.as_bool().is_some(),
                            "object" => value.as_object().is_some(),
                            "array" => value.as_array().is_some(),
                            _ => false,
                        }
                    } else {
                        false
                    }
                });
                if !valid {
                    return Err("value does not match any of the allowed types".to_string());
                }
            }
            _ => {}
        }
    }

    Ok(())
}
