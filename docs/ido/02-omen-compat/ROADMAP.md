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

### M0 POSIX tranche (M0-D/G) — implemented subset

- **M0-D** bounded POSIX PTY harness (`crates/omen-compat/src/posix/`),
  session / controlling-terminal / foreground-pgrp establishment, typed
  observations (`PosixProcessIdentity`, `PosixTerminalState`,
  `PosixWaitState`), Linux `/proc` observer, and invariant judges for the
  thirteen job-control / TTY IDs in `compat/invariants/posix.md`.
- **M0-G** `omen-gremlin` POSIX modes (`--posix-report`,
  `--posix-stop-report`, `--posix-sigint-report`,
  `--posix-sigint-observe`, `--posix-winch-report`,
  `--posix-termios-dirty-exit`), Tier POSIX CONTROL calibration tests, and
  Tier POSIX OMEN scenarios (foreground topology, exit reacquisition, stop,
  Ctrl-C with observed delivery, signal mask, SIGWINCH, termios recovery,
  zombie observation) plus measurement-integrity regressions
  (`posix_evidence.rs`).
- Decisions D2-014 (measurement not repair), D2-015 (establish then
  observe controlling terminal), D2-016 (bounded hostile PTY I/O),
  D2-017 (dependent claims never exceed observed evidence — no fabricated
  identity; reacquisition requires proven prior handoff; VINTR ≠ SIGINT
  delivery; `/proc` stopped ≠ Omen wait-path observation).
- Defect ledger: `POSIX_M0_DEFECTS.md` (M0-002 regraded INCONCLUSIVE;
  M0-003 kept UNAVAILABLE as observability gap).

Still open for M0: Windows ConPTY/console control, deeper process-tree
containment, and any repair of production defects discovered by the
instrument (separate tranche).

## M1 — Deterministic Compat foundation

After architecture approval: bounded fixtures, declarative scenarios,
structured results, evidence references, and conservative classification.

## M2 — Selected golden applications

Only after M0 and M1 are proven: a deliberately small set of real tools such
as `micro`, `gh`, or `vim`, chosen for coverage rather than a claim of
universality.

Automatic repair, unrestricted fuzzing, arbitrary application discovery,
production shims, and universal compatibility claims remain deferred.
