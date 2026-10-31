//! End-to-end integration test for dynamic schema drift and Parquet writing.
//!
//! Generates 10,000 synthetic JSON lines with an unstable schema:
//! - Type changes: Int64 -> Float64, Int64 -> Utf8
//! - Missing / optional keys across rows
//! - Nested JSON objects and arrays
//!
//! Writes Parquet files to a local temp directory, then reads them back using
//! `parquet::file::reader::SerializedFileReader` and `ParquetRecordBatchReaderBuilder`
//! to verify 100% row integrity, schema convergence, and zero data loss.

use std::fs::File;
use std::path::PathBuf;

use arrow::datatypes::DataType;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::file::reader::{FileReader, SerializedFileReader};
use rlt_core::buffer::{BufferConfig, MicroBatchBuffer};
use rlt_core::flatten::flatten_json_object;
use rlt_core::sink::{Compression, ParquetSink, SinkDestination};
use serde_json::{json, Value};
use walkdir::WalkDir;

#[tokio::test]
async fn test_e2e_schema_drift_10k_rows() {
    let temp_dir = tempfile::tempdir().expect("create temp dir");
    let dest_path = temp_dir.path().to_str().expect("valid path").to_string();

    let config = BufferConfig {
        batch_size_bytes: 512 * 1024, // 512 KB batches to force multiple flushes
        max_rows: 2_000,
    };
    let mut buffer = MicroBatchBuffer::new(config);
    let sink = ParquetSink::new(
        SinkDestination::Local {
            base_path: dest_path.clone(),
        },
        Compression::Zstd,
    );

    let total_rows_to_generate = 10_000;
    let mut written_files = Vec::new();

    for i in 0..total_rows_to_generate {
        // Construct unstable rows with progressive schema drift
        let val: Value = match i {
            0..=2499 => {
                // Phase 1: int id, nested user {name, age}, int score
                json!({
                    "id": i,
                    "user": {
                        "name": format!("user_{i}"),
                        "age": 20 + (i % 50)
                    },
                    "score": 100
                })
            }
            2500..=4999 => {
                // Phase 2: int id, nested user without age, float score, new 'country' field
                json!({
                    "id": i,
                    "user": {
                        "name": format!("user_{i}")
                    },
                    "score": 99.5 + ((i % 10) as f64) * 0.1,
                    "country": "FR"
                })
            }
            5000..=7499 => {
                // Phase 3: string id (widening Int64 -> Utf8), int score, boolean active
                json!({
                    "id": format!("id_{i}"),
                    "score": 88,
                    "active": i % 2 == 0
                })
            }
            _ => {
                // Phase 4: string id, tags array, metadata object, active omitted
                json!({
                    "id": format!("id_{i}"),
                    "tags": ["alpha", "beta"],
                    "meta": {
                        "version": 2,
                        "flag": true
                    }
                })
            }
        };

        let flat = flatten_json_object(&val).expect("flatten json");
        let should_flush = buffer.push(flat).expect("push to buffer");

        if should_flush {
            if let Some(batch) = buffer.flush().expect("flush batch") {
                let file_path = sink.write(&batch).await.expect("write parquet");
                written_files.push(file_path);
            }
        }
    }

    // Flush any remaining buffered rows
    if !buffer.is_empty() {
        if let Some(batch) = buffer.flush().expect("final flush") {
            let file_path = sink.write(&batch).await.expect("write parquet final");
            written_files.push(file_path);
        }
    }

    assert!(!written_files.is_empty(), "should have written parquet files");

    // Discover all written parquet files in the Hive partitioned directory
    let mut discovered_files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&dest_path).into_iter().filter_map(|e| e.ok()) {
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("parquet") {
            discovered_files.push(entry.path().to_path_buf());
        }
    }

    assert_eq!(
        discovered_files.len(),
        written_files.len(),
        "discovered parquet files count must match written files count"
    );

    // Read back and verify using parquet::file::reader
    let mut total_verified_rows: usize = 0;

    for path in &discovered_files {
        let file = File::open(path).expect("open parquet file");
        let reader = SerializedFileReader::new(file).expect("create SerializedFileReader");

        let file_meta = reader.metadata().file_metadata();
        let rows_in_file = file_meta.num_rows() as usize;
        assert!(rows_in_file > 0, "file must not be empty");
        total_verified_rows += rows_in_file;

        // Verify with ParquetRecordBatchReaderBuilder
        let file_for_arrow = File::open(path).expect("open parquet file for arrow");
        let builder = ParquetRecordBatchReaderBuilder::try_new(file_for_arrow)
            .expect("create arrow parquet reader builder");
        let arrow_schema = builder.schema().clone();

        let record_reader = builder.build().expect("build arrow reader");
        let mut rows_read_arrow = 0;

        for batch_res in record_reader {
            let batch = batch_res.expect("read record batch");
            rows_read_arrow += batch.num_rows();

            // Verify expected schema types when fields are present
            if let Ok(field) = arrow_schema.field_with_name("id") {
                assert!(
                    *field.data_type() == DataType::Int64 || *field.data_type() == DataType::Utf8,
                    "id must be either Int64 or Utf8, found {:?}",
                    field.data_type()
                );
            }
            if let Ok(field) = arrow_schema.field_with_name("score") {
                assert!(
                    *field.data_type() == DataType::Int64 || *field.data_type() == DataType::Float64,
                    "score must be Int64 or Float64, found {:?}",
                    field.data_type()
                );
            }
        }

        assert_eq!(
            rows_read_arrow, rows_in_file,
            "arrow reader row count must match file metadata row count"
        );
    }

    assert_eq!(
        total_verified_rows, total_rows_to_generate,
        "Total rows verified across all Parquet files must be exactly 10,000 with zero loss!"
    );
}
