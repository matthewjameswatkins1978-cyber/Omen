# Omen 0.8 Tethers Boundary

This repository is initialised as a Tethers workspace by `.tethers/config.json`.
The configuration deliberately contains no self-granted permissions. Tethers
remains the authority boundary; Omen only consumes the resulting capability
and execution evidence.

## Bounded capability set

The current Windows Tethers host advertises these capabilities for this
workspace:

| Capability | Use | Omen status |
| --- | --- | --- |
| `workspace.stat`, `workspace.list`, `workspace.read` | Observe bounded workspace state | Available to host tooling |
| `workspace.create`, `workspace.replace`, `workspace.rename`, `workspace.delete` | Explicit host-controlled file lifecycle operations | Available to host tooling; not exposed as generic Omen `filesystem.write` |
| `git.status`, `git.diff`, `git.log`, `git.branch_current` | Read-only repository inspection | Available to host tooling |
| `git.stage`, `git.commit`, `git.branch_create` | Explicit local Git mutation | Available only through an explicit host-authorised Git request |
| `exec.run` | Run one explicit program and argv with bounded output | Available only through an explicit host-authorised request; no shell is inserted |
| `threadmoth.preview` | Validate and preview one workspace-relative ThreadMoth request | Available through the guarded ThreadMoth provider |
| `threadmoth.apply` | Apply one validated ThreadMoth request | Available through the guarded ThreadMoth provider |

ThreadMoth is the only mutation provider in this set. Its request must remain
workspace-relative, use the advertised protocol, and carry its own exact
target/cardinality/guard/effect-budget information. Tethers validates the
request before the provider is called and records the provider result in the
host Trail.

## Omen capability mapping

- `mutation.threadmoth` requires the guarded `threadmoth.preview` and
  `threadmoth.apply` path above. Omen does not substitute a raw write when
  that path is unavailable or refused.
- `execution.run` requires a separate current Tethers execution admission.
  The local host's `exec.run` capability is not silently treated as that
  Omen contract; the caller must supply the exact host-authorised request.
- `filesystem.read` may use the bounded workspace read surface.
- `filesystem.write` remains unsupported by Omen composition. The presence of
  host lifecycle operations does not grant Omen generic write authority.
- `composition.plan` and `composition.run` remain Omen planning/reporting
  surfaces. They do not create Tethers authority and do not bypass the
  capability-specific admission above.

## Omen 0.8 composition decision

`THREADMOTH_COMPOSITION = BLOCKED_BY_LIVE_TETHERS_AUTHORITY_BOUNDARY`.

The current public Tethers host seam does not provide Omen with a
capability-specific current admission for an exact structured ThreadMoth
mutation. Omen therefore keeps the existing ThreadMoth request/certificate
machinery and defers composition execution. It does not calculate or serialize
permission, approval, grants, or admission tokens.

## Important limitation

The installed host exposes a workspace project configuration, not the future
installable Tether Set source format described by the Tethers architecture.
This file therefore documents the live boundary and its required mappings; it
does not claim that an Omen-owned file can grant provider permission. A true
versioned Tether Set for Omen/ThreadMoth must be authored and admitted in the
Tethers repository through its own task and manifest/policy workflow.
