# IDO No. 2 roadmap

## M0 — Runtime ground truth

Establish the process, PTY, terminal, signal, stream, and cleanup facts that
Compat must observe. Existing Omen behaviour is preserved while defects are
measured; this filing change does not repair them.

### M0 portable tranche (M0-A/B/C) — implemented subset

Implemented now:

- **M0-A** core types (`Platform`, `Capability`, `CommandSpec`, `StdinSpec`,
  `Deadline`, `ExitCause`, `EnvPolicy`, `EvidenceGrade`) and a bounded
  portable runner (argv spawn, independent stdout/stderr drain, explicit
  stdin policy, timeout kill + bounded reap, explicit truncation).
- **M0-B** observations, evidence grades, invariant results
  (PASS/FAIL/UNSUPPORTED/UNAVAILABLE/OPEN_DEFECT/INCONCLUSIVE), structured
  failure records, replay descriptors, portable invariant judgments.
- **M0-C** portable `omen-gremlin` fixture modes (`--exit-code`,
  `--stdin-report`, `--dual-stream`, `--large-output`, `--sleep-bounded`,
  `--spawn-child-portable`, `--compat-report`) and Tier 0/Tier 1 tests.

Still open for M0: POSIX process-group/TTY work, Windows ConPTY/console
control, deeper process-tree containment, and any repair of production
defects discovered by the instrument.

## M1 — Deterministic Compat foundation

After architecture approval: bounded fixtures, declarative scenarios,
structured results, evidence references, and conservative classification.

## M2 — Selected golden applications

Only after M0 and M1 are proven: a deliberately small set of real tools such
as `micro`, `gh`, or `vim`, chosen for coverage rather than a claim of
universality.

Automatic repair, unrestricted fuzzing, arbitrary application discovery,
production shims, and universal compatibility claims remain deferred.
