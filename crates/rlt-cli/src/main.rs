//! # ⚡ `rlt` — Rust Load Tool CLI & HTTP Worker
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
//!
//! Subcommands:
//! - `stream`: Ingest NDJSON from `stdin` or a local `.jsonl` file.
//! - `serve`: High-throughput HTTP server receiving `POST /ingest` payloads.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

use rlt_core::buffer::{BufferConfig, MicroBatchBuffer};
use rlt_core::flatten::flatten_json_object;
use rlt_core::sink::{Compression, ParquetSink, SinkDestination};

/// rlt — Rust Load Tool: High-performance streaming JSON → Parquet ingestion.
#[derive(Parser, Debug)]
#[command(name = "rlt", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Stream NDJSON from stdin or a file into Parquet files.
    Stream {
        /// Source: "stdin" or a path to a .jsonl file.
        #[arg(long, default_value = "stdin")]
        source: String,

        /// Destination: local path or gs://bucket/prefix.
        #[arg(long)]
        destination: String,

        /// Batch size in megabytes before flushing.
        #[arg(long, default_value = "32")]
        batch_size_mb: usize,

        /// Flush interval in seconds (time-based trigger).
        #[arg(long, default_value = "30")]
        flush_interval_secs: u64,

        /// Compression algorithm.
        #[arg(long, default_value = "zstd")]
        compression: CompressionArg,
    },

    /// Start an HTTP server that accepts POST /ingest with JSON payloads.
    Serve {
        /// Port to listen on.
        #[arg(long, default_value = "8080")]
        port: u16,

        /// Destination: local path or gs://bucket/prefix.
        #[arg(long)]
        destination: String,

        /// Batch size in megabytes before flushing.
        #[arg(long, default_value = "32")]
        batch_size_mb: usize,

        /// Flush interval in seconds (time-based trigger).
        #[arg(long, default_value = "30")]
        flush_interval_secs: u64,

        /// Compression algorithm.
        #[arg(long, default_value = "zstd")]
        compression: CompressionArg,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CompressionArg {
    Zstd,
    Snappy,
    None,
}

impl From<CompressionArg> for Compression {
    fn from(c: CompressionArg) -> Self {
        match c {
            CompressionArg::Zstd => Compression::Zstd,
            CompressionArg::Snappy => Compression::Snappy,
            CompressionArg::None => Compression::None,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Stream {
            source,
            destination,
            batch_size_mb,
            flush_interval_secs,
            compression,
        } => {
            run_stream(
                &source,
                &destination,
                batch_size_mb,
                flush_interval_secs,
                compression.into(),
            )
            .await
        }
        Commands::Serve {
            port,
            destination,
            batch_size_mb,
            flush_interval_secs,
            compression,
        } => {
            run_serve(
                port,
                &destination,
                batch_size_mb,
                flush_interval_secs,
                compression.into(),
            )
            .await
        }
    }
}

/// Run the stream ingestion mode.
async fn run_stream(
    source: &str,
    destination: &str,
    batch_size_mb: usize,
    flush_interval_secs: u64,
    compression: Compression,
) -> Result<()> {
    let config = BufferConfig {
        batch_size_bytes: batch_size_mb * 1024 * 1024,
        max_rows: 500_000,
    };
    let mut buffer = MicroBatchBuffer::new(config);
    let sink = ParquetSink::new(SinkDestination::parse(destination), compression);

    let reader: Box<dyn tokio::io::AsyncBufRead + Unpin + Send> = if source == "stdin" {
        Box::new(BufReader::new(tokio::io::stdin()))
    } else {
        let file = tokio::fs::File::open(source)
            .await
            .with_context(|| format!("failed to open source file: {source}"))?;
        Box::new(BufReader::new(file))
    };

    let mut lines = reader.lines();
    let mut total_rows: u64 = 0;
    let mut total_files: u64 = 0;
    let mut dlq_count: u64 = 0;
    let flush_interval = Duration::from_secs(flush_interval_secs);
    let mut last_flush = tokio::time::Instant::now();

    tracing::info!(
        source = source,
        destination = destination,
        batch_size_mb = batch_size_mb,
        "starting stream ingestion"
    );

    loop {
        let line_result = tokio::time::timeout(flush_interval, lines.next_line()).await;

        match line_result {
            Ok(Ok(Some(line))) => {
                if line.trim().is_empty() {
                    continue;
                }

                match serde_json::from_str::<serde_json::Value>(&line) {
                    Ok(value) => match flatten_json_object(&value) {
                        Ok(flat) => {
                            let should_flush = buffer
                                .push(flat)
                                .context("failed to push row to buffer")?;
                            total_rows += 1;

                            if should_flush {
                                total_files += flush_buffer(&mut buffer, &sink).await?;
                                last_flush = tokio::time::Instant::now();
                            }
                        }
                        Err(e) => {
                            dlq_count += 1;
                            tracing::warn!(error = %e, line = %line, "DLQ: failed to flatten");
                        }
                    },
                    Err(e) => {
                        dlq_count += 1;
                        tracing::warn!(error = %e, "DLQ: failed to parse JSON line");
                    }
                }
            }
            Ok(Ok(None)) => {
                // EOF
                tracing::info!("reached end of input");
                break;
            }
            Ok(Err(e)) => {
                return Err(e).context("I/O error reading lines");
            }
            Err(_) => {
                // Timeout → time-based flush
                if !buffer.is_empty() && last_flush.elapsed() >= flush_interval {
                    tracing::info!("time-based flush triggered");
                    total_files += flush_buffer(&mut buffer, &sink).await?;
                    last_flush = tokio::time::Instant::now();
                }
            }
        }
    }

    // Final flush
    if !buffer.is_empty() {
        total_files += flush_buffer(&mut buffer, &sink).await?;
    }

    tracing::info!(
        total_rows = total_rows,
        total_files = total_files,
        dlq_count = dlq_count,
        "stream ingestion complete"
    );

    Ok(())
}

/// Flush the buffer and write to the sink.
async fn flush_buffer(buffer: &mut MicroBatchBuffer, sink: &ParquetSink) -> Result<u64> {
    match buffer.flush().context("failed to flush buffer")? {
        Some(batch) => {
            let path = sink
                .write(&batch)
                .await
                .context("failed to write parquet")?;
            tracing::info!(
                rows = batch.num_rows(),
                columns = batch.num_columns(),
                path = %path,
                "flushed batch to parquet"
            );
            Ok(1)
        }
        None => Ok(0),
    }
}

// ─── HTTP Server (Axum) ───────────────────────────────────────────────────

/// Shared state for the HTTP server.
struct AppState {
    buffer: Mutex<MicroBatchBuffer>,
    sink: ParquetSink,
}

/// Run the HTTP server mode.
async fn run_serve(
    port: u16,
    destination: &str,
    batch_size_mb: usize,
    flush_interval_secs: u64,
    compression: Compression,
) -> Result<()> {
    let config = BufferConfig {
        batch_size_bytes: batch_size_mb * 1024 * 1024,
        max_rows: 500_000,
    };

    let state = Arc::new(AppState {
        buffer: Mutex::new(MicroBatchBuffer::new(config)),
        sink: ParquetSink::new(SinkDestination::parse(destination), compression),
    });

    // Spawn a background task for time-based flushing
    let flush_state = state.clone();
    let flush_interval = Duration::from_secs(flush_interval_secs);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(flush_interval);
        loop {
            interval.tick().await;
            let mut buffer = flush_state.buffer.lock().await;
            if !buffer.is_empty() {
                match buffer.flush() {
                    Ok(Some(batch)) => {
                        match flush_state.sink.write(&batch).await {
                            Ok(path) => {
                                tracing::info!(
                                    rows = batch.num_rows(),
                                    path = %path,
                                    "time-based flush: wrote parquet"
                                );
                            }
                            Err(e) => {
                                tracing::error!(error = %e, "time-based flush: write failed");
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!(error = %e, "time-based flush: buffer flush failed");
                    }
                }
            }
        }
    });

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/ingest", post(ingest_handler))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(port = port, destination = destination, "starting HTTP server");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind to port {port}"))?;

    axum::serve(listener, app)
        .await
        .context("server error")?;

    Ok(())
}

