# Tethers Windows Replay-Root Owner Seam (Omen H2 / Preview 23)

Status: **external Tethers-owned defect, accepted as a Preview-23
documented exclusion by Lucy ruling**. Not an Omen defect. Omen must not
compensate for it. Preview-23 acceptance with this seam does NOT remove
the Windows live-authority item from the 1.0 convergence gate.

## Canonical Tethers identity

- Repository: `matthewjameswatkins1978-cyber/tethers-lang`
- Pinned source SHA: `7e29110319c554a6586865ec6c47a45498696d16`
  (R2 external authority gate; SHA-guarded in Omen CI on every run)
- Gate binary: `tethers-reference-host` (`tethers-0.1/host-rust`), release
- Engine: `tethers_mcp_main` (`tethers-0.1/engine-ocaml`, dune-built)
- Protocol: `tethers.authority/1`, product `0.8.0`

## Failing validation boundary (exact)

` tethers-0.1/host-rust/src/replay_windows.rs`

- `validate_security()` — owner MUST equal the process token user:
  `if !owner.equals(&user) { return unavailable(); }`
- `validate_existing_root()` — every provision/revalidation passes
  through it, including directories the Gate itself creates
  (`create_new_child` → `child_directory` → `validate_existing_root`).
- `provision_replay()` — all failures redact to
  `ReplayError::PersistenceUnavailable`.
- `gate_command.rs::run_gate()` — first provision fails → DACL-only
  `harden_host_data_acl()` succeeds → second provision still fails →
  `GATE_REPLAY_UNAVAILABLE` (`cannot provision durable replay state:
  PersistenceUnavailable`, exit code 6). The harden step can never repair
  ownership (it only sets the DACL), which is why the error is
  `REPLAY_UNAVAILABLE` and never `HOST_DATA_UNAVAILABLE`.

## Hosted environment (deterministic, all runs)

- Runner: `windows-latest` (`C:` fixed drive, NTFS)
- User: `runnervmvmocb\runneradmin`
- Token default owner: `BUILTIN\Administrators`
  (`whoami /groups` shows `BUILTIN\Administrators … Group owner`)
- Replay root: `C:\Users\RUNNER~1\AppData\Local\Temp\omen-h2-e2e-*\host-data`
  (absolute, no `/`, no `\\.` prefix, empty, DACL exactly three
  FullControl ALLOW ACEs: SYSTEM, Administrators, runneradmin)
- Direct forensics (hosted, all 6 E2E tests identically):
  `owner=BUILTIN\Administrators`, `whoami=runnervmvmocb\runneradmin`

## Exact typed Gate error

```json
{"schema":"tethers.cli/1","command":"gate","status":"failed","exit_code":6,
 "data":{},"error":{"code":"GATE_REPLAY_UNAVAILABLE",
 "message":"cannot provision durable replay state: PersistenceUnavailable"}}
```

## Minimal reproduction

1. On a Windows host whose token default owner is `BUILTIN\Administrators`
   (elevated / UAC-disabled administrator), create an empty dir with the
   documented strict DACL.
2. Run the canonical Gate (`gate --stdio --config … --engine … --trail …
   --host-data-root <dir>`) and send `hello`.
3. Hello is refused with `GATE_REPLAY_UNAVAILABLE`. Omen-side reproducer:
   `crates/omen-authority/tests/e2e_real_gate.rs` (any of the 6 live
   tests) — all fail identically at the hello handshake.

## Compliant-ownership comparison (passes)

- Local Windows (non-elevated, owner == invoking user): 18/18 live runs
  green; post-ruling CI keeps the positive assertion alive wherever the
  environment is compliant (adaptive seam probe).
- Ubuntu (uid ownership model, same Gate protocol): 6/6 live E2E green
  across 5+ consecutive hosted runs.

## Why Omen cannot repair it

1. New-directory ownership is OS-assigned from the token default owner;
   no caller flag changes that.
2. Setting the owner on Omen-created `host-data` alone is insufficient:
   the Gate re-validates ownership on every directory IT creates.
3. Pre-fabricating Gate-internal replay state from Omen would be a shadow
   implementation of Tethers internals and prove nothing about production.
4. Spawning the Gate under a different/de-elevated token would forge
   execution identity to satisfy authority.
5. Accepting group ownership is a Tethers-side semantic change (weakens
   the user-binding invariant) — a Tethers owner decision, not Omen's.

## Requested upstream resolution (Tethers owner)

Either make the Windows replay-root admission work under
administrator-default-owner tokens (or document elevation as
unsupported), because every elevated Windows invoker — CI runners and
real admin users alike — is currently unable to provision durable replay
state. Until then, Windows live-authority support in Omen stays
explicitly constrained.

## Ready-to-file upstream issue

Title: `Windows: replay-root provision fails under administrator-default-owner tokens (GATE_REPLAY_UNAVAILABLE)`

Body: canonical SHA `7e29110…`; `replay_windows.rs::validate_security`
requires `owner == TokenUser`; on administrator tokens new directories
are owned by `BUILTIN\Administrators`, so `provision_replay` fails
before and after the DACL harden, and Gate-created children would fail
revalidation the same way. Repro: empty DACL-hardened dir + `gate
--stdio` hello on an elevated host → exit 6,
`GATE_REPLAY_UNAVAILABLE`. Expected: provision succeeds (or elevation
is declared unsupported by contract).

## 1.0 blocker record

Preview 23 H2 acceptance with this seam does NOT resolve Windows
live-Tethers authority for Omen 1.0. Before 1.0 freeze, Tethers must fix
the owner seam, or Windows live-authority support must remain explicitly
constrained/unsupported. The macOS `prctl` compile seam
(`child_process.rs:405` under plain `cfg(unix)`) is recorded separately
and remains excluded for the same release train.
