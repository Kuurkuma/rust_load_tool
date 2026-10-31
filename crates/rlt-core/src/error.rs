//! Centralized error types for `rlt-core`.

use thiserror::Error;

/// All errors that can occur within the rlt-core engine.
#[derive(Debug, Error)]
pub enum RltError {
    /// A JSON line could not be parsed.
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    /// An Arrow operation failed.
    #[error("Arrow error: {0}")]
    Arrow(#[from] arrow::error::ArrowError),

    /// A Parquet operation failed.
    #[error("Parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    /// An object_store operation failed.
    #[error("Object store error: {0}")]
    ObjectStore(#[from] object_store::Error),

    /// The input JSON value was not a JSON object.
    #[error("expected a JSON object, got: {0}")]
    NotAnObject(String),

    /// Schema coercion failed for a particular field.
    #[error("schema coercion error for field '{field}': cannot widen {from:?} to {to:?}")]
    CoercionError {
        field: String,
        from: arrow::datatypes::DataType,
        to: arrow::datatypes::DataType,
    },

    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A generic internal error with a message.
    #[error("{0}")]
    Internal(String),
}

/// Result type alias used throughout rlt-core.
pub type Result<T> = std::result::Result<T, RltError>;
