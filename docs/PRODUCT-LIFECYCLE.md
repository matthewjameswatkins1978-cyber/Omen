# Omen Product Lifecycle (0.9-H)

Installing Omen installs usable Omen. Updating, cleaning, repairing,
rolling back, or uninstalling never requires understanding Omen's
internals. Convenience never erases truth: when Omen is uncertain, it
keeps data, refuses the action, and says why.

## Install

- **Normal install needs no toolchain.** No Rust, Cargo, Python, Node, or
  Go. The install is self-contained.
- **First launch works.** `omen` creates its own state directories with
  sane defaults. `omen setup` is optional and idempotent — it only
  prepares defaults and tells you what you *may* do next (install a
  provider CLI, run `omen doctor`).
- **Ownership.** Omen knows how it was installed:
  - `Omen` — placed by Omen itself (Preview conveyor or self-update).
    Self-update is available.
  - `PackageManager` — placed by WinGet/Homebrew/etc. Omen tells you to
    update through that manager and never replaces its own binary behind
    the manager's back.
  - `Development` — a cargo build. Never pretends to be a managed install.
  - `Unknown` — self-update is refused, not assumed.

## Channels

Two channels: **Stable** and **Preview**. The channel lives in *your*
installation (`omen channel` shows it, `omen channel stable|preview`
sets it). A project/workspace file can never change your channel,
update source, or retention — such keys are rejected as hostile input.

Use `omen update --check` anytime: it reports current version, channel,
ownership, and the candidate (if any) without downloading anything. A
network failure is reported as a network failure — never as "up to date".

## Update

Omen-owned installs update transactionally:

```
discover → download → verify → compatibility → snapshot →
stage → migrate → health-check → activate
```

- Nothing activates before checksums, provenance, and embedded identity
  all verify. Corrupted updates fail closed.
- The previous healthy install is kept until the new one proves healthy.
- If anything fails, the old version keeps working, the broken candidate
  is quarantined, and the failure is recorded — never a "maybe updated"
  state.
- **Binary rollback and state rollback are different things.** `omen`
  reports each separately and never presents one as the other.

## Keep or remove: clean, GC, uninstall

- `omen clean` removes only definitely disposable debris (temp files,
  stale scratch, failed downloads, reproducible caches). When in doubt it
  leaves things alone. `omen clean --plan` previews.
- `omen gc` applies retention policy to old, unprotected evidence.
  History always survives: collected payloads leave a truthful
  "payload unavailable / collected" marker with identity and digest.
  `omen gc --plan` previews; `omen gc --apply --plan-file <f>` applies,
  re-checking protection first — a plan that went stale refuses instead
  of deleting newly protected data.
- `omen pin add digest:<hex>` protects evidence from collection.
- **Uninstall removes the application, not your truth.** Default
  (`omen uninstall`) removes app bytes and keeps history, evidence, and
  pins, telling you exactly what stays. `omen uninstall --scope
  app-cache|everything --apply` widens removal; pins are always honored.

## Diagnose and repair

- `omen doctor` **observes only** — running it twice changes nothing. It
  distinguishes *installed* from *configured* from *working* from
  *authorised*: a provider binary on disk with no account is reported as
  "installed, authorisation unknown until use", not as working.
- `omen repair` shows a plan first and changes things only with
  `--apply`. Repairs that would require guessing (corrupt history,
  ambiguous install, missing provenance) are refused with a reason.
- `omen diagnostics` writes a support bundle to a local folder for you to
  inspect before sharing. Secrets and key values are redacted (only
  source labels like "provider configured: yes" are kept). Omen never
  uploads it.

## Display

One result, four projections: rich, standard, plain, machine
(`--machine`). Colour is decoration — status is always a text token like
`[ok]` / `[warn]` / `[fail]`, so `NO_COLOR`, pipes, narrow terminals,
and ASCII-only environments (`\O/ >` prompt) keep full meaning. Omen
never depends on animation or cursor tricks outside a real terminal.
