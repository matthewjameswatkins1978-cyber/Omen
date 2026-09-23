# IDO No. 2 roadmap

## M0 — Runtime ground truth

Establish the process, PTY, terminal, signal, stream, and cleanup facts that
Compat must observe. Existing Omen behaviour is preserved while defects are
measured; this filing change does not repair them.

### M0 portable tranche (M0-A/B/C) — implemented subset

Implemented now:

- **M0-A** core types (`Platform`, `Capability`, `CommandSpec`, `StdinSpec`,
  `Deadline`, `ExitCause`, `EnvPolicy`, `EvidenceGrade`) and a bounded
  portable runner (Tokio cancellable process I/O, independent stdout/stderr
  drain with grace+abort, explicit stdin policy, timeout kill + bounded root
  reap, explicit truncation vs EOF vs drain timeout).
- **M0-B** observations, evidence grades, invariant results
  (PASS/FAIL/UNSUPPORTED/UNAVAILABLE/OPEN_DEFECT/INCONCLUSIVE), structured
  failure records via secret-safe `CommandEvidence`, replay descriptors with
  `ReplayFidelity`, portable invariant judgments (elapsed-time aware).
- **M0-C** portable `omen-gremlin` fixture modes (`--exit-code`,
  `--stdin-report`, `--dual-stream`, `--large-output`, `--sleep-bounded`,
  `--spawn-child-portable`, `--compat-report`, `--descendant-holds-stdout`,
  `--ignore-stdin`) and Tier 0/Tier 1/secret-canary tests.

Measurement-integrity repair: bounded I/O + secret-safe durable evidence
laws documented in ARCHITECTURE and DECISIONS D2-008/D2-009.

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
