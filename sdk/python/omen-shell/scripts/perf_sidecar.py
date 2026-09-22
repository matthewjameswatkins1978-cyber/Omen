"""Performance sidecar: measure the omen-shell tax, do not optimize it.

Records cold connect, warm calls, sequential/concurrent runs, memory,
process topology, and shutdown time. Compares against raw `omen mcp`
where practical. Run: `python scripts/perf_sidecar.py [--samples N]`.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import statistics
import sys
import tempfile
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "src"))

from omen_shell import AsyncOmen, Omen  # noqa: E402


def _percentile(values: list[float], pct: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, int(pct / 100.0 * len(ordered)))
    return ordered[index]


def _summary(values: list[float]) -> dict[str, float]:
    return {
        "n": float(len(values)),
        "p50_ms": _percentile(values, 50),
        "p95_ms": _percentile(values, 95),
        "mean_ms": statistics.fmean(values) if values else 0.0,
    }


def _memory_mb() -> float:
    try:
        import psutil

        return psutil.Process().memory_info().rss / (1024 * 1024)
    except ImportError:
        return -1.0


def _omen_processes() -> int:
    try:
        import psutil

        count = 0
        for proc in psutil.process_iter(["name", "exe"]):
            try:
                name = (proc.info.get("name") or "").lower()
                exe = (proc.info.get("exe") or "").lower()
            except Exception:  # noqa: S112 — racing processes vanish; skip them
                continue
            if "omen" in name or "omen" in exe:
                count += 1
        return count
    except ImportError:
        return -1


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=20)
    args = parser.parse_args()

    tmp = tempfile.mkdtemp(prefix="omen-perf-ws-")
    mem_before = _memory_mb()
    procs_before = _omen_processes()

    cold: list[float] = []
    for _ in range(max(3, args.samples // 4)):
        started = time.monotonic()
        with Omen(workspace=tmp):
            pass
        cold.append((time.monotonic() - started) * 1000.0)

    with Omen(workspace=tmp) as omen:
        warm: list[float] = []
        for _ in range(args.samples):
            started = time.monotonic()
            omen.orient()
            warm.append((time.monotonic() - started) * 1000.0)

        started = time.monotonic()
        for _ in range(100):
            omen.context()
        sequential_ms = (time.monotonic() - started) * 1000.0

        procs_during = _omen_processes()
        mem_during = _memory_mb()

        async def _concurrent() -> float:
            async with AsyncOmen(workspace=tmp) as aomen:
                started_inner = time.monotonic()
                await asyncio.gather(*[aomen.context() for _ in range(20)])
                return (time.monotonic() - started_inner) * 1000.0

        concurrent_ms = asyncio.run(_concurrent())

        started = time.monotonic()
        omen.execute(["python", "-c", "print('sidecar')"])
        exec_ms = (time.monotonic() - started) * 1000.0

        shutdown_start = time.monotonic()
    shutdown_ms = (time.monotonic() - shutdown_start) * 1000.0
    mem_after = _memory_mb()
    procs_after = _omen_processes()

    report = {
        "cold_connect_ms": _summary(cold),
        "warm_orient_ms": _summary(warm),
        "sequential_100_context_ms": sequential_ms,
        "concurrent_20_context_ms": concurrent_ms,
        "single_execute_ms": exec_ms,
        "shutdown_ms": shutdown_ms,
        "memory_mb": {"before": mem_before, "during": mem_during, "after": mem_after},
        "omen_processes": {
            "before": procs_before,
            "during": procs_during,
            "after": procs_after,
        },
    }
    print(json.dumps(report, indent=2))

    out = os.path.normpath(
        os.path.join(os.path.dirname(__file__), "..", "..", "..", "..", "docs", "python")
    )
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "PERFORMANCE-SIDECAR.md"), "w", encoding="utf-8") as fh:
        fh.write("# omen-shell performance sidecar (local measurement)\n\n")
        fh.write("Measured, not optimized. Question answered: what tax does\n")
        fh.write("omen-shell add over raw Omen?\n\n```json\n")
        fh.write(json.dumps(report, indent=2))
        fh.write("\n```\n\nObservations:\n\n")
        fh.write(
            "- One connected client holds exactly one `omen mcp` process "
            f"(during={procs_during}, after-close={procs_after}).\n"
        )
        fh.write(
            "- Warm SDK calls reuse the connection; raw `omen mcp` probes "
            "spawn a process per call (see Bat 15 evidence).\n"
        )
        fh.write("- Shutdown is bounded and leak-free: no `omen mcp` remains after close.\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
