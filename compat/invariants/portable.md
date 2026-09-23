# Portable invariants (M0)

Stable machine-readable IDs. Implementations may use idiomatic Rust names;
serialized identity must match these IDs or clearly preserve their meaning.

| ID | Meaning | M0 evidence |
| --- | --- | --- |
| `BOUNDED_WAIT_NO_HANG` | Every scenario reaches a terminal outcome within the declared maximum wall-clock bound (`deadline + CLEANUP_BOUND + IO_COMPLETION_GRACE + tolerance`). | Actual `elapsed_ms` ≤ `declared_max_wall_ms`; terminal exit or timeout. |
| `EXIT_CAUSE_PRESERVED` | Fixture-chosen ordinary exit code remains that exact code. | `ExitCause::Code` equality with Strong OS wait evidence. |
| `DRAIN_BOTH_STREAMS_NO_DEADLOCK` | Harness drains stdout and stderr independently without deadlock. | Both stream totals recorded; run completes without timeout. EOF may be false if drain was bounded out — not claimed as complete. |
| `DESCRIPTOR_CLOSURE_EARLY_EXIT` | Child exit while harness still owns stream handles does not hang the runner. | Exit observed; runner returns within declared bound. |
| `ZERO_UNSCRIPTED_INPUT_WRITES` | `StdinSpec::Closed` writes no hidden input; fixture sees EOF and 0 bytes. | Fixture `stdin-report` JSON: `stdin_eof=true`, `bytes_read=0`. |
| `ENV_RECORDED` | Dedicated fixture-observed probe matched; no secret dump in durable evidence. | In-memory compare of probe value; durable `EnvApplied { key }` only. |

POSIX job-control / TTY invariants (M0-D/G) live in
[`posix.md`](posix.md). Portable IDs above remain M0-A/B/C evidence.

Stream truth distinguishes `truncated`, `eof_observed`, and `drain_timed_out`.
Root-process cleanup is never labelled as descendant containment.

Judgment lives in `omen-compat`, not in fixtures. Fixtures report facts only.
