//! # ⚡ `rlt-core` — Core Engine for the Rust Load Tool
//!
//! The Cloud-Native Ingestion Engine: Turn messy, unpredictable JSON streams into
//! clean Apache Parquet files on Google Cloud Storage—with zero GC pauses and at a
//! fraction of the cost.
//!
//! ## Architecture & Data Flow
//!
//! ```text
//!                       INCOMING STREAM
//!              (HTTP Webhooks or Batch JSON logs)
//!                            │
//!                            ▼
//! ┌─────────────────────────────────────────────────────────────┐
//! │                 CLOUD RUN CONTAINER (rlt)                   │
//! │                                                             │
//! │  [1] INGESTION BUFFER (Tokio Async)                         │
//! │      └─ Non-blocking network byte intake                    │
//! │                                                             │
//! │  [2] JSON NORMALIZATION                                     │
//! │      └─ Recursively flattens nested objects:                │
//! │         {"client": {"id": 10}}  ===>  client__id: 10        │
//! │                                                             │
//! │  [3] DYNAMIC SCHEMA ENGINE (arrow-rs)                       │
//! │      ├─ Compares against active schema state                │
//! │      ├─ New column?  ──> Appended with `null` defaults      │
//! │      └─ Type conflict? ──> Progressive type widening        │
//! │                            (Int -> Float -> String)         │
//! │                                                             │
//! │  [4] MICRO-BATCH MEMORY CHUNKING                            │
//! │      └─ Slices records into in-memory Apache Arrow batches  │
//! │                                                             │
//! │  [5] PARQUET ENCODING & COMPRESSION                         │
//! │      ├─ Zstandard (ZSTD) compression                        │
//! │      └─ Flush triggers: 64 MB buffer OR 60 seconds elapsed  │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼ Multipart Upload (Zero Disk Copy)
//! ┌─────────────────────────────────────────────────────────────┐
//! │                 GOOGLE CLOUD STORAGE (Bucket)               │
//! │                                                             │
//! │  gs://my-bucket/events/                                     │
//! │       └── year=2026/month=10/day=31/chunk_a7f9.parquet      │
//! └─────────────────────────────────────────────────────────────┘
//!                            │
//!                            ▼ (Directly queryable)
//!                [ BigQuery External Table / dbt ]
//! ```

pub mod buffer;
pub mod error;
pub mod flatten;
pub mod schema;
pub mod sink;

pub use buffer::MicroBatchBuffer;
pub use error::RltError;
pub use flatten::flatten_json_object;
pub use schema::SchemaManager;
pub use sink::{Compression, ParquetSink, SinkDestination};
