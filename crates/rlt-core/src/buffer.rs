//! Micro-batch buffer that accumulates flattened JSON rows and flushes
//! them as Arrow `RecordBatch` objects.
//!
//! Flush triggers:
//! - Estimated memory size exceeds `batch_size_bytes`.
//! - Row count exceeds an internal limit.
//!
//! Corrupted/unparseable rows are routed to a dead-letter queue callback.

use std::sync::Arc;

use arrow::array::{
    ArrayRef, BooleanBuilder, Float64Builder, Int64Builder, NullArray,
    StringBuilder,
};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use serde_json::{Map, Value};

use crate::error::{Result, RltError};
use crate::schema::{coerce_value, SchemaManager};

/// Configuration for the micro-batch buffer.
#[derive(Debug, Clone)]
pub struct BufferConfig {
    /// Maximum estimated batch size in bytes before a flush is triggered.
    pub batch_size_bytes: usize,
    /// Maximum number of rows before a flush is triggered.
    pub max_rows: usize,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            batch_size_bytes: 32 * 1024 * 1024, // 32 MB
            max_rows: 500_000,
        }
    }
}

/// A micro-batch buffer that accumulates flattened JSON rows.
#[derive(Debug)]
pub struct MicroBatchBuffer {
    /// Buffer configuration.
    config: BufferConfig,
    /// The schema manager tracking the evolving schema.
    schema_manager: SchemaManager,
    /// Accumulated rows (flattened JSON objects).
    rows: Vec<Map<String, Value>>,
    /// Estimated total size in bytes of accumulated rows.
    estimated_bytes: usize,
}

impl MicroBatchBuffer {
    /// Create a new buffer with the given configuration.
    pub fn new(config: BufferConfig) -> Self {
        Self {
            config,
            schema_manager: SchemaManager::new(),
            rows: Vec::new(),
            estimated_bytes: 0,
        }
    }

    /// Create a new buffer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(BufferConfig::default())
    }

    /// Add a flattened JSON row to the buffer.
    ///
    /// Returns `true` if the buffer should be flushed after this insertion.
    pub fn push(&mut self, row: Map<String, Value>) -> Result<bool> {
        // Update schema with this row
        self.schema_manager.observe(&row)?;

        // Estimate memory contribution (~rough: sum of key+value string lengths)
        let row_size: usize = row
            .iter()
            .map(|(k, v)| k.len() + estimate_value_size(v))
            .sum();
        self.estimated_bytes += row_size;

        self.rows.push(row);

        Ok(self.should_flush())
    }

    /// Check whether the buffer should be flushed.
    pub fn should_flush(&self) -> bool {
        self.estimated_bytes >= self.config.batch_size_bytes || self.rows.len() >= self.config.max_rows
    }

    /// Return the number of rows currently buffered.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Get a reference to the current schema manager.
    pub fn schema_manager(&self) -> &SchemaManager {
        &self.schema_manager
    }

    /// Flush the buffer: convert all accumulated rows into an Arrow `RecordBatch`
    /// using the current unified schema, then clear the buffer.
    ///
    /// Returns `None` if the buffer is empty.
    pub fn flush(&mut self) -> Result<Option<RecordBatch>> {
        if self.rows.is_empty() {
            return Ok(None);
        }

        let schema = self.schema_manager.current_schema();
        let batch = build_record_batch(&schema, &self.rows)?;

        // Clear the buffer but keep the schema
        self.rows.clear();
        self.estimated_bytes = 0;

        Ok(Some(batch))
    }

    /// Return the current unified schema.
    pub fn current_schema(&self) -> Schema {
        self.schema_manager.current_schema()
    }
}

/// Build an Arrow `RecordBatch` from a slice of flattened JSON rows
/// according to the given schema.
fn build_record_batch(schema: &Schema, rows: &[Map<String, Value>]) -> Result<RecordBatch> {
    let num_rows = rows.len();
    let mut columns: Vec<ArrayRef> = Vec::with_capacity(schema.fields().len());

    for field in schema.fields() {
        let array = build_column(field, rows, num_rows)?;
        columns.push(array);
    }

    let batch = RecordBatch::try_new(Arc::new(schema.clone()), columns)?;
    Ok(batch)
}

