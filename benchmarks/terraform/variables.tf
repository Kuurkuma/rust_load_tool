variable "project_id" {
  description = "The Google Cloud Platform project ID."
  type        = string
}

variable "region" {
  description = "GCP region to deploy Cloud Run services and storage bucket."
  type        = string
  default     = "europe-west1"
}

variable "benchmark_id" {
  description = "Unique benchmark identifier suffix for resource naming."
  type        = string
  default     = "v1"
}

variable "rlt_image" {
  description = "Docker image URI for rlt (Rust)."
  type        = string
}

variable "ingestr_image" {
  description = "Docker image URI for ingestr (Go)."
  type        = string
}

variable "dlt_image" {
  description = "Docker image URI for dlt (Python)."
  type        = string
}

variable "cpu_limit" {
  description = "CPU allocated to each Cloud Run container instance (strict parity)."
  type        = string
  default     = "1"
}

variable "memory_limit" {
  description = "RAM allocated to each Cloud Run container instance (strict parity)."
  type        = string
  default     = "512Mi"
}

variable "concurrency" {
  description = "Maximum concurrent HTTP requests per Cloud Run container instance."
  type        = number
  default     = 80
}

variable "artifact_repository_id" {
  description = "Name of the Artifact Registry repository."
  type        = string
  default     = "rlt-docker-repo"
}

variable "gcp_services" {
  description = "List of GCP APIs to enable for benchmark deployment."
  type        = list(string)
  default = [
    "run.googleapis.com",
    "storage.googleapis.com",
    "iam.googleapis.com",
    "cloudresourcemanager.googleapis.com",
    "artifactregistry.googleapis.com",
    "cloudbuild.googleapis.com",
  ]
}
