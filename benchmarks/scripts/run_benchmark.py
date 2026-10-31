#!/usr/bin/env python3
"""
run_benchmark.py - High-Performance Async Load Generator with Schema Drift.

Measures:
- Exact Cold-Start Latency (initial request against 0 instances).
- Warm Ingestion Latency (p50, p90, p95, p99, min, max, avg).
- Effective Throughput (RPS).
- Error Rate under 4-stage Schema Drift.
"""

import argparse
import asyncio
import json
import math
import sys
import time
from typing import Any, Dict, List, Tuple


def generate_drift_payload(index: int, total: int, batch_size: int = 1) -> List[Dict[str, Any]]:
    """
    Generate a batch of JSON records reflecting one of 4 progressive Schema Drift stages.

    Stage 1 (0.00 - 0.25): Baseline stable schema {id: int, user: str, score: int}
    Stage 2 (0.25 - 0.50): Column addition (+ country, + signup_date)
    Stage 3 (0.50 - 0.75): Type widening (id -> str, score -> float)
    Stage 4 (0.75 - 1.00): Deep nesting (+ meta: {device, os}, tags: [str])
    """
    ratio = index / max(total, 1)
    batch = []

    for b in range(batch_size):
        item_id = index * batch_size + b

        if ratio < 0.25:
            # Stage 1: Stable baseline
            record = {
                "id": item_id,
                "user": f"user_{item_id}",
                "score": 100,
                "timestamp": "2026-10-31T20:00:00Z",
            }
        elif ratio < 0.50:
            # Stage 2: New columns
            record = {
                "id": item_id,
                "user": f"user_{item_id}",
                "score": 100,
                "country": "FR" if item_id % 2 == 0 else "DE",
                "signup_date": "2026-01-15",
                "timestamp": "2026-10-31T20:00:00Z",
            }
        elif ratio < 0.75:
            # Stage 3: Type widening (id: int -> string, score: int -> float)
            record = {
                "id": f"usr_widened_{item_id}",
                "user": f"user_{item_id}",
                "score": 98.75 + (item_id % 10) * 0.1,
                "country": "FR",
                "timestamp": "2026-10-31T20:00:00Z",
            }
        else:
            # Stage 4: Structural nesting & arrays
            record = {
                "id": f"usr_nested_{item_id}",
                "user": f"user_{item_id}",
                "score": 85.5,
                "meta": {
                    "device": "mobile" if item_id % 2 == 0 else "desktop",
                    "os": "macOS" if item_id % 3 == 0 else "linux",
                    "nested_version": 2,
                },
                "tags": ["cloud", "serverless", "benchmark"],
                "active": True,
                "timestamp": "2026-10-31T20:00:00Z",
            }

        batch.append(record)

    return batch


async def measure_cold_start(session: Any, base_url: str) -> Tuple[float, int, str]:
    """
    Send the very first probe request to measure cold-start latency.
    """
    import aiohttp
    ingest_url = f"{base_url.rstrip('/')}/ingest"
    probe_payload = generate_drift_payload(index=0, total=100, batch_size=1)

    start = time.perf_counter()
    try:
        async with session.post(
            ingest_url,
            json=probe_payload,
            headers={"Content-Type": "application/json"},
            timeout=aiohttp.ClientTimeout(total=60.0),
        ) as resp:
            elapsed = (time.perf_counter() - start) * 1000.0  # ms
            body = await resp.text()
            return elapsed, resp.status, body
    except Exception as e:
        elapsed = (time.perf_counter() - start) * 1000.0
        return elapsed, 0, str(e)


async def worker(
    queue: asyncio.Queue,
    session: Any,
    ingest_url: str,
    latencies: List[float],
    errors: List[str],
    status_counts: Dict[int, int],
):
    import aiohttp
    while True:
        try:
            payload = await queue.get()
        except asyncio.CancelledError:
            break

        start = time.perf_counter()
        try:
            async with session.post(
                ingest_url,
                json=payload,
                headers={"Content-Type": "application/json"},
                timeout=aiohttp.ClientTimeout(total=30.0),
            ) as resp:
                elapsed_ms = (time.perf_counter() - start) * 1000.0
                await resp.read()
                latencies.append(elapsed_ms)
                status_counts[resp.status] = status_counts.get(resp.status, 0) + 1

                if resp.status >= 400:
                    errors.append(f"HTTP {resp.status}")
        except Exception as e:
            elapsed_ms = (time.perf_counter() - start) * 1000.0
            latencies.append(elapsed_ms)
            errors.append(str(e))
            status_counts[0] = status_counts.get(0, 0) + 1
        finally:
            queue.task_done()


