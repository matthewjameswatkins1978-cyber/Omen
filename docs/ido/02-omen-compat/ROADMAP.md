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

Measurement-integrity repairs: bounded I/O + secret-safe durable evidence
laws (D2-008/D2-009); final truth-model repair — non-waiting kill initiation
+ bounded reap, one shared post-root I/O completion window matching
`declared_max_wall_ms`, and mechanically validated `ReplayFidelity::Exact`
(D2-010/D2-011); Exact authority sealed to `try_exact_fixture` alone
(D2-012); Exact sealed at the type boundary — private fields + controlled
deserialization rejects Exact (D2-013).

Still open for M0: POSIX process-group/TTY work, Windows ConPTY/console
control, deeper process-tree containment, and any repair of production
defects discovered by the instrument.

## M1 — Deterministic Compat foundation

Approved by D2-014 for the Windows/Linux RC. The deterministic portable
foundation consists of bounded reference execution and canonical observation,
exact-fixture replay, differential comparison, explicit bounded normalization,
seeded structured fuzzing with named failure predicates, bounded shrinking,
minimal regression-record generation, and evidence-graded capability reports.
The same process runner and secret-safe evidence model underpin each operation.

This does not complete the full M0 runtime-ground-truth campaign or initial
reference-shell matrix. PTY/ConPTY and terminal-control probes, application
adapters, golden apps, unrestricted fuzzing, and automatic repair remain
deferred. Cross-shell Windows/Linux fixtures and representative known-good
reference subjects remain required before Compat is an accepted shell quality
engine.

## M2 — Selected golden applications

Only after M0 and M1 are proven: a deliberately small set of real tools such
as `micro`, `gh`, or `vim`, chosen for coverage rather than a claim of
universality.

Automatic repair, unrestricted fuzzing, arbitrary application discovery,
production shims, and universal compatibility claims remain deferred.
