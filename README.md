# ⚡ rlt (Rust Load Tool)

> **The Cloud-Native Ingestion Engine: Turn messy, unpredictable JSON streams into clean Apache Parquet files on Google Cloud Storage—with zero GC pauses and at a fraction of the cost.**

---

## 🎯 Why rlt? (The Problem)

In a modern GCP data architecture:
1. **BigQuery and dbt** excel at modeling and transforming data (**T**).
2. However, the ingestion phase (**E-L**) is often offloaded to resource-heavy Python scripts or microservices.

### Bottlenecks Under Load:
* 🐢 **Painful Cold Starts:** Python containers take 3 to 5 seconds to boot during traffic spikes on Cloud Run.
* 💥 **Schema Drift Fragility:** If an upstream API adds a new field or changes an integer into a string, the pipeline either crashes or pollutes the destination table.
* 💸 **Memory Waste:** Python allocates significant memory overhead when parsing and transforming dynamic JSON, forcing teams to overprovision Cloud Run containers.

**`rlt` solves these challenges by combining the raw efficiency of Rust, the Apache Arrow columnar ecosystem, and automated schema evolution.**

---

## 🏗️ Architecture & Data Flow

Here is how an incoming JSON event moves through `rlt` to land as an optimized Parquet file in Google Cloud Storage:

```text
                      INCOMING STREAM
             (HTTP Webhooks or Batch JSON logs)
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                 CLOUD RUN CONTAINER (rlt)                   │
│                                                             │
│  [1] INGESTION BUFFER (Tokio Async)                         │
│      └─ Non-blocking network byte intake                    │
│                                                             │
│  [2] JSON NORMALIZATION                                     │
│      └─ Recursively flattens nested objects:                │
│         {"client": {"id": 10}}  ===>  client__id: 10        │
│                                                             │
│  [3] DYNAMIC SCHEMA ENGINE (arrow-rs)                       │
│      ├─ Compares against active schema state                │
│      ├─ New column?  ──> Appended with `null` defaults      │
│      └─ Type conflict? ──> Progressive type widening        │
│                            (Int -> Float -> String)         │
│                                                             │
│  [4] MICRO-BATCH MEMORY CHUNKING                            │
│      └─ Slices records into in-memory Apache Arrow batches  │
│                                                             │
│  [5] PARQUET ENCODING & COMPRESSION                         │
│      ├─ Zstandard (ZSTD) compression                        │
│      └─ Flush triggers: 64 MB buffer OR 60 seconds elapsed  │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼ Multipart Upload (Zero Disk Copy)
┌─────────────────────────────────────────────────────────────┐
│                 GOOGLE CLOUD STORAGE (Bucket)               │
│                                                             │
│  gs://my-bucket/events/                                     │
│       └── year=2026/month=10/day=31/chunk_a7f9.parquet      │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼ (Directly queryable)
               [ BigQuery External Table / dbt ]
```

---

## ✨ Features

- **Dynamic Schema Drift & Monotonic Type Widening**: Automatically detects and evolves schema over heterogeneous JSON streams without dropping fields.
  - `Int64` + `Float64` $\to$ `Float64`
  - Incompatible types (e.g. `Int64` + `Utf8` or `Boolean` + `Int64`) $\to$ fallback to `Utf8`
  - New columns $\to$ added dynamically as `nullable: true`
- **Recursive JSON Key Flattener**: Transforms nested objects (`{"parent": {"child": 42}}` $\to$ `parent__child: 42`). Arrays are serialized as JSON strings.
- **Zero GC & Dual-Trigger Micro-Batching**: Micro-batches triggered by estimated memory buffer size (e.g. 32 MB / 64 MB) or time interval (e.g. 30s / 60s), converting in-memory maps into Apache Arrow `RecordBatch`.
- **Multi-Destination Parquet Sink**:
  - Local filesystem (`LocalFileSystem`) and Google Cloud Storage (`gs://bucket/prefix`) via `object_store`.
  - Automatic Hive partitioning: `year=YYYY/month=MM/day=DD/data_{uuid}.parquet`.
  - Configurable compression: `ZSTD` (level 3, default), `Snappy`, or `None`.
- **Dual Interfaces**:
  - `stream`: High-throughput CLI ingesting from `stdin` or NDJSON files.
  - `serve`: Non-blocking Axum HTTP server ingesting webhooks via `POST /ingest` (single object or array of objects).
