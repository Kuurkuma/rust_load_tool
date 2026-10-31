# ⚡ Cloud Run Ingestion Benchmark: `rlt` vs. `ingestr` vs. `dlt`

An automated, reproducible, serverless benchmark harness testing three real-world streaming JSON ingestion engines under dynamic **Schema Drift**:

| Competitor | Language & Stack | Design Philosophy |
|---|---|---|
| 🦀 **`rlt`** | Rust (Axum, `arrow-rs`, `parquet`, `object_store`) | Zero-GC, native columnar Arrow batching, progressive monotonic type widening |
| 🐹 **`ingestr`** | Go (Go net/http, `parquet-go`) | Compiled static binary, goroutine concurrency, Go runtime GC |
| 🐍 **`dlt`** | Python 3.11 (FastAPI, Uvicorn, `dlt[gcp]`, `pyarrow`) | Python data engineering ecosystem, dynamic schema inference |

---

## 🎯 Benchmark Objectives & Parity Safeguards

Each service is deployed onto **Google Cloud Run v2** with **strictly identical resource constraints**:

- **vCPU:** `1.0`
- **Memory:** `512 MiB`
- **Concurrency:** `80` requests per instance
- **Min instances:** `0` (enforces scale-to-zero cold-start measurement)
- **Max instances:** `20`
- **Execution Environment:** `EXECUTION_ENVIRONMENT_GEN2`
- **Destination:** Google Cloud Storage bucket (`gs://${PROJECT}-rlt-bench-v1/`) writing partitioned Parquet files

---

## 🔬 Four-Stage Schema Drift Protocol

The asynchronous load generator (`run_benchmark.py`) subjects each microservice to 4 progressive schema drift stages:

1. **Stage 1 (Baseline - 25%):** Stable schema: `{id: int, user: str, score: int}`
2. **Stage 2 (Column Drift - 25%):** New columns appear: `+ country: str, + signup_date: str`
3. **Stage 3 (Type Drift - 25%):** Widening: `id` changes from `int` to `string` (`"usr_widened_5021"`), and `score` changes to `float` (`98.75`)
4. **Stage 4 (Structural Drift - 25%):** Deep nesting: `+ meta: {device: str, os: str}, tags: [str]`

---

## 🚀 Running the Benchmark (One-Click)

### 1. Prerequisites
- Google Cloud SDK (`gcloud auth login`)
- Terraform >= 1.5
- Docker
- Python 3.9+ with `aiohttp`

### 2. Execute Benchmark
```bash
cd benchmarks
./run_all.sh --project your-gcp-project-id --region europe-west1 --requests 1000 --concurrency 20
```

The script will:
1. Build and push container images to Google Artifact Registry.
2. Provision Cloud Run services and a GCS bucket with Terraform.
3. Allow services to scale to 0 to capture exact cold-start latency.
4. Run the 4-stage Schema Drift load test via `orchestrator.py`.
5. Output live terminal scorecards and save the final report to `results/latest_report.md`.

### 3. Cleanup & Teardown
```bash
./run_all.sh --project your-gcp-project-id --destroy
```

---

## 📁 Repository Structure

```
benchmarks/
├── README.md                           # This documentation
├── run_all.sh                          # End-to-end automation CLI script
├── scripts/
│   ├── requirements.txt                # aiohttp, rich, google-cloud-monitoring
│   ├── run_benchmark.py                # Async HTTP load generator
│   └── orchestrator.py                 # Multi-engine orchestrator & Markdown generator
├── competitors/
│   ├── dlt-python/                     # dlt (FastAPI + dlt filesystem-parquet)
│   ├── ingestr-go/                     # ingestr (Go + parquet-go)
│   └── rlt-rust/                       # rlt (Rust + Arrow + Parquet)
├── terraform/                          # Infrastructure as Code (Cloud Run v2 + GCS + IAM)
└── results/
    └── latest_report.md                # Output comparative report
```
