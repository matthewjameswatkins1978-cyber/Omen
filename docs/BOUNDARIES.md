# System Boundaries

## The Architectural Doctrine
> **Omen is substrate, not sovereign.**

```text
                  ┌──────────────┐
                  │   LANTERN    │ (Durable memory, long-term context, provenance)
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   RESOLVE    │ (Live guard tokens, concurrency, scope locks)
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │   TETHERS    │ (Permission, Capability identity, Policy, Intent, Trail)
                  └──────┬───────┘
                         │
        ═════════════════╪═════════════════  Boundary Line
                         │
                  ┌──────▼───────┐
                  │     OMEN     │ (Physical machine mechanics, containment, typed state)
                  └──────┬───────┘
                         │
                  ┌──────▼───────┐
                  │  THREADMOTH  │ (Deterministic bounded structural file mutations)
                  └──────────────┘
```

### Boundary Invariants
1. **Lantern**: Omen produces execution traces and fact snapshots, but does not implement cross-session memory or replace Lantern.
2. **Resolve**: Resolve coordinates live locks on opaque scope keys. Omen does not interpret Resolve plans or lock tokens.
3. **Tethers**: Tethers owns permission, approval, replay, and outcome truth. Omen receives execution contracts with external Tethers execution IDs, performs physical execution, and reports machine reality.
4. **ThreadMoth**: ThreadMoth performs bounded structural edits. Omen uses ThreadMoth via its public machine CLI interface and tracks changed file generations.
