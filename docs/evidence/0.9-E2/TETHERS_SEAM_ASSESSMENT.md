# E2.4 — Tethers Host Seam Assessment

Determination: **`BLOCKED_BY_TETHERS`**

Recorded against E2 branch HEAD at assessment time; re-verify before any
future claim of a live seam. A visible blocker is acceptable. A fabricated
semantic bridge is not.

## What was inspected

1. Full Omen repo for `tether` (case-insensitive, code + docs + config).
2. All `Cargo.toml` files for a `tethers` crate dependency.
3. Process environment for `TETHERS_*` variables (empty).
4. CI workflows for Tethers references (none).
5. Sibling checkout `D:\Tethers Web\README.md` (display-only prototype,
   self-described: no backend, no source-code link into a Tethers engine).
6. Omen's own 0.8 acceptance evidence (`docs/evidence/0.8/`).

## Findings (absence proofs)

- `AdmissionRequest` / `AdmissionResult` / `CapabilityIdentity` /
  `ScopeIdentity`: **0 hits** anywhere. (Repair note: the E2 candidate
  briefly shipped Omen-owned neutral data carriers for these names; the
  repair removes that speculative ontology. Tethers owns capability,
  scope, admission, and revocation semantics, so Omen must not mint the
  vocabulary first and force Tethers to conform to it.)
- `*.toml + tether`: **0 hits**. Root `Cargo.toml` carries 16 `omen-*`
  path dependencies plus commodity crates; no `tethers` crate.
- `TETHERS_*` env: **0 hits** in code and environment.
- No Tethers URL, endpoint, socket, server process, or Host interface in
  Omen docs, config, or code.
- The only consumption path is file-supplied `ExecutionContract` JSON via
  CLI (`crates/omen-cli/src/main.rs`, "Path to Tethers execution contract
  JSON"); the bundled test fixture fabricates that JSON and asserts Omen
  executes mechanics only, leaving policy/approval to Tethers.
- Omen's own 0.8 evidence already declares the composition seam blocked
  awaiting a future Tether Set + live admission seam
  (`docs/evidence/0.8/TETHERS_BOUNDARY.md`,
  `docs/evidence/0.8/FINAL_ACCEPTANCE.md`).

## Conclusion

No stable, accepted external-Host admission interface exists TODAY. Omen
therefore:

- implements no `OmenPermissionEngine`, `OmenPolicyRules`,
  `OmenApprovalStore`, `OmenFallbackAuthority`, or semantic equivalent
  (verify: `rg -i "PermissionEngine|PolicyRules|ApprovalStore|FallbackAuthority" crates/`
  returns nothing);
- keeps executing file-directed mechanics exactly as before (no behavior
  change, no silent substitute).
- ships NO Omen-owned authority/admission/scope/evidence types: the
  blocked seam is evidence, not an invitation to design Tethers inside
  Omen. When Tethers exposes its accepted Host contract, Omen will
  implement the smallest adapter to that real contract.

## Revocation / multi-step admission

With no live provider there is nothing to revoke and no step to re-admit.
There is deliberately no Omen-side vocabulary for the hostile cases
(stale admission, wrong capability/scope, revocation before dispatch,
revocation between steps): those verdicts can only be supplied by a
future live seam outside Omen. Last-responsible-moment checks belong
immediately before consequential dispatch at that time — permission for
step one must never silently become indefinite permission for later steps.
