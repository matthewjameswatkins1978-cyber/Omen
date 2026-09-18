# Omen Assurance Model

## Assurance Levels
Omen explicitly classifies the semantic provenance and enforcement rigor of every observation, fact, and containment guarantee.

| Level | Definition |
|---|---|
| `DETERMINISTIC` | Mechanically reproducible outcome guaranteed by mathematical or structural invariants (e.g. CAS SHA-256 digest, ThreadMoth AST transform). |
| `VERIFIED` | Established by active tool probe or execution that completed successfully (e.g. `fact://test/status` after `cargo test`). |
| `ENFORCED` | Hard operating system containment actively preventing violations (e.g. Linux Landlock restricted write paths, Windows Job Object termination). |
| `OBSERVED` | Reported truthfully by runtime observation without active OS sandboxing (e.g. standard process execution without namespace isolation). |
| `CLAIMED` | Asserted by a tool or profile without independent runtime verification. |
| `INFERRED` | Derived via logical rules or dependency graphs. |
| `UNKNOWN` | No assurance claims available.

## Fail-Closed Preflight
If an `ExecutionContract` requests an assurance level (e.g. filesystem containment: `ENFORCED`) that the current host platform or backend cannot provide, Omen **rejects execution prior to process spawn**. Omen never executes and warns afterward.
