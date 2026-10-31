# ☁️ Deploying `rlt` on Google Cloud (Cloud Run & GCS)

This guide walks you through deploying **`rlt` (Rust Load Tool)** to **Google Cloud Run** for high-throughput, low-latency, zero-GC streaming JSON ingestion directly into **Google Cloud Storage (GCS)** and queryable via **BigQuery**.

---

## 📐 Architecture Overview

```text
 ┌────────────────────────────────────────────────────────┐
 │   PRODUCERS (Webhooks, Mobile Apps, Microservices)     │
 └──────────────────────────┬─────────────────────────────┘
                            │ HTTPS POST /ingest
                            ▼
 ┌────────────────────────────────────────────────────────┐
 │            GOOGLE CLOUD RUN (rlt container)            │
 │                                                        │
 │  • Scale-to-zero enabled (<50ms cold starts)           │
 │  • Concurrency: 80-250 concurrent HTTP requests/inst   │
 │  • Memory: 512 MiB (ultra-lean vs 2-4 GiB for Python) │
 │  • Port: 8080                                          │
 └──────────────────────────┬─────────────────────────────┘
                            │ Streaming Multipart Upload
                            ▼
 ┌────────────────────────────────────────────────────────┐
 │              GOOGLE CLOUD STORAGE BUCKET               │
 │                                                        │
 │  gs://${BUCKET_NAME}/events/                           │
 │       └── year=YYYY/month=MM/day=DD/data_${UUID}.parquet
 └──────────────────────────┬─────────────────────────────┘
                            │ Directly Queryable
                            ▼
 ┌────────────────────────────────────────────────────────┐
 │            BIGQUERY / dbt EXTERNAL TABLE               │
 │                                                        │
 │  • Auto-Hive partitioning (year, month, day)           │
 │  • Dynamic schema support                              │
 └────────────────────────────────────────────────────────┘
```

---

## 🛠️ Prerequisites

1. Google Cloud SDK (`gcloud`) installed and logged in:
   ```bash
   gcloud auth login
   ```
2. Set your environment variables:
   ```bash
   export PROJECT_ID="your-gcp-project-id"
   export REGION="europe-west9"            # e.g., us-central1, europe-west1
   export BUCKET_NAME="${PROJECT_ID}-rlt-data"
   export REPO_NAME="rlt-docker-repo"
   export SERVICE_NAME="rlt-ingest-service"
   export SA_NAME="rlt-cloudrun-sa"

   gcloud config set project ${PROJECT_ID}
   ```
3. Enable required GCP APIs:
   ```bash
   gcloud services enable \
     run.googleapis.com \
     artifactregistry.googleapis.com \
     cloudbuild.googleapis.com \
     storage.googleapis.com \
     bigquery.googleapis.com
   ```

---

## 🔐 Step 1: Service Account & Least-Privilege IAM

Create a dedicated Google Service Account for Cloud Run with write-only access to the target GCS bucket:

```bash
# 1. Create the service account
gcloud iam service-accounts create ${SA_NAME} \
  --display-name="rlt Cloud Run Ingestion Service Account"

export SA_EMAIL="${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com"

# 2. Create the GCS destination bucket
gcloud storage buckets create gs://${BUCKET_NAME} \
  --location=${REGION} \
  --uniform-bucket-level-access

# 3. Grant Storage Object Creator permission (write only, least privilege)
gcloud storage buckets add-iam-policy-binding gs://${BUCKET_NAME} \
  --member="serviceAccount:${SA_EMAIL}" \
  --role="roles/storage.objectCreator"
```

---

## 📦 Step 2: Build & Push Container to Artifact Registry

### 1. Create an Artifact Registry repository:
```bash
gcloud artifacts repositories create ${REPO_NAME} \
  --repository-format=docker \
  --location=${REGION} \
  --description="Docker repository for rlt ingestion engine"
```

### 2. Build and submit using Google Cloud Build (no local Docker required):
```bash
export IMAGE_URI="${REGION}-docker.pkg.dev/${PROJECT_ID}/${REPO_NAME}/rlt:latest"

gcloud builds submit --tag ${IMAGE_URI} .
```

*The multi-stage `Dockerfile` uses `rust:1.85-slim-bookworm` to build an optimized release binary (with LTO and symbol stripping), then packages it onto `gcr.io/distroless/cc-debian12:nonroot` for a total image size of only ~35 MB.*

