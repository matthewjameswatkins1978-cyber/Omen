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
  `ScopeIdentity`: **0 hits** anywhere before E2.4 (E2.4 adds neutral Omen
  data carriers only — no Tethers logic).
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
- ships neutral Host-side data carriers only
  (`crates/omen-core/src/authority.rs`: `AdmissionRequest`,
  `AdmissionResult`-shaped `AdmissionVerdict`, `CapabilityIdentity`,
  `ScopeIdentity`, `AuthorityEvidenceReference`);
- reports `live_admission_status() == BlockedByTethers` with this evidence;
- fails closed: `check_live_admission()` returns `NotAdmitted {
  ProviderUnavailable }` for every request shape (proven by
  `authority::tests::no_live_provider_means_no_admission_for_any_shape`);
- keeps executing file-directed mechanics exactly as before (no behavior
  change, no silent substitute).

## Revocation / multi-step admission

With no live provider there is nothing to revoke and no step to re-admit.
The data vocabulary for the hostile cases (stale admission, wrong
capability/scope, revocation before dispatch, revocation between steps)
exists as `NotAdmittedReason` data; the verdicts can only be supplied by a
future live seam outside Omen. Last-responsible-moment checks belong
immediately before consequential dispatch at that time — permission for
step one must never silently become indefinite permission for later steps.
