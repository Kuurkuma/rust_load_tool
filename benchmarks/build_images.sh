#!/usr/bin/env bash
# ==============================================================================
# build_images.sh - Build & Push the 3 Competitor Images to Artifact Registry
# Uses Cloud Build (no local Docker needed) or local Docker (with linux/amd64)
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

PROJECT_ID="${GCP_PROJECT_ID:-$(gcloud config get-value project 2>/dev/null || echo "")}"
REGION="${GCP_REGION:-europe-west1}"
REPO_NAME="rlt-docker-repo"
TAG="latest"
USE_CLOUD_BUILD=auto

# Colors
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BOLD='\033[1m'
NC='\033[0m'

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
        --repo)
            REPO_NAME="$2"
            shift 2
            ;;
        -t|--tag)
            TAG="$2"
            shift 2
            ;;
        --cloud-build)
            USE_CLOUD_BUILD=true
            shift
            ;;
        --docker)
            USE_CLOUD_BUILD=false
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

if [[ -z "${PROJECT_ID}" ]]; then
    echo -e "${RED}[ERROR] GCP Project ID is required. Pass --project <ID> or set GCP_PROJECT_ID.${NC}" >&2
    exit 1
fi

echo -e "${BLUE}[INFO]${NC} Project: ${BOLD}${PROJECT_ID}${NC} | Region: ${BOLD}${REGION}${NC} | Repo: ${BOLD}${REPO_NAME}${NC}"

# 1. Ensure required APIs are enabled
echo -e "${BLUE}[INFO]${NC} Ensuring artifactregistry.googleapis.com and cloudbuild.googleapis.com are enabled..."
gcloud services enable artifactregistry.googleapis.com cloudbuild.googleapis.com --project="${PROJECT_ID}" --quiet

# 2. Ensure Artifact Registry repository exists
echo -e "${BLUE}[INFO]${NC} Checking Artifact Registry repository '${REPO_NAME}'..."
gcloud artifacts repositories describe "${REPO_NAME}" \
    --location="${REGION}" \
    --project="${PROJECT_ID}" &>/dev/null || \
gcloud artifacts repositories create "${REPO_NAME}" \
    --repository-format=docker \
    --location="${REGION}" \
    --project="${PROJECT_ID}" \
    --description="Docker repository for ingestion benchmark competitors"

IMAGE_BASE="${REGION}-docker.pkg.dev/${PROJECT_ID}/${REPO_NAME}"
RLT_IMAGE="${IMAGE_BASE}/rlt-rust:${TAG}"
INGESTR_IMAGE="${IMAGE_BASE}/ingestr-go:${TAG}"
DLT_IMAGE="${IMAGE_BASE}/dlt-python:${TAG}"

# Determine whether to use Cloud Build or Docker
if [[ "${USE_CLOUD_BUILD}" == "auto" ]]; then
    if docker info &>/dev/null; then
        USE_CLOUD_BUILD=false
        echo -e "${BLUE}[INFO]${NC} Local Docker daemon detected. Building with local Docker (linux/amd64)..."
    else
        USE_CLOUD_BUILD=true
        echo -e "${YELLOW}[WARN]${NC} Local Docker daemon not available. Falling back to Google Cloud Build (remote)..."
    fi
fi

BENCHMARK_ID="${BENCHMARK_ID:-v1}"
SA_EMAIL="${BENCH_SA:-rlt-bench-sa-${BENCHMARK_ID}@${PROJECT_ID}.iam.gserviceaccount.com}"

if [[ "${USE_CLOUD_BUILD}" == "true" ]]; then
    echo -e "${BLUE}[INFO]${NC} === Building with Google Cloud Build using Service Account: ${BOLD}${SA_EMAIL}${NC} ==="

    echo -e "${BLUE}[INFO]${NC} [1/3] Building rlt (Rust)..."
    gcloud builds submit "${REPO_ROOT}" \
        --service-account="projects/${PROJECT_ID}/serviceAccounts/${SA_EMAIL}" \
        --config=<(cat <<EOF
steps:
- name: 'gcr.io/cloud-builders/docker'
  args: ['build', '-t', '${RLT_IMAGE}', '-f', 'Dockerfile', '.']
images:
- '${RLT_IMAGE}'
EOF
) \
        --project="${PROJECT_ID}"

    echo -e "${BLUE}[INFO]${NC} [2/3] Building ingestr (Go)..."
    gcloud builds submit "${SCRIPT_DIR}/competitors/ingestr-go" \
        --service-account="projects/${PROJECT_ID}/serviceAccounts/${SA_EMAIL}" \
        --tag="${INGESTR_IMAGE}" \
        --project="${PROJECT_ID}"

    echo -e "${BLUE}[INFO]${NC} [3/3] Building dlt (Python)..."
    gcloud builds submit "${SCRIPT_DIR}/competitors/dlt-python" \
        --service-account="projects/${PROJECT_ID}/serviceAccounts/${SA_EMAIL}" \
        --tag="${DLT_IMAGE}" \
        --project="${PROJECT_ID}"
else
    echo -e "${BLUE}[INFO]${NC} === Building with Local Docker (platform linux/amd64) ==="
    gcloud auth configure-docker "${REGION}-docker.pkg.dev" --quiet

    echo -e "${BLUE}[INFO]${NC} [1/3] Building rlt (Rust)..."
    docker buildx build --platform linux/amd64 -t "${RLT_IMAGE}" -f "${REPO_ROOT}/Dockerfile" "${REPO_ROOT}" --push

    echo -e "${BLUE}[INFO]${NC} [2/3] Building ingestr (Go)..."
    docker buildx build --platform linux/amd64 -t "${INGESTR_IMAGE}" -f "${SCRIPT_DIR}/competitors/ingestr-go/Dockerfile" "${SCRIPT_DIR}/competitors/ingestr-go" --push

    echo -e "${BLUE}[INFO]${NC} [3/3] Building dlt (Python)..."
    docker buildx build --platform linux/amd64 -t "${DLT_IMAGE}" -f "${SCRIPT_DIR}/competitors/dlt-python/Dockerfile" "${SCRIPT_DIR}/competitors/dlt-python" --push
fi

echo -e "\n${GREEN}[SUCCESS] All 3 competitor images pushed to Artifact Registry:${NC}"
echo -e "  • ${RLT_IMAGE}"
echo -e "  • ${INGESTR_IMAGE}"
echo -e "  • ${DLT_IMAGE}"
echo -e "\n${BOLD}You can now safely run 'terraform apply' in benchmarks/terraform!${NC}\n"
