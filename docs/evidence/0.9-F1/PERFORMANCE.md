# 0.9-F1 Performance Evidence

These are **evidence**, not CI latency gates. Structural properties are the
correctness gates; wall-clock numbers are reported from a local run.

## Structural properties (gated)

- zero subprocess calls on the keystroke path (engine is pure Rust over local state)
- zero network calls
- zero model calls
- zero PATH scans on keystrokes (bounded out-of-band cache only)
- no recursive traversal (one capped `read_dir` for path completion)
- bounded candidate count (`MAX_FINAL_CANDIDATES = 24`)
- bounded filesystem entries (`MAX_FS_SCAN = 1024`, `MAX_FS_ENTRIES = 256`)
- bounded PATH cache (`MAX_COMMAND_NAMES = 512`)
- no wait/sleep/retry loops

Gated by: `f1_candidate_generation_is_bounded_per_source`,
`f1_huge_directory_fixture_stays_bounded`,
`f1_completion_structural_bounds_not_wall_clock`,
`test_shell_ux_degraded_mode_when_daemon_offline` (bounded set, not <10ms).

Fragile CI timing asserts (`assert completion < 5ms`, `< 10ms`) were removed
from completion paths and replaced with the structural proofs above.

## Local benchmark (`f1_perf_evidence_report`)

Fixture: 200 files in cwd, 50 files in `src/`, PATH cache of 1000 commands.

Representative query: `cat f0` at end of line (filesystem + ranking).

| Scenario | Result |
|----------|--------|
| cold representative completion | 0.76 ms (24 candidates retained) |
| warm p50 | 0.64 ms |
| warm p95 | 0.91 ms |
| 1,000-candidate ranking (`cmd-`) | 0.56 ms (24 retained) |
| awkward-path completion (`src/m0`) | 0.56 ms |

Second run (reproduced):

| Scenario | Result |
|----------|--------|
| cold | 0.82 ms |
| warm p50 / p95 | 0.63 / 0.77 ms |
| 1,000-candidate ranking | 0.54 ms |
| awkward-path | 0.44 ms |

Well inside the 1 ms keystroke budget claimed by `docs/COMPLETION.md`, but
that budget is not a CI gate.

## Large-directory bounded proof

Fixture: 400 files matching `bulk-*.txt`. Query `cat bulk-`.

- candidates retained: <= `MAX_FS_ENTRIES` (256) and <= `MAX_FINAL_CANDIDATES` (24)
- scan stops at `MAX_FS_SCAN` (1024)
- retained set is sorted then truncated (deterministic ordering of the
  retained set)

## xtask representative index

`cargo xtask preview` completion benchmark uses a representative hot index
(50 facts current+dirty, 20 path commands) and 100 queries.
