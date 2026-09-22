# omen-shell performance sidecar (local measurement)

Measured, not optimized. Question answered: what tax does
omen-shell add over raw Omen?

```json
{
  "cold_connect_ms": {
    "n": 5.0,
    "p50_ms": 24.650699997437187,
    "p95_ms": 27.40040000207955,
    "mean_ms": 25.154320000729058
  },
  "warm_orient_ms": {
    "n": 20.0,
    "p50_ms": 0.8098000034806319,
    "p95_ms": 1.047999998263549,
    "mean_ms": 0.8311699999467237
  },
  "sequential_100_context_ms": 50.27749999862863,
  "concurrent_20_context_ms": 5.575399998633657,
  "single_execute_ms": 125.2189000006183,
  "shutdown_ms": 5.009500004234724,
  "memory_mb": {
    "before": 28.23046875,
    "during": 30.18359375,
    "after": 30.37109375
  },
  "omen_processes": {
    "before": 0,
    "during": 1,
    "after": 0
  }
}
```

Observations:

- One connected client holds exactly one `omen mcp` process (during=1, after-close=0).
- Warm SDK calls reuse the connection; raw `omen mcp` probes spawn a process per call (see Bat 15 evidence).
- Shutdown is bounded and leak-free: no `omen mcp` remains after close.
