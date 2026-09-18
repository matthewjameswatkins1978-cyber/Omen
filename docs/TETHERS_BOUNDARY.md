# Tethers Boundary Specification

## Boundary Responsibility

Omen accepts an externally authorized execution identity from Tethers and executes the physical mechanics underneath.

### Tethers Owns:
- Permission & Policy
- Trusted Capability Identity
- Scope authorization
- Approval workflows
- Durable intent
- Execution replay & journal
- Provider outcome truth
- Audit Trail

### Omen Owns:
- Enforcement preflight
- Process lifecycle & supervision
- Argv construction & closed stdin
- Resource mediation
- Local observations
- Machine facts & invalidation
- Raw CAS artifacts & bounded projections
- Physical runtime evidence

Omen **never** introduces a competing policy language, permission model, or approval workflow.
When executing a contract, Omen reports `runtime_status`, `process_exit`, and `adapter_classification`. It does not unilaterally declare provider success in Tethers' domain.
