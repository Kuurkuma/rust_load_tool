//! Dynamic schema drift detection and type widening.
//!
//! The [`SchemaManager`] observes JSON rows and maintains a unified Arrow schema
//! that evolves monotonically via a type-widening lattice.

use arrow::datatypes::{DataType, Field, Schema};
use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::error::Result;

/// Manages a dynamically evolving Arrow schema.
///
/// As new JSON rows are observed, the schema widens according to the type
/// lattice. Fields are never removed; they can only be added (as nullable)
/// or widened to a broader type.
#[derive(Debug, Clone)]
pub struct SchemaManager {
    /// Ordered map of field name → current DataType.
    fields: IndexMap<String, DataType>,
}

impl Default for SchemaManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaManager {
    /// Create a new, empty schema manager.
    pub fn new() -> Self {
        Self {
            fields: IndexMap::new(),
        }
    }

    /// Create a schema manager pre-loaded with an existing Arrow schema.
    pub fn from_schema(schema: &Schema) -> Self {
        let mut fields = IndexMap::new();
        for field in schema.fields() {
            fields.insert(field.name().clone(), field.data_type().clone());
        }
        Self { fields }
    }

    /// Observe a single flattened JSON row and update the unified schema.
    ///
    /// Each key-value pair in the map is inspected:
    /// - New keys are added as nullable fields.
    /// - Existing keys are widened if the incoming type differs.
    pub fn observe(&mut self, row: &Map<String, Value>) -> Result<()> {
        for (key, value) in row {
            let incoming_type = infer_type(value);
            match self.fields.get(key) {
                Some(existing) => {
                    let widened = widen_types(existing, &incoming_type)?;
                    self.fields.insert(key.clone(), widened);
                }
                None => {
                    self.fields.insert(key.clone(), incoming_type);
                }
            }
        }
        Ok(())
    }

    /// Return the current unified Arrow schema. All fields are nullable.
    pub fn current_schema(&self) -> Schema {
        let fields: Vec<Field> = self
            .fields
            .iter()
            .map(|(name, dtype)| Field::new(name, dtype.clone(), true))
            .collect();
        Schema::new(fields)
    }

    /// Return the number of fields currently tracked.
    pub fn field_count(&self) -> usize {
        self.fields.len()
    }

    /// Merge two Arrow schemas using the type-widening lattice.
    ///
    /// Fields present in only one schema become nullable in the result.
    pub fn merge_schema(a: &Schema, b: &Schema) -> Result<Schema> {
        let mut merged: IndexMap<String, DataType> = IndexMap::new();

        // Add all fields from schema a
        for field in a.fields() {
            merged.insert(field.name().clone(), field.data_type().clone());
        }

        // Merge fields from schema b
        for field in b.fields() {
            match merged.get(field.name()) {
                Some(existing) => {
                    let widened = widen_types(existing, field.data_type())?;
                    merged.insert(field.name().clone(), widened);
                }
                None => {
                    merged.insert(field.name().clone(), field.data_type().clone());
                }
            }
        }

        let fields: Vec<Field> = merged
            .into_iter()
            .map(|(name, dtype)| Field::new(name, dtype, true))
            .collect();

        Ok(Schema::new(fields))
    }
}

/// Infer the Arrow DataType from a JSON value.
fn infer_type(value: &Value) -> DataType {
    match value {
        Value::Null => DataType::Null,
        Value::Bool(_) => DataType::Boolean,
        Value::Number(n) => {
            if n.is_i64() {
                DataType::Int64
            } else {
                DataType::Float64
            }
        }
        Value::String(_) => DataType::Utf8,
        // Arrays and nested objects are serialized as JSON strings
        Value::Array(_) | Value::Object(_) => DataType::Utf8,
    }
}

/// Apply the type-widening lattice to produce the least upper bound of two types.
///
/// # Widening rules
///
/// | Existing   | Incoming   | Result    |
/// |------------|------------|-----------|
/// | Null       | T          | T         |
/// | T          | Null       | T         |
/// | Int64      | Float64    | Float64   |
/// | Float64    | Int64      | Float64   |
/// | T          | T          | T         |
/// | T          | U (incompat)| Utf8     |
fn widen_types(existing: &DataType, incoming: &DataType) -> Result<DataType> {
    // Same type — no widening needed
    if existing == incoming {
        return Ok(existing.clone());
    }

    // Null is the bottom of the lattice
    if *existing == DataType::Null {
        return Ok(incoming.clone());
    }
    if *incoming == DataType::Null {
        return Ok(existing.clone());
    }

    // Numeric widening: Int64 + Float64 → Float64
    if (*existing == DataType::Int64 && *incoming == DataType::Float64)
        || (*existing == DataType::Float64 && *incoming == DataType::Int64)
    {
        return Ok(DataType::Float64);
    }

    // If either side is already Utf8, stay Utf8
    if *existing == DataType::Utf8 || *incoming == DataType::Utf8 {
        return Ok(DataType::Utf8);
    }

    // Incompatible types → fallback to Utf8
    tracing::debug!(
        existing = ?existing,
        incoming = ?incoming,
        "incompatible types, widening to Utf8"
    );
    Ok(DataType::Utf8)
}