def calculate_percentiles(data: List[float]) -> Dict[str, float]:
    if not data:
        return {"p50": 0.0, "p90": 0.0, "p95": 0.0, "p99": 0.0, "min": 0.0, "max": 0.0, "avg": 0.0}

    s = sorted(data)
    n = len(s)

    def p(pct: float) -> float:
        k = (n - 1) * (pct / 100.0)
        f = math.floor(k)
        c = math.ceil(k)
        if f == c:
            return s[int(k)]
        return s[f] * (c - k) + s[c] * (k - f)

    return {
        "p50": round(p(50), 2),
        "p90": round(p(90), 2),
        "p95": round(p(95), 2),
        "p99": round(p(99), 2),
        "min": round(s[0], 2),
        "max": round(s[-1], 2),
        "avg": round(sum(s) / n, 2),
    }


async def run_benchmark(
    url: str,
    total_requests: int = 1000,
    concurrency: int = 20,
    batch_size: int = 1,
    measure_cold: bool = True,
) -> Dict[str, Any]:
    try:
        import aiohttp
    except ImportError:
        print("Error: aiohttp is required. Install via: pip install -r requirements.txt", file=sys.stderr)
        sys.exit(1)

    connector = aiohttp.TCPConnector(limit=concurrency + 10, ttl_dns_cache=300)
    ingest_url = f"{url.rstrip('/')}/ingest"

    async with aiohttp.ClientSession(connector=connector) as session:
        cold_start_ms = 0.0
        cold_status = 200

        if measure_cold:
            print(f"[*] Probing cold-start on {ingest_url} ...")
            cold_start_ms, cold_status, body = await measure_cold_start(session, url)
            print(f"[*] Cold-start latency: {cold_start_ms:.2f} ms (HTTP {cold_status})")

        print(f"[*] Executing load test: {total_requests} requests, concurrency={concurrency}, batch_size={batch_size}")
        queue: asyncio.Queue = asyncio.Queue()

        for i in range(total_requests):
            payload = generate_drift_payload(index=i, total=total_requests, batch_size=batch_size)
            queue.put_nowait(payload)

        latencies: List[float] = []
        errors: List[str] = []
        status_counts: Dict[int, int] = {}

        workers = [
            asyncio.create_task(worker(queue, session, ingest_url, latencies, errors, status_counts))
            for _ in range(concurrency)
        ]

        t_start = time.perf_counter()
        await queue.join()
        total_duration = time.perf_counter() - t_start

        for w in workers:
            w.cancel()
        await asyncio.gather(*workers, return_exceptions=True)

    percentiles = calculate_percentiles(latencies)
    successful = sum(count for status, count in status_counts.items() if 200 <= status < 300)
    failed = total_requests - successful
    error_rate_pct = round((failed / max(total_requests, 1)) * 100.0, 2)
    effective_rps = round(total_requests / max(total_duration, 0.001), 2)
    records_per_sec = round((total_requests * batch_size) / max(total_duration, 0.001), 2)

    results = {
        "target_url": url,
        "cold_start_ms": round(cold_start_ms, 2),
        "cold_start_status": cold_status,
        "total_requests": total_requests,
        "total_records": total_requests * batch_size,
        "concurrency": concurrency,
        "duration_seconds": round(total_duration, 3),
        "effective_rps": effective_rps,
        "records_per_second": records_per_sec,
        "successful_requests": successful,
        "failed_requests": failed,
        "error_rate_percent": error_rate_pct,
        "status_distribution": status_counts,
        "latency_percentiles_ms": percentiles,
    }

    return results


def main():
    parser = argparse.ArgumentParser(description="rlt Ingestion Benchmark Load Generator")
    parser.add_argument("--url", required=True, help="Cloud Run service base URL (e.g. https://service.run.app)")
    parser.add_argument("--total-requests", type=int, default=1000, help="Total number of HTTP POST requests")
    parser.add_argument("--concurrency", type=int, default=20, help="Concurrent workers")
    parser.add_argument("--batch-size", type=int, default=1, help="JSON objects per HTTP request")
    parser.add_argument("--no-cold-start", action="store_true", help="Skip cold-start measurement probe")
    parser.add_argument("--output", help="Write JSON summary to file")

    args = parser.parse_args()

    results = asyncio.run(
        run_benchmark(
            url=args.url,
            total_requests=args.total_requests,
            concurrency=args.concurrency,
            batch_size=args.batch_size,
            measure_cold=not args.no_cold_start,
        )
    )

    formatted = json.dumps(results, indent=2)
    print("\n=== Benchmark Results ===")
    print(formatted)

    if args.output:
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(formatted)
        print(f"\n[+] Results saved to {args.output}")


if __name__ == "__main__":
    main()
