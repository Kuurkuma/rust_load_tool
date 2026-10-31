#!/usr/bin/env bash
# ==============================================================================
# run_all.sh - End-to-End Cloud Run Ingestion Benchmark Harness
# Targets: rlt (Rust), ingestr (Go), dlt (Python)
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TF_DIR="${SCRIPT_DIR}/terraform"
SCRIPTS_DIR="${SCRIPT_DIR}/scripts"

# Defaults
PROJECT_ID="${GCP_PROJECT_ID:-$(gcloud config get-value project 2>/dev/null || echo "")}"
REGION="${GCP_REGION:-europe-west1}"
BENCHMARK_ID="${BENCHMARK_ID:-v1}"
REPO_NAME="rlt-benchmarks"
TOTAL_REQUESTS=1000
CONCURRENCY=20
BATCH_SIZE=1
ACTION="run"
SKIP_BUILD=false

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
BOLD='\033[1m'
NC='\033[0m'

log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1" >&2
}

# ------------------------------------------------------------------------------
# Usage Help
# ------------------------------------------------------------------------------
usage() {
    echo -e "${BOLD}Usage:${NC} $0 [options]

${BOLD}Options:${NC}
  -p, --project <PROJECT_ID>     GCP Project ID (default: current gcloud project)
  -r, --region <REGION>          GCP Region (default: europe-west1)
  -n, --requests <COUNT>         Total HTTP requests per competitor (default: 1000)
  -c, --concurrency <COUNT>      Concurrent load generator workers (default: 20)
  -b, --batch-size <COUNT>       Records per HTTP request (default: 1)
  --skip-build                   Skip container build/push and reuse existing images
  --destroy                      Tear down Terraform infrastructure and exit
  -h, --help                     Show this help message

${BOLD}Examples:${NC}
  $0 --project my-gcp-project --region europe-west1
  $0 --project my-gcp-project --destroy"
    exit 0
}

# ------------------------------------------------------------------------------
# Parse Arguments
# ------------------------------------------------------------------------------
while [[ $# -gt 0 ]]; do
    case "$1" in
        -p|--project)
            PROJECT_ID="$2"
            shift 2
            ;;
        -r|--region)
            REGION="$2"
            shift 2
            ;;
        -n|--requests)
            TOTAL_REQUESTS="$2"
            shift 2
            ;;
        -c|--concurrency)
            CONCURRENCY="$2"
            shift 2
            ;;
        -b|--batch-size)
            BATCH_SIZE="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --destroy)
            ACTION="destroy"
            shift
            ;;
        -h|--help)
            usage
            ;;
        *)
            log_error "Unknown option: $1"
            usage
            ;;
    esac
done

if [[ -z "${PROJECT_ID}" ]]; then
    log_error "GCP Project ID is required. Pass --project <ID> or set GCP_PROJECT_ID."
    exit 1
fi

# ------------------------------------------------------------------------------
# 1. Check Prerequisites
# ------------------------------------------------------------------------------
log_info "Verifying prerequisites..."
for cmd in gcloud terraform docker python3; do
    if ! command -v "${cmd}" &>/dev/null; then
        log_error "Required CLI tool '${cmd}' is not installed or not in PATH."
        exit 1
    fi
done

# Check Python dependencies
if ! python3 -c "import aiohttp" &>/dev/null; then
    log_warn "'aiohttp' not detected in current Python environment. Installing dependencies..."
    pip3 install -q -r "${SCRIPTS_DIR}/requirements.txt"
fi

IMAGE_BASE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${REPO_NAME}"
RLT_IMAGE="${IMAGE_BASE}/rlt-rust:${BENCHMARK_ID}"
INGESTR_IMAGE="${IMAGE_BASE}/ingestr-go:${BENCHMARK_ID}"
DLT_IMAGE="${IMAGE_BASE}/dlt-python:${BENCHMARK_ID}"

# ------------------------------------------------------------------------------
# Handle --destroy Action
# ------------------------------------------------------------------------------
if [[ "${ACTION}" == "destroy" ]]; then
    log_warn "Destroying benchmark infrastructure in ${TF_DIR}..."
    cd "${TF_DIR}"
    terraform destroy -auto-approve \
        -var="project_id=${PROJECT_ID}" \
        -var="region=${REGION}" \
        -var="benchmark_id=${BENCHMARK_ID}" \
        -var="rlt_image=${RLT_IMAGE}" \
        -var="ingestr_image=${INGESTR_IMAGE}" \
        -var="dlt_image=${DLT_IMAGE}"
    log_success "Infrastructure successfully destroyed."
    exit 0