/// Convert a serde_json Value to a string representation suitable for a Utf8 column.
pub fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Coerce a JSON value to match a target DataType.
///
/// If the value's natural type doesn't match the target, we attempt
/// to convert it (e.g., stringify an integer if the target is Utf8).
pub fn coerce_value(value: &Value, target: &DataType) -> Value {
    let natural = infer_type(value);
    if natural == *target || *value == Value::Null {
        return value.clone();
    }

    match target {
        DataType::Utf8 => Value::String(value_to_string(value)),
        DataType::Float64 => {
            // Try to promote Int64 → Float64
            if let Some(i) = value.as_i64() {
                Value::Number(
                    serde_json::Number::from_f64(i as f64)
                        .unwrap_or_else(|| serde_json::Number::from(i)),
                )
            } else {
                Value::String(value_to_string(value))
            }
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_types() {
        assert_eq!(infer_type(&Value::Null), DataType::Null);
        assert_eq!(infer_type(&Value::Bool(true)), DataType::Boolean);
        assert_eq!(infer_type(&serde_json::json!(42)), DataType::Int64);
        assert_eq!(infer_type(&serde_json::json!(2.5)), DataType::Float64);
        assert_eq!(infer_type(&serde_json::json!("hello")), DataType::Utf8);
        assert_eq!(infer_type(&serde_json::json!([1, 2, 3])), DataType::Utf8);
        assert_eq!(infer_type(&serde_json::json!({"a": 1})), DataType::Utf8);
    }

    #[test]
    fn test_widen_same_types() {
        assert_eq!(
            widen_types(&DataType::Int64, &DataType::Int64).ok(),
            Some(DataType::Int64)
        );
        assert_eq!(
            widen_types(&DataType::Utf8, &DataType::Utf8).ok(),
            Some(DataType::Utf8)
        );
    }

    #[test]
    fn test_widen_null_bottom() {
        assert_eq!(
            widen_types(&DataType::Null, &DataType::Int64).ok(),
            Some(DataType::Int64)
        );
        assert_eq!(
            widen_types(&DataType::Float64, &DataType::Null).ok(),
            Some(DataType::Float64)
        );
    }

    #[test]
    fn test_widen_int_float() {
        assert_eq!(
            widen_types(&DataType::Int64, &DataType::Float64).ok(),
            Some(DataType::Float64)
        );
        assert_eq!(
            widen_types(&DataType::Float64, &DataType::Int64).ok(),
            Some(DataType::Float64)
        );
    }

    #[test]
    fn test_widen_incompatible_to_utf8() {
        assert_eq!(
            widen_types(&DataType::Boolean, &DataType::Int64).ok(),
            Some(DataType::Utf8)
        );
        assert_eq!(
            widen_types(&DataType::Int64, &DataType::Boolean).ok(),
            Some(DataType::Utf8)
        );
    }

    #[test]
    fn test_widen_to_utf8_absorbs() {
        assert_eq!(
            widen_types(&DataType::Int64, &DataType::Utf8).ok(),
            Some(DataType::Utf8)
        );
        assert_eq!(
            widen_types(&DataType::Utf8, &DataType::Float64).ok(),
            Some(DataType::Utf8)
        );
    }

    #[test]
    fn test_schema_manager_observe_basic() {
        let mut mgr = SchemaManager::new();

        let row: Map<String, Value> = serde_json::from_str(r#"{"name": "alice", "age": 30}"#)
            .expect("valid json");
        mgr.observe(&row).expect("observe should succeed");

        let schema = mgr.current_schema();
        assert_eq!(schema.fields().len(), 2);
        assert_eq!(
            schema.field_with_name("name").ok().map(|f| f.data_type().clone()),
            Some(DataType::Utf8)
        );
        assert_eq!(
            schema.field_with_name("age").ok().map(|f| f.data_type().clone()),
            Some(DataType::Int64)
        );
    }

    #[test]
    fn test_schema_manager_drift_new_column() {
        let mut mgr = SchemaManager::new();

        let row1: Map<String, Value> = serde_json::from_str(r#"{"a": 1}"#).expect("valid");
        let row2: Map<String, Value> = serde_json::from_str(r#"{"a": 2, "b": "new"}"#).expect("valid");

        mgr.observe(&row1).expect("ok");
        assert_eq!(mgr.field_count(), 1);

        mgr.observe(&row2).expect("ok");
        assert_eq!(mgr.field_count(), 2);

        let schema = mgr.current_schema();
        assert!(schema.field_with_name("b").is_ok());
        assert!(schema.field_with_name("b").map(|f| f.is_nullable()).unwrap_or(false));
    }

    #[test]
    fn test_schema_manager_drift_type_widening() {
        let mut mgr = SchemaManager::new();

        // First see int
        let row1: Map<String, Value> = serde_json::from_str(r#"{"val": 42}"#).expect("valid");
        mgr.observe(&row1).expect("ok");
        assert_eq!(
            schema_type(&mgr.current_schema(), "val"),
            Some(DataType::Int64)
        );

        // Then see float → widens to Float64
        let row2: Map<String, Value> = serde_json::from_str(r#"{"val": 2.5}"#).expect("valid");
        mgr.observe(&row2).expect("ok");
        assert_eq!(
            schema_type(&mgr.current_schema(), "val"),
            Some(DataType::Float64)
        );
    }

    #[test]
    fn test_schema_manager_incompatible_fallback_utf8() {
        let mut mgr = SchemaManager::new();

        let row1: Map<String, Value> = serde_json::from_str(r#"{"x": 42}"#).expect("valid");
        let row2: Map<String, Value> = serde_json::from_str(r#"{"x": "hello"}"#).expect("valid");

        mgr.observe(&row1).expect("ok");
        mgr.observe(&row2).expect("ok");

        assert_eq!(
            schema_type(&mgr.current_schema(), "x"),
            Some(DataType::Utf8)
        );
    }

    #[test]
    fn test_merge_schemas() {
        let s1 = Schema::new(vec![
            Field::new("a", DataType::Int64, true),
            Field::new("b", DataType::Utf8, true),
        ]);
        let s2 = Schema::new(vec![
            Field::new("a", DataType::Float64, true),
            Field::new("c", DataType::Boolean, true),
        ]);

        let merged = SchemaManager::merge_schema(&s1, &s2).expect("merge should succeed");
        assert_eq!(merged.fields().len(), 3);
        assert_eq!(schema_type(&merged, "a"), Some(DataType::Float64));
        assert_eq!(schema_type(&merged, "b"), Some(DataType::Utf8));
        assert_eq!(schema_type(&merged, "c"), Some(DataType::Boolean));
    }

    #[test]
    fn test_coerce_int_to_utf8() {
        let v = serde_json::json!(42);
        let coerced = coerce_value(&v, &DataType::Utf8);
        assert_eq!(coerced, Value::String("42".to_string()));
    }

    #[test]
    fn test_coerce_int_to_float() {
        let v = serde_json::json!(42);
        let coerced = coerce_value(&v, &DataType::Float64);
        assert!(coerced.as_f64().is_some());
        assert!((coerced.as_f64().unwrap_or(0.0) - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_coerce_null_stays_null() {
        let v = Value::Null;
        let coerced = coerce_value(&v, &DataType::Utf8);
        assert_eq!(coerced, Value::Null);
    }

    fn schema_type(schema: &Schema, name: &str) -> Option<DataType> {
        schema.field_with_name(name).ok().map(|f| f.data_type().clone())
    }

    #[test]
    fn test_heterogeneous_stream_convergence() {
        // Simulate a realistic heterogeneous stream
        let rows = vec![
            r#"{"id": 1, "name": "alice", "score": 95}"#,
            r#"{"id": 2, "name": "bob", "score": 88.5}"#,          // score Int→Float
            r#"{"id": "three", "name": "charlie", "score": 72}"#,  // id Int→Utf8
            r#"{"id": 4, "name": "diana", "active": true}"#,       // new col
            r#"{"id": 5, "score": null}"#,                          // missing name
        ];

        let mut mgr = SchemaManager::new();
        for raw in &rows {
            let row: Map<String, Value> = serde_json::from_str(raw).expect("valid");
            mgr.observe(&row).expect("observe ok");
        }

        let schema = mgr.current_schema();
        assert_eq!(schema_type(&schema, "id"), Some(DataType::Utf8));
        assert_eq!(schema_type(&schema, "name"), Some(DataType::Utf8));
        assert_eq!(schema_type(&schema, "score"), Some(DataType::Float64));
        assert_eq!(schema_type(&schema, "active"), Some(DataType::Boolean));
    }
}
