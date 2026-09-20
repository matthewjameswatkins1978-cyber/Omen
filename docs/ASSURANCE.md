# Omen Assurance Model

## 1. Physical Enforcement Levels (`EnforcementLevel`)

In Omen 0.6 Physical Maturity, all physical containment and execution boundaries are classified according to five canonical enforcement levels:

| Level | Definition |
|---|---|
| `ENFORCED` | Hard kernel or hardware boundary actively preventing violation (e.g. Linux Landlock restricted write paths, Windows Job Object termination with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`). |
| `MEDIATED` | An intermediary subsystem or runtime proxy enforces or filters execution (e.g. WSL boundary, container runtime, virtualization layer). |
| `OBSERVED` | Reported truthfully by runtime observation without active hardware or kernel restriction (e.g. unconfined native process tree where PIDs are tracked post-spawn). |
| `BEST_EFFORT` | Platform or backend attempts enforcement but cannot guarantee containment under all conditions (e.g. advisory locks, soft memory limits). |
| `UNSUPPORTED` | Host platform or backend lacks the capability completely. |

## 2. Observation Provenance Levels (`Assurance`)

Omen explicitly classifies the semantic provenance and integrity of observations, facts, and artifacts:

| Level | Definition |
|---|---|
| `DETERMINISTIC` | Mechanically reproducible outcome guaranteed by mathematical or structural invariants (e.g. CAS SHA-256 digest, ThreadMoth AST transform). |
| `VERIFIED` | Established by active tool probe or execution that completed successfully (e.g. `fact://test/status` after `cargo test`). |
| `ENFORCED` | Actively constrained by platform security mechanisms during execution. |
| `OBSERVED` | Directly observed from process execution without sandbox restrictions. |
| `CLAIMED` | Asserted by a tool or profile without independent runtime verification. |
| `INFERRED` | Derived via logical rules or dependency graphs. |
| `UNKNOWN` | No assurance claims available. |

## 3. Fail-Closed Preflight

If an `ExecutionRequest` specifies a required assurance (e.g. `RequiredAssurance::Enforced`) that the active host backend cannot guarantee for that capability:
- Omen **refuses execution prior to process spawn**.
- It emits a structured `CoreError::PreflightFailed` detailing the required vs available assurance.
- Omen never executes unconfined while claiming protection or warning after the fact.

