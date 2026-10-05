# Omen Compat corpus

This directory contains the small, reviewable compatibility corpus for IDO No.
2. It is data and test knowledge, not production runtime code.

| Path | Owns | Does not own |
| --- | --- | --- |
| `scenarios/` | Declarative steps, inputs, and expected observations | Rust harness implementation |
| `golden/` | Deliberately approved real-application targets and profiles | Per-application runtime hacks |
| `invariants/` | Names and definitions of behavioural truths | A second Omen authority |
| `fixtures/` | Small purpose-built probe programs and inputs | Large recordings or generated traces |

Keep committed material small and deterministic. Put large traces, terminal
recordings, matrices, and diagnostic dumps in CI artifacts. When a failure is
confirmed, reduce it to a minimal fixture or scenario before committing it.

See [IDO No. 2](../docs/ido/02-omen-compat/README.md) for the governing
boundary and lifecycle.
