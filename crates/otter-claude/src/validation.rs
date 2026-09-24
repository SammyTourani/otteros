use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use otter_json::{Value, Object, parse_str};

/// Errors that can occur during validation.
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Invalid JSON in input
    InvalidJson,
    /// Missing required property
    MissingRequired(String),
    /// Type mismatch
    TypeError {
        /// Property name
        property: String,
        /// Expected type
        expected: &'static str,
        /// Actual type
        actual: &'static str,
    },
    /// Extra property not allowed
    ExtraProperty(String),
    /// Enum value not allowed
    InvalidEnum {
        /// Property name
        property: String,
        /// The disallowed value
        value: String,
    },
}

impl ValidationError {
    /// Format the error as a message for the API.
    pub fn as_message(&self) -> String {
        match self {
            ValidationError::InvalidJson => "INVALID_JSON: Failed to parse input".into(),
            ValidationError::MissingRequired(prop) => {
                format!("INVALID_JSON: Missing required property '{}'", prop)
            }
            ValidationError::TypeError { property, expected, actual } => {
                format!(
                    "INVALID_JSON: Property '{}' must be {}, got {}",
                    property, expected, actual
                )
            }
            ValidationError::ExtraProperty(prop) => {
                format!("INVALID_JSON: Unexpected property '{}'", prop)
            }
            ValidationError::InvalidEnum { property, value } => {
                format!("INVALID_JSON: Property '{}' has invalid value '{}'", property, value)
            }
        }
    }
}

/// Validates tool input against a JSON schema.
#[allow(dead_code)]
pub fn validate_tool_input(
    input_str: &str,
    schema: &Object,
) -> Result<String, ValidationError> {
    // Parse the input JSON
    let input = parse_str(input_str).map_err(|_| ValidationError::InvalidJson)?;

    // Input must be an object
    let input_obj = input.as_object().ok_or(ValidationError::InvalidJson)?;

    // Validate against the schema
    validate_object(input_obj, schema)?;

    // Return the original string if valid
    Ok(input_str.to_string())
}

/// Validates an object against a schema.
fn validate_object(obj: &Object, schema: &Object) -> Result<(), ValidationError> {
    // Get required properties
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect()
        })
        .unwrap_or_default();

    // Get properties schema
    let properties = schema.get("properties").and_then(|v| v.as_object());

    // Check all required properties are present
    for req_prop in &required {
        if !obj.contains_key(req_prop) {
            return Err(ValidationError::MissingRequired(req_prop.to_string()));
        }
    }

    // Check additionalProperties setting
    let allow_additional = schema
        .get("additionalProperties")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    // Validate each property
    for (key, value) in obj.iter() {
        if let Some(props) = properties {
            if let Some(prop_schema) = props.get(key).and_then(|v| v.as_object()) {
                // Validate against the property schema
                validate_value(value, prop_schema)?;
            } else if !allow_additional {
                // Extra property not allowed
                return Err(ValidationError::ExtraProperty(key.to_string()));
            }
        } else if !allow_additional {
            // No properties defined and additionalProperties is false
            return Err(ValidationError::ExtraProperty(key.to_string()));
        }
    }

    Ok(())
}

/// Validates a value against a schema.
#[allow(clippy::collapsible_match)]
fn validate_value(value: &Value, schema: &Object) -> Result<(), ValidationError> {
    // Get the type constraint
    if let Some(type_str) = schema.get("type").and_then(|v| v.as_str()) {
        match type_str {
            "object" => {
                if !value.is_null() {
                    if let Some(obj) = value.as_object() {
                        validate_object(obj, schema)?;
                    } else {
                        return Err(ValidationError::TypeError {
                            property: "<unknown>".to_string(),
                            expected: "object",
                            actual: get_value_type(value),
                        });
                    }
                }
            }
            "array" => {
                if let Some(arr) = value.as_array() {
                    if let Some(items_schema) = schema.get("items").and_then(|v| v.as_object()) {
                        for item in arr {
                            validate_value(item, items_schema)?;
                        }
                    }
                } else {
                    return Err(ValidationError::TypeError {
                        property: "<unknown>".to_string(),
                        expected: "array",
                        actual: get_value_type(value),
                    });
                }
            }
            "string" => {
                if !value.as_str().is_some() && !value.is_null() {
                    return Err(ValidationError::TypeError {
                        property: "<unknown>".to_string(),
                        expected: "string",
                        actual: get_value_type(value),
                    });
                }
            }
            "number" => {
                if !value.as_number().is_some() && !value.is_null() {
                    return Err(ValidationError::TypeError {
                        property: "<unknown>".to_string(),
                        expected: "number",
                        actual: get_value_type(value),
                    });
                }
            }
            "integer" => {
                if !value.as_i64().is_some() && !value.is_null() {
                    return Err(ValidationError::TypeError {
                        property: "<unknown>".to_string(),
                        expected: "integer",
                        actual: get_value_type(value),
                    });
                }
            }
            "boolean" => {
                if !value.as_bool().is_some() && !value.is_null() {
                    return Err(ValidationError::TypeError {
                        property: "<unknown>".to_string(),
                        expected: "boolean",
                        actual: get_value_type(value),
                    });
                }
            }
            _ => {}
        }
    }

    // Check enum constraint
    if let Some(enum_values) = schema.get("enum").and_then(|v| v.as_array()) {
        let valid = enum_values.iter().any(|e| e == value);
        if !valid {
            let value_str = match value {
                Value::String(s) => s.clone(),
                _ => format!("{:?}", value),
            };
            return Err(ValidationError::InvalidEnum {
                property: "<unknown>".to_string(),
                value: value_str,
            });
        }
    }

    Ok(())
}

/// Gets a human-readable type name for a value.
fn get_value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