fi

# ------------------------------------------------------------------------------
# 2. Build and Push Container Images
# ------------------------------------------------------------------------------
if [[ "${SKIP_BUILD}" == false ]]; then
    log_info "Configuring Artifact Registry repository '${REPO_NAME}'..."
    gcloud artifacts repositories describe "${REPO_NAME}" \
        --location="${REGION}" \
        --project="${PROJECT_ID}" &>/dev/null || \
    gcloud artifacts repositories create "${REPO_NAME}" \
        --repository-format=docker \
        --location="${REGION}" \
        --project="${PROJECT_ID}" \
        --description="Docker repository for ingestion benchmark competitors"

    gcloud auth configure-docker "${REGION}-docker.pkg.dev" --quiet

    log_info "Building and pushing [1/3] rlt (Rust)..."
    docker build -t "${RLT_IMAGE}" -f "${REPO_ROOT}/Dockerfile" "${REPO_ROOT}"
    docker push "${RLT_IMAGE}"

    log_info "Building and pushing [2/3] ingestr (Go)..."
    docker build -t "${INGESTR_IMAGE}" -f "${SCRIPT_DIR}/competitors/ingestr-go/Dockerfile" "${SCRIPT_DIR}/competitors/ingestr-go"
    docker push "${INGESTR_IMAGE}"

    log_info "Building and pushing [3/3] dlt (Python)..."
    docker build -t "${DLT_IMAGE}" -f "${SCRIPT_DIR}/competitors/dlt-python/Dockerfile" "${SCRIPT_DIR}/competitors/dlt-python"
    docker push "${DLT_IMAGE}"

    log_success "All container images successfully pushed to Artifact Registry."
else
    log_info "Skipping container image build (--skip-build specified)."
fi

# ------------------------------------------------------------------------------
# 3. Deploy Infrastructure via Terraform
# ------------------------------------------------------------------------------
log_info "Initializing and applying Terraform infrastructure..."
cd "${TF_DIR}"
terraform init -backend=false

terraform apply -auto-approve \
    -var="project_id=${PROJECT_ID}" \
    -var="region=${REGION}" \
    -var="benchmark_id=${BENCHMARK_ID}" \
    -var="rlt_image=${RLT_IMAGE}" \
    -var="ingestr_image=${INGESTR_IMAGE}" \
    -var="dlt_image=${DLT_IMAGE}"

TF_OUTPUTS_JSON="${SCRIPT_DIR}/results/tf_outputs.json"
mkdir -p "${SCRIPT_DIR}/results"
terraform output -json > "${TF_OUTPUTS_JSON}"

RLT_URL="$(terraform output -raw rlt_service_url)"
INGESTR_URL="$(terraform output -raw ingestr_service_url)"
DLT_URL="$(terraform output -raw dlt_service_url)"
BUCKET_NAME="$(terraform output -raw benchmark_bucket_name)"

log_success "Infrastructure provisioned successfully."
echo -e "  • rlt (Rust):    ${BOLD}${RLT_URL}${NC}"
echo -e "  • ingestr (Go):  ${BOLD}${INGESTR_URL}${NC}"
echo -e "  • dlt (Python):  ${BOLD}${DLT_URL}${NC}"
echo -e "  • GCS Bucket:    ${BOLD}gs://${BUCKET_NAME}${NC}"

# Allow Cloud Run instances to scale down to 0 before cold-start probe
log_info "Allowing Cloud Run services 15 seconds to settle to 0 instances..."
sleep 15

# ------------------------------------------------------------------------------
# 4. Execute Benchmark Suite
# ------------------------------------------------------------------------------
log_info "Executing benchmark orchestrator..."
python3 "${SCRIPTS_DIR}/orchestrator.py" \
    --rlt-url="${RLT_URL}" \
    --ingestr-url="${INGESTR_URL}" \
    --dlt-url="${DLT_URL}" \
    --total-requests="${TOTAL_REQUESTS}" \
    --concurrency="${CONCURRENCY}" \
    --batch-size="${BATCH_SIZE}" \
    --cooling-seconds=15 \
    --project-id="${PROJECT_ID}"

REPORT_PATH="${SCRIPT_DIR}/results/latest_report.md"
log_success "Benchmark finished! Detailed report saved to: ${REPORT_PATH}"

echo -e "\n${BOLD}To clean up all deployed GCP resources, run:${NC}"
echo -e "  $0 --project ${PROJECT_ID} --destroy\n"