/// Build a single column array from the given field definition and rows.
fn build_column(
    field: &Field,
    rows: &[Map<String, Value>],
    num_rows: usize,
) -> Result<ArrayRef> {
    let name = field.name();
    let dtype = field.data_type();

    match dtype {
        DataType::Null => {
            Ok(Arc::new(NullArray::new(num_rows)))
        }
        DataType::Boolean => {
            let mut builder = BooleanBuilder::with_capacity(num_rows);
            for row in rows {
                match row.get(name) {
                    Some(Value::Bool(b)) => builder.append_value(*b),
                    Some(Value::Null) | None => builder.append_null(),
                    Some(other) => {
                        // Coerce to string-boolean
                        let s = other.to_string();
                        match s.as_str() {
                            "true" => builder.append_value(true),
                            "false" => builder.append_value(false),
                            _ => builder.append_null(),
                        }
                    }
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Int64 => {
            let mut builder = Int64Builder::with_capacity(num_rows);
            for row in rows {
                match row.get(name) {
                    Some(Value::Number(n)) => {
                        if let Some(i) = n.as_i64() {
                            builder.append_value(i);
                        } else {
                            builder.append_null();
                        }
                    }
                    Some(Value::Null) | None => builder.append_null(),
                    Some(_) => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Float64 => {
            let mut builder = Float64Builder::with_capacity(num_rows);
            for row in rows {
                match row.get(name) {
                    Some(Value::Number(n)) => {
                        if let Some(f) = n.as_f64() {
                            builder.append_value(f);
                        } else {
                            builder.append_null();
                        }
                    }
                    Some(Value::Null) | None => builder.append_null(),
                    Some(_) => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        DataType::Utf8 => {
            let mut builder = StringBuilder::with_capacity(num_rows, num_rows * 32);
            for row in rows {
                match row.get(name) {
                    Some(Value::String(s)) => builder.append_value(s),
                    Some(Value::Null) | None => builder.append_null(),
                    Some(other) => {
                        // Coerce non-string values to their JSON representation
                        let coerced = coerce_value(other, &DataType::Utf8);
                        match coerced {
                            Value::String(s) => builder.append_value(&s),
                            Value::Null => builder.append_null(),
                            _ => builder.append_value(coerced.to_string()),
                        }
                    }
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        other => Err(RltError::Internal(format!(
            "unsupported data type for column '{}': {:?}",
            name, other
        ))),
    }
}

/// Rough estimate of the in-memory size of a JSON value.
fn estimate_value_size(value: &Value) -> usize {
    match value {
        Value::Null => 1,
        Value::Bool(_) => 1,
        Value::Number(_) => 8,
        Value::String(s) => s.len(),
        Value::Array(arr) => arr.iter().map(estimate_value_size).sum::<usize>() + 2,
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| k.len() + estimate_value_size(v))
            .sum::<usize>()
            + 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_flat_row(raw: &str) -> Map<String, Value> {
        let v: Value = serde_json::from_str(raw).expect("valid json");
        crate::flatten::flatten_json_object(&v).expect("flatten ok")
    }

    #[test]
    fn test_buffer_push_and_flush() {
        let config = BufferConfig {
            batch_size_bytes: 1024 * 1024,
            max_rows: 100,
        };
        let mut buffer = MicroBatchBuffer::new(config);

        for i in 0..10 {
            let row = make_flat_row(&format!(r#"{{"id": {}, "name": "user{}"}}"#, i, i));
            buffer.push(row).expect("push ok");
        }

        assert_eq!(buffer.len(), 10);

        let batch = buffer.flush().expect("flush ok").expect("should have batch");
        assert_eq!(batch.num_rows(), 10);
        assert_eq!(batch.num_columns(), 2);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_buffer_schema_drift() {
        let mut buffer = MicroBatchBuffer::with_defaults();

        // Row 1: id is integer
        buffer
            .push(make_flat_row(r#"{"id": 1, "name": "alice"}"#))
            .expect("ok");

        // Row 2: id is now a string → schema widens to Utf8
        buffer
            .push(make_flat_row(r#"{"id": "two", "name": "bob"}"#))
            .expect("ok");

        // Row 3: new column appears
        buffer
            .push(make_flat_row(r#"{"id": 3, "name": "charlie", "score": 95.5}"#))
            .expect("ok");

        let batch = buffer.flush().expect("flush ok").expect("should have batch");
        assert_eq!(batch.num_rows(), 3);
        assert_eq!(batch.num_columns(), 3);

        let schema = batch.schema();
        // id should be Utf8 after drift
        assert_eq!(
            schema.field_with_name("id").ok().map(|f| f.data_type().clone()),
            Some(DataType::Utf8)
        );
        // score should be Float64
        assert_eq!(
            schema.field_with_name("score").ok().map(|f| f.data_type().clone()),
            Some(DataType::Float64)
        );
    }

    #[test]
    fn test_buffer_flush_empty() {
        let mut buffer = MicroBatchBuffer::with_defaults();
        let result = buffer.flush().expect("flush ok");
        assert!(result.is_none());
    }

    #[test]
    fn test_buffer_should_flush_by_rows() {
        let config = BufferConfig {
            batch_size_bytes: usize::MAX,
            max_rows: 5,
        };
        let mut buffer = MicroBatchBuffer::new(config);

        for i in 0..4 {
            let should = buffer
                .push(make_flat_row(&format!(r#"{{"x": {}}}"#, i)))
                .expect("ok");
            assert!(!should);
        }

        let should = buffer
            .push(make_flat_row(r#"{"x": 4}"#))
            .expect("ok");
        assert!(should);
    }

    #[test]
    fn test_buffer_coercion_int_as_utf8() {
        let mut buffer = MicroBatchBuffer::with_defaults();

        // First row: val is int
        buffer
            .push(make_flat_row(r#"{"val": 42}"#))
            .expect("ok");
        // Second row: val is string → schema widens to Utf8
        buffer
            .push(make_flat_row(r#"{"val": "hello"}"#))
            .expect("ok");

        let batch = buffer.flush().expect("ok").expect("batch");
        let schema = batch.schema();
        assert_eq!(
            schema.field_with_name("val").ok().map(|f| f.data_type().clone()),
            Some(DataType::Utf8)
        );

        // The integer 42 should have been coerced to the string "42"
        let col = batch.column(0);
        let str_array = col
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .expect("should be string array");
        assert_eq!(str_array.value(0), "42");
        assert_eq!(str_array.value(1), "hello");
    }

    #[test]
    fn test_nested_json_flattened_then_buffered() {
        let mut buffer = MicroBatchBuffer::with_defaults();

        let row = make_flat_row(r#"{"user": {"name": "alice", "age": 30}, "active": true}"#);
        buffer.push(row).expect("ok");

        let batch = buffer.flush().expect("ok").expect("batch");
        let schema = batch.schema();
        assert!(schema.field_with_name("user__name").is_ok());
        assert!(schema.field_with_name("user__age").is_ok());
        assert!(schema.field_with_name("active").is_ok());
    }
}
