# System Boundaries

## The Architectural Doctrine

> **Omen is substrate, not sovereign.**

The surrounding division of responsibility is absolute:

```text
                  ┌──────────────┐
                  │   LANTERN    │ (Durable memory, long-term context, provenance)
                  │   "Knows"    │
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   RESOLVE    │ (Live guard tokens, scope locks, fencing, conflict admission)
                  │ "Coordinates"│
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   TETHERS    │ (Permission, Capability identity, Policy, Intent, Replay, Outcome truth)
                  │  "Controls"  │
                  └──────┬───────┘
                         │
        ═════════════════╪═════════════════  Boundary Line
                         │
                  ┌──────▼───────┐
                  │     OMEN     │ (Physical machine mechanics, containment, typed state, facts, CAS)
                  │ "Makes Legible│
                  │& Enforceable"│
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │  THREADMOTH  │ (Deterministic bounded structural file mutations & refusal)
                  │  "Mutates"   │
                  └──────────────┘
```

> **Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

---

## 1. Boundary Invariants

### Lantern
- **Lantern owns**: Durable memory, long-term context, cross-session provenance, and durable contextual knowledge.
- **Boundary**: Omen produces physical execution traces, fact snapshots, and CAS artifacts, but does not implement long-term cognitive memory or replace Lantern.

### Resolve
- **Resolve owns**: Live coordination, guard tokens, scope locks, fencing, and concurrency/conflict admission.
- **Boundary**: Resolve admission cannot grant Tethers permission. Omen shared-runtime coordination (`omend` in 0.4) coordinates physical OS state and local IPC only; it must never grow into a competing lock or guard authority.

### Tethers
- **Tethers owns**: Policy, permission, trusted capability identity, approval, durable execution intent, authoritative replay/retry semantics, provider outcome truth, and authority/capability scope.
- **Boundary**: **Tethers controls authority. Omen reports enforceability.** Omen must never create a second permission model, capability trust model, approval system, or authoritative replay mechanism. When Omen reconstructs execution requests or compositions from subordinate evidence, execution must always be re-admitted under current Tethers authority.

### Omen
- **Omen owns**: Physical execution mechanics, containment, process handling, typed resources, observations, Facts, artifacts, machine state, execution evidence, truthful platform-assurance reporting, and human/agent interfaces over this reality.
- **Boundary**: Omen is substrate, not sovereign. It never becomes an autonomous agent, policy system, or workflow brain.

### ThreadMoth
- **ThreadMoth owns**: Deterministic bounded structural mutation, structural search/rewrite, pre/post validation, and mutation refusal.
- **Boundary**: Omen uses ThreadMoth via its structured CLI interface and tracks generation invalidation across mutated files. Omen does not reimplement AST mutation internally.