---

## 🚀 Step 3: Deploy to Cloud Run

Deploy `rlt` to Cloud Run with environment variables configured for GCS upload and automatic batching:

```bash
gcloud run deploy ${SERVICE_NAME} \
  --image=${IMAGE_URI} \
  --region=${REGION} \
  --platform=managed \
  --service-account=${SA_EMAIL} \
  --port=8080 \
  --memory=512Mi \
  --cpu=1 \
  --concurrency=80 \
  --min-instances=0 \
  --max-instances=50 \
  --timeout=300 \
  --set-env-vars="DESTINATION=gs://${BUCKET_NAME}/events,BATCH_SIZE_MB=32,FLUSH_INTERVAL_SECS=30,COMPRESSION=zstd,RUST_LOG=info" \
  --allow-unauthenticated
```

> [!TIP]
> **Why 512 MiB RAM is enough:**
> Unlike Python/JVM runtimes that experience memory bloat and GC pauses under load, `rlt` uses in-memory micro-batch buffers with deterministic memory tracking. 512 MiB is more than sufficient for 32–64 MB micro-batching.

### Retrieve Service URL:
```bash
export SERVICE_URL=$(gcloud run services describe ${SERVICE_NAME} --region=${REGION} --format="value(status.url)")
echo "rlt endpoint: ${SERVICE_URL}"
```

---

## 🧪 Step 4: Validate Ingestion

### 1. Health Check
```bash
curl -i "${SERVICE_URL}/health"
# Returns: HTTP/2 200 - ok
```

### 2. Ingest Sample JSON Webhook
```bash
curl -X POST "${SERVICE_URL}/ingest" \
  -H "Content-Type: application/json" \
  -d '{
    "event_id": "evt_99182",
    "timestamp": "2026-10-31T20:00:00Z",
    "user": {
      "id": 42,
      "email": "alice@example.com"
    },
    "cart": {
      "items_count": 3,
      "total_amount": 149.99
    }
  }'
```

### 3. Verify files in GCS:
```bash
gcloud storage ls --recursive "gs://${BUCKET_NAME}/events/"
```
Output:
```text
gs://${BUCKET_NAME}/events/year=2026/month=10/day=31/data_a7f9c2d1-....parquet
```

---

## 📊 Step 5: Query from BigQuery (External Table)

Create a BigQuery external table partitioned on the Hive date layout (`year`, `month`, `day`):

```sql
CREATE OR REPLACE EXTERNAL TABLE `your_project.analytics.rlt_events_raw`
WITH PARTITION COLUMNS (
    year INT64,
    month INT64,
    day INT64
)
OPTIONS (
    format = 'PARQUET',
    uris = ['gs://your-gcp-project-id-rlt-data/events/*'],
    hive_partition_uri_prefix = 'gs://your-gcp-project-id-rlt-data/events/',
    require_hive_partition_filter = false
);
```

### Querying your raw data:
```sql
SELECT
    user__id,
    user__email,
    cart__total_amount,
    year,
    month,
    day
FROM
    `your_project.analytics.rlt_events_raw`
WHERE
    year = 2026
    AND month = 10
LIMIT 100;
```

---

## 📈 Step 6: Observability & Production Monitoring

### 1. Viewing Ingestion Logs:
Filter logs in Google Cloud Logging:
```text
resource.type="cloud_run_revision"
resource.labels.service_name="rlt-ingest-service"
jsonPayload.message=~"flushed batch to parquet"
```

### 2. Dead-Letter Queue (DLQ) Alerts:
Set up a metric-based alert on unparseable/corrupted payload warnings:
```text
resource.type="cloud_run_revision"
textPayload=~"DLQ"
```

---

## 🛡️ Production Tuning Reference

| Configuration | Recommended Value | Description |
|---|---|---|
| `BATCH_SIZE_MB` | `32` to `64` | Target Parquet chunk size before flushing |
| `FLUSH_INTERVAL_SECS` | `30` to `60` | Max latency before writing in-flight events to GCS |
| `COMPRESSION` | `zstd` | Level 3 ZSTD provides ~4x compression ratio with near-instant decompression |
| `--concurrency` | `80` to `150` | Maximum simultaneous HTTP requests per Cloud Run container |
| `--min-instances` | `0` (or `1` for 0ms SLA) | Set to `0` for scale-to-zero cost savings; `<50ms` cold start is unnoticeable |
