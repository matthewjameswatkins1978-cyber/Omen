# Initial Windows/Linux comparison subjects

Status: **declared, not yet executed against the new Portable Shell**. This
manifest guides Compat's initial comparison campaign; a subject is not an
acceptance result until exact tool identity, scenario, output, and evidence are
recorded.

| Subject | Windows reference | Linux reference | Evidence needed |
| --- | --- | --- | --- |
| Native Omen semantics | Omen Portable Shell on Windows | Omen Portable Shell on Linux | Typed result plus declared canonical byte projection |
| Conventional shell/tool behavior | PowerShell 7 and `cmd.exe` only where their behavior is the intended baseline | `/bin/sh` and Bash where their behavior is the intended baseline | Exact executable identity, argv, environment policy, cwd, exit and stream facts |
| Selected known-good utilities | Selected uutils builds or an explicitly named native reference tool | Selected uutils or installed conventional utilities | Version/fingerprint, licence/provenance, supported options, exact output bytes |

Do not treat Windows PowerShell and `cmd.exe`, or Linux `sh` and Bash, as
interchangeable. Each scenario names the reference whose semantics it intends
to witness. No shell is invoked through an ambient `PATH` lookup when a pinned
reference identity is required.

Comparisons exclude only explicitly declared noise. Normalization is ordered,
bounded, and applies to comparison projections only; raw observations remain
available. A matching timeout or incomplete stream is inconclusive, while any
observed mismatch is divergent. Every accepted divergence must become a named
Omen semantic decision, a platform difference, a fix, or a permanent regression.
