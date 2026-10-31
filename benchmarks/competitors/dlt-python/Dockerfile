# ==============================================================================
# dlt (Python) Ingestion Service Dockerfile
# Native uv + pyproject.toml workflow
# ==============================================================================
FROM python:3.11-slim

WORKDIR /app

# Install Astral's uv binary directly
COPY --from=ghcr.io/astral-sh/uv:latest /uv /uvx /bin/

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PORT=8080 \
    UV_COMPILE_BYTECODE=1

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy pyproject.toml and install dependencies natively with uv
COPY pyproject.toml .
RUN uv pip install --system --no-cache -r pyproject.toml

COPY main.py .

# Run as non-root user
RUN useradd -m -u 1000 appuser && chown -R appuser:appuser /app
USER appuser

EXPOSE 8080

CMD ["python", "main.py"]
