#!/usr/bin/env python3
"""
dlt-python Ingestion Service.

Exposes:
- GET /health -> "ok"
- POST /ingest -> Accepts JSON payload (object or array), normalizes using dlt, writes Parquet.
"""

import asyncio
import os
import sys
from typing import Any, Dict, List, Union

from fastapi import FastAPI, HTTPException, Request
from fastapi.responses import JSONResponse
import uvicorn

app = FastAPI(title="dlt Ingestion Service", version="1.0.0")

# Destination path / bucket
DESTINATION = os.getenv("DESTINATION", "/tmp/dlt_output")
PORT = int(os.getenv("PORT", "8080"))
BATCH_SIZE = int(os.getenv("BATCH_SIZE_RECORDS", "500"))

buffer: List[Dict[str, Any]] = []
buffer_lock = asyncio.Lock()


def flush_dlt_pipeline(records: List[Dict[str, Any]]):
    """
    Run dlt pipeline to write Parquet files to destination.
    """
    try:
        import dlt
        pipeline = dlt.pipeline(
            pipeline_name="dlt_bench_pipeline",
            destination="filesystem",
            dataset_name="events",
        )
        pipeline.run(
            records,
            table_name="events",
            loader_file_format="parquet",
        )
    except Exception as e:
        print(f"[!] dlt pipeline run warning: {e}", file=sys.stderr)


@app.get("/health")
async def health():
    return "ok"


@app.post("/ingest")
async def ingest(request: Request):
    try:
        body = await request.json()
    except Exception as e:
        raise HTTPException(status_code=400, detail=f"Invalid JSON: {e}")

    records: List[Dict[str, Any]]
    if isinstance(body, list):
        records = body
    elif isinstance(body, dict):
        records = [body]
    else:
        raise HTTPException(status_code=400, detail="Expected JSON object or array")

    to_flush = []
    async with buffer_lock:
        buffer.extend(records)
        if len(buffer) >= BATCH_SIZE:
            to_flush = list(buffer)
            buffer.clear()

    if to_flush:
        # Offload sync dlt pipeline run to threadpool
        asyncio.get_event_loop().run_in_executor(None, flush_dlt_pipeline, to_flush)

    return JSONResponse(content={"ingested": len(records), "dlq": 0})


@app.on_event("shutdown")
async def shutdown():
    async with buffer_lock:
        if buffer:
            flush_dlt_pipeline(list(buffer))
            buffer.clear()


if __name__ == "__main__":
    uvicorn.run("main:app", host="0.0.0.0", port=PORT, log_level="warning")
