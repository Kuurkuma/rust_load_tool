output "artifact_registry_repository_name" {
  description = "Name of the Artifact Registry repository."
  value       = google_artifact_registry_repository.docker_repo.name
}

output "benchmark_bucket_name" {
  description = "Name of the GCS bucket for benchmark outputs."
  value       = google_storage_bucket.benchmark_bucket.name
}

output "service_account_email" {
  description = "Email of the benchmark service account."
  value       = google_service_account.bench_sa.email
}

output "rlt_service_url" {
  description = "Public URL for rlt (Rust) Cloud Run service."
  value       = google_cloud_run_v2_service.rlt.uri
}

output "ingestr_service_url" {
  description = "Public URL for ingestr (Go) Cloud Run service."
  value       = google_cloud_run_v2_service.ingestr.uri
}

output "dlt_service_url" {
  description = "Public URL for dlt (Python) Cloud Run service."
  value       = google_cloud_run_v2_service.dlt.uri
}
