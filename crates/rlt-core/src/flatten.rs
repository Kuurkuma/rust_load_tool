//! Recursive JSON key flattener.
//!
//! Converts nested JSON objects into a flat `Map<String, Value>` using `__`
//! as the key separator. Arrays are serialized to JSON strings.

use serde_json::{Map, Value};

use crate::error::{Result, RltError};

/// Recursively flatten a JSON object.
///
/// Nested keys are joined with `__`. Arrays and nested objects that contain
/// further nesting are serialized as JSON strings at the leaf level.
///
/// # Examples
///
/// ```
/// use serde_json::json;
/// use rlt_core::flatten_json_object;
///
/// let input = json!({"parent": {"child": 42, "list": [1,2,3]}});
/// let flat = flatten_json_object(&input).unwrap();
/// assert_eq!(flat.get("parent__child").and_then(|v| v.as_i64()), Some(42));
/// assert!(flat.get("parent__list").and_then(|v| v.as_str()).is_some());
/// ```
pub fn flatten_json_object(value: &Value) -> Result<Map<String, Value>> {
    match value {
        Value::Object(map) => {
            let mut result = Map::new();
            flatten_recursive(map, &mut String::new(), &mut result);
            Ok(result)
        }
        other => Err(RltError::NotAnObject(format!("{}", other))),
    }
}

/// Internal recursive helper.
fn flatten_recursive(
    map: &Map<String, Value>,
    prefix: &mut String,
    output: &mut Map<String, Value>,
) {
    for (key, value) in map {
        let full_key = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}__{key}")
        };

        match value {
            Value::Object(inner) => {
                flatten_recursive(inner, &mut full_key.clone(), output);
            }
            Value::Array(_) => {
                // Serialize arrays as JSON strings
                output.insert(full_key, Value::String(value.to_string()));
            }
            other => {
                output.insert(full_key, other.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_flat_object_unchanged() {
        let input = json!({"a": 1, "b": "hello", "c": true});
        let flat = flatten_json_object(&input).expect("should flatten");
        assert_eq!(flat.len(), 3);
        assert_eq!(flat.get("a").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(flat.get("b").and_then(|v| v.as_str()), Some("hello"));
        assert_eq!(flat.get("c").and_then(|v| v.as_bool()), Some(true));
    }

    #[test]
    fn test_nested_single_level() {
        let input = json!({"parent": {"child": 42}});
        let flat = flatten_json_object(&input).expect("should flatten");
        assert_eq!(flat.len(), 1);
        assert_eq!(flat.get("parent__child").and_then(|v| v.as_i64()), Some(42));
    }

    #[test]
    fn test_nested_multi_level() {
        let input = json!({"a": {"b": {"c": {"d": "deep"}}}});
        let flat = flatten_json_object(&input).expect("should flatten");
        assert_eq!(flat.get("a__b__c__d").and_then(|v| v.as_str()), Some("deep"));
    }

    #[test]
    fn test_array_serialized() {
        let input = json!({"tags": [1, 2, 3]});
        let flat = flatten_json_object(&input).expect("should flatten");
        let tags = flat.get("tags").and_then(|v| v.as_str());
        assert_eq!(tags, Some("[1,2,3]"));
    }

    #[test]
    fn test_mixed_nested_and_flat() {
        let input = json!({
            "id": 1,
            "meta": {
                "source": "api",
                "geo": {
                    "lat": 48.8566,
                    "lon": 2.3522
                }
            },
            "tags": ["a", "b"]
        });
        let flat = flatten_json_object(&input).expect("should flatten");
        assert_eq!(flat.get("id").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(flat.get("meta__source").and_then(|v| v.as_str()), Some("api"));
        assert!((flat.get("meta__geo__lat").and_then(|v| v.as_f64()).unwrap_or(0.0) - 48.8566).abs() < 0.0001);
        assert!((flat.get("meta__geo__lon").and_then(|v| v.as_f64()).unwrap_or(0.0) - 2.3522).abs() < 0.0001);
        assert!(flat.get("tags").and_then(|v| v.as_str()).is_some());
    }

    #[test]
    fn test_not_an_object_error() {
        let input = json!([1, 2, 3]);
        assert!(flatten_json_object(&input).is_err());
    }

    #[test]
    fn test_null_values_preserved() {
        let input = json!({"a": null, "b": {"c": null}});
        let flat = flatten_json_object(&input).expect("should flatten");
        assert_eq!(flat.get("a"), Some(&Value::Null));
        assert_eq!(flat.get("b__c"), Some(&Value::Null));
    }
}
