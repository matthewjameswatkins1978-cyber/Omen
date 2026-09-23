# Portable invariants (M0)

Stable machine-readable IDs. Implementations may use idiomatic Rust names;
serialized identity must match these IDs or clearly preserve their meaning.

| ID | Meaning | M0 evidence |
| --- | --- | --- |
| `BOUNDED_WAIT_NO_HANG` | Every scenario reaches normal, timeout, or harness-failure within an outer deadline. | Runner terminal outcome + elapsed ≤ deadline (+ cleanup bound). |
| `EXIT_CAUSE_PRESERVED` | Fixture-chosen ordinary exit code remains that exact code. | `ExitCause::Code` equality with Strong OS wait evidence. |
| `DRAIN_BOTH_STREAMS_NO_DEADLOCK` | Harness drains stdout and stderr independently without deadlock. | Both stream totals recorded; run completes without timeout. |
| `DESCRIPTOR_CLOSURE_EARLY_EXIT` | Child exit while harness still owns stream handles does not hang the runner. | Exit observed; runner returns. |
| `ZERO_UNSCRIPTED_INPUT_WRITES` | `StdinSpec::Closed` writes no hidden input; fixture sees EOF and 0 bytes. | Fixture `stdin-report` JSON: `stdin_eof=true`, `bytes_read=0`. |
| `ENV_RECORDED` | Environment policy and dedicated fixture-observed test variable match; no secret dump. | Harness `EnvRecorded` + fixture `OMEN_COMPAT_PROBE` value. |

Judgment lives in `omen-compat`, not in fixtures. Fixtures report facts only.