/// Health check endpoint.
async fn health_handler() -> &'static str {
    "ok"
}

/// Ingest endpoint: accepts a JSON object or an array of JSON objects.
async fn ingest_handler(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> std::result::Result<(StatusCode, String), (StatusCode, String)> {
    let value: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("invalid JSON: {e}"),
        )
    })?;

    let rows: Vec<serde_json::Value> = match &value {
        serde_json::Value::Array(arr) => arr.clone(),
        serde_json::Value::Object(_) => vec![value],
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "expected a JSON object or array".to_string(),
            ));
        }
    };

    let mut ingested = 0u64;
    let mut dlq = 0u64;
    let mut should_flush = false;

    {
        let mut buffer = state.buffer.lock().await;
        for row_value in &rows {
            match flatten_json_object(row_value) {
                Ok(flat) => match buffer.push(flat) {
                    Ok(flush) => {
                        ingested += 1;
                        should_flush = should_flush || flush;
                    }
                    Err(e) => {
                        dlq += 1;
                        tracing::warn!(error = %e, "DLQ: buffer push failed");
                    }
                },
                Err(e) => {
                    dlq += 1;
                    tracing::warn!(error = %e, "DLQ: flatten failed");
                }
            }
        }

        if should_flush {
            match buffer.flush() {
                Ok(Some(batch)) => {
                    match state.sink.write(&batch).await {
                        Ok(path) => {
                            tracing::info!(
                                rows = batch.num_rows(),
                                path = %path,
                                "size-based flush: wrote parquet"
                            );
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "flush write failed");
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(error = %e, "flush failed");
                }
            }
        }
    }

    Ok((
        StatusCode::OK,
        format!(
            r#"{{"ingested": {}, "dlq": {}}}"#,
            ingested, dlq
        ),
    ))
}
