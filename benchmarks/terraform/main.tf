terraform {
  required_version = ">= 1.5"
  required_providers {
    google = {
      source  = "hashicorp/google"
      version = ">= 5.0"
    }
  }
}

provider "google" {
  project = var.project_id
  region  = var.region
}

# ------------------------------------------------------------------------------
# 0. Enable Required GCP APIs
# ------------------------------------------------------------------------------
resource "google_project_service" "gcp_services" {
  for_each = toset(var.gcp_services)

  project            = var.project_id
  service            = each.key
  disable_on_destroy = false
}

# ------------------------------------------------------------------------------
# 1. Artifact Registry Docker Repository
# ------------------------------------------------------------------------------
resource "google_artifact_registry_repository" "docker_repo" {
  repository_id = var.artifact_repository_id
  location      = var.region
  format        = "DOCKER"
  description   = "Docker repository for ingestion benchmark competitors"

  depends_on = [google_project_service.gcp_services]
}

# ------------------------------------------------------------------------------
# 2. GCS Bucket for Parquet Outputs
# ------------------------------------------------------------------------------
resource "google_storage_bucket" "benchmark_bucket" {
  name                        = "${var.project_id}-rlt-bench-${var.benchmark_id}"
  location                    = var.region
  force_destroy               = true
  uniform_bucket_level_access = true

  lifecycle_rule {
    condition {
      age = 1 # Auto delete test parquet files after 1 day
    }
    action {
      type = "Delete"
    }
  }

  depends_on = [google_project_service.gcp_services]
}

# ------------------------------------------------------------------------------
# 2. Service Account & IAM Permissions
# ------------------------------------------------------------------------------
resource "google_service_account" "bench_sa" {
  account_id   = "rlt-bench-sa-${var.benchmark_id}"
  display_name = "Cloud Run Benchmark Ingestion SA"

  depends_on = [google_project_service.gcp_services]
}

resource "google_storage_bucket_iam_member" "bench_storage_admin" {
  bucket = google_storage_bucket.benchmark_bucket.name
  role   = "roles/storage.objectAdmin"
  member = "serviceAccount:${google_service_account.bench_sa.email}"
}

# ------------------------------------------------------------------------------
# 4. Cloud Run Service: rlt (Rust)
# ------------------------------------------------------------------------------
resource "google_cloud_run_v2_service" "rlt" {
  name     = "rlt-bench-rlt-${var.benchmark_id}"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  depends_on = [
    google_project_service.gcp_services,
    google_artifact_registry_repository.docker_repo,
  ]

  template {
    execution_environment            = "EXECUTION_ENVIRONMENT_GEN2"
    service_account                  = google_service_account.bench_sa.email
    max_instance_request_concurrency = var.concurrency

    scaling {
      min_instance_count = 0
      max_instance_count = 20
    }

    containers {
      image = var.rlt_image

      resources {
        limits = {
          cpu    = var.cpu_limit
          memory = var.memory_limit
        }
        cpu_idle = true
      }

      ports {
        container_port = 8080
      }

      env {
        name  = "DESTINATION"
        value = "gs://${google_storage_bucket.benchmark_bucket.name}/rlt"
      }
      env {
        name  = "BATCH_SIZE_MB"
        value = "32"
      }
      env {
        name  = "FLUSH_INTERVAL_SECS"
        value = "30"
      }
      env {
        name  = "COMPRESSION"
        value = "zstd"
      }
      env {
        name  = "RUST_LOG"
        value = "info"
      }
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "rlt_public" {
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.rlt.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}

# ------------------------------------------------------------------------------
# 5. Cloud Run Service: ingestr (Go)
# ------------------------------------------------------------------------------
resource "google_cloud_run_v2_service" "ingestr" {
  name     = "rlt-bench-ingestr-${var.benchmark_id}"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  depends_on = [
    google_project_service.gcp_services,
    google_artifact_registry_repository.docker_repo,
  ]

  template {
    execution_environment            = "EXECUTION_ENVIRONMENT_GEN2"
    service_account                  = google_service_account.bench_sa.email
    max_instance_request_concurrency = var.concurrency

    scaling {
      min_instance_count = 0
      max_instance_count = 20
    }

    containers {
      image = var.ingestr_image

      resources {
        limits = {
          cpu    = var.cpu_limit
          memory = var.memory_limit
        }
        cpu_idle = true
      }

      ports {
        container_port = 8080
      }

      env {
        name  = "DESTINATION"
        value = "gs://${google_storage_bucket.benchmark_bucket.name}/ingestr"
      }
      env {
        name  = "BATCH_SIZE_RECORDS"
        value = "500"
      }
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "ingestr_public" {
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.ingestr.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}

# ------------------------------------------------------------------------------
# 6. Cloud Run Service: dlt (Python)
# ------------------------------------------------------------------------------
resource "google_cloud_run_v2_service" "dlt" {
  name     = "rlt-bench-dlt-${var.benchmark_id}"
  location = var.region
  ingress  = "INGRESS_TRAFFIC_ALL"

  depends_on = [
    google_project_service.gcp_services,
    google_artifact_registry_repository.docker_repo,
  ]

  template {
    execution_environment            = "EXECUTION_ENVIRONMENT_GEN2"
    service_account                  = google_service_account.bench_sa.email
    max_instance_request_concurrency = var.concurrency

    scaling {
      min_instance_count = 0
      max_instance_count = 20
    }

    containers {
      image = var.dlt_image

      resources {
        limits = {
          cpu    = var.cpu_limit
          memory = var.memory_limit
        }
        cpu_idle = true
      }

      ports {
        container_port = 8080
      }

      env {
        name  = "DESTINATION"
        value = "gs://${google_storage_bucket.benchmark_bucket.name}/dlt"
      }
      env {
        name  = "BATCH_SIZE_RECORDS"
        value = "500"
      }
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "dlt_public" {
  project  = var.project_id
  location = var.region
  name     = google_cloud_run_v2_service.dlt.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}
