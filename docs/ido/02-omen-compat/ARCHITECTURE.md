# Omen Compat architecture boundary

## Dependency direction

```text
compat corpus + omen-compat harness
                 │ observes
                 ▼
           Omen runtime under test
```

The reverse edge is forbidden: production crates must never import
`omen-compat`. Workspace membership is not a runtime dependency.

## M0 portable machinery (implemented)

The crate now contains authorized measurement types and a bounded portable
runner (`Platform`, `Capability`, `CommandSpec`, `StdinSpec`, `Deadline`,
`ExitCause`, observations, evidence grades, invariant results, structured
failures, replay descriptors). Platform-specific capabilities remain schema
placeholders until their milestone.

## Evidence layers

- Invariants state what must be true.
- Scenarios state how a bounded case is exercised.
- Fixtures provide the smallest useful probe.
- Golden targets witness selected real tools after the foundation is sound.
- CI artifacts retain large generated output without making Git the trace store.
- Documents record design, decisions, and interpretation; they are not test
  inputs.

Compatibility results must preserve the distinction between observed Omen
behaviour, reference behaviour, known difference, Omen defect, and unresolved
evidence. Compat is diagnostic and evidence-producing, not an authority or an
automatic fixer.
