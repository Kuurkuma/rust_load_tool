# ==============================================================================
# dlt (Python) Ingestion Service Dockerfile
# Base: python:3.11-slim with Astral's uv package manager
# ==============================================================================
FROM python:3.11-slim

WORKDIR /app

# Install uv directly from Astral's official image
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PORT=8080 \
    UV_SYSTEM_PYTHON=1

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

COPY requirements.txt .
# Fast, cached dependency installation using uv
RUN uv pip install --no-cache -r requirements.txt

COPY main.py .

# Run as non-root user
RUN useradd -m -u 1000 appuser && chown -R appuser:appuser /app
USER appuser

EXPOSE 8080

CMD ["python", "main.py"]