- **Dead-Letter Queue (DLQ)**: Corrupted or unparseable JSON payloads are safely isolated and accounted for without interrupting the ingestion pipeline.

---

## 📁 Workspace Structure

```
├── Cargo.toml
├── crates/
│   ├── rlt-core/           # Engine logic
│   │   ├── src/
│   │   │   ├── error.rs    # thiserror RltError
│   │   │   ├── schema.rs   # SchemaManager & type-widening lattice
│   │   │   ├── flatten.rs  # Recursive JSON flattener
│   │   │   ├── buffer.rs   # MicroBatchBuffer -> Arrow RecordBatch
│   │   │   └── sink.rs     # ParquetSink (Local & GCS)
│   │   └── Cargo.toml
│   └── rlt-cli/            # CLI binary and Axum server
│       ├── src/
│       │   └── main.rs     # clap derive CLI + axum endpoints
│       └── Cargo.toml
└── tests/
    └── e2e_drift_test.rs   # 10,000-row heterogeneous schema validation
```

---

## 🚀 Quickstart

### Build

```bash
# Debug build
cargo build

# Optimized release build (LTO = true, opt-level = 3)
cargo build --release
```

### CLI Ingestion (`stream`)

Stream from stdin:
```bash
printf '{"id": 1, "name": "alice"}\n{"id": "two", "name": "bob", "score": 95.5}\n' \
  | ./target/release/rlt stream --source stdin --destination ./output --batch-size-mb 16
```

Stream from file:
```bash
./target/release/rlt stream --source events.jsonl --destination ./output --compression zstd
```

Stream directly to Google Cloud Storage:
```bash
./target/release/rlt stream --source events.jsonl --destination gs://my-bucket/events/
```

### HTTP Webhook Ingestion (`serve`)

Start server on port 8080:
```bash
./target/release/rlt serve --port 8080 --destination ./output --flush-interval-secs 10
```

Post a single JSON event:
```bash
curl -X POST http://localhost:8080/ingest \
  -H "Content-Type: application/json" \
  -d '{"user": {"id": 1, "name": "Alice"}, "event": "login"}'
```

Post an array of events:
```bash
curl -X POST http://localhost:8080/ingest \
  -H "Content-Type: application/json" \
  -d '[{"id": 1, "action": "click"}, {"id": "two", "action": "scroll", "duration_ms": 120.5}]'
```

Health check:
```bash
curl http://localhost:8080/health
```

---

## ☁️ Google Cloud & Cloud Run Deployment

`rlt` is designed specifically for containerized, serverless ingestion on **Google Cloud Run** writing to **Google Cloud Storage (GCS)** and queried by **BigQuery**.

### Quick Deployment Steps:

1. **Build container with Cloud Build** (using multi-stage [`Dockerfile`](Dockerfile)):
   ```bash
   gcloud builds submit --tag europe-west9-docker.pkg.dev/$PROJECT_ID/rlt-repo/rlt:latest .
   ```

2. **Deploy to Cloud Run with scale-to-zero**:
   ```bash
   gcloud run deploy rlt-ingest-service \
     --image=europe-west9-docker.pkg.dev/$PROJECT_ID/rlt-repo/rlt:latest \
     --region=europe-west9 \
     --memory=512Mi \
     --cpu=1 \
     --concurrency=80 \
     --min-instances=0 \
     --set-env-vars="DESTINATION=gs://$MY_BUCKET/events,BATCH_SIZE_MB=32,FLUSH_INTERVAL_SECS=30" \
     --allow-unauthenticated
   ```

3. **Query via BigQuery External Table**:
   ```sql
   CREATE OR REPLACE EXTERNAL TABLE `analytics.rlt_events`
   WITH PARTITION COLUMNS (year INT64, month INT64, day INT64)
   OPTIONS (
       format = 'PARQUET',
       uris = ['gs://my-bucket/events/*'],
       hive_partition_uri_prefix = 'gs://my-bucket/events/'
   );
   ```

📖 **For step-by-step IAM permissions, lifecycle rules, and architecture details, read the complete guide: [docs/DEPLOYMENT_GCP.md](docs/DEPLOYMENT_GCP.md).**

---

## 🧪 Verification & Testing

```bash
# Run unit tests and documentation tests (35 passed)
cargo test

# Run the 10,000-row end-to-end schema drift test
cargo test --test e2e_drift_test

# Enforce clean lints
cargo clippy --all-targets -- -D warnings
```
