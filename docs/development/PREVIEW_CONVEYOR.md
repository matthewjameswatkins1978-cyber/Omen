# Omen Preview Conveyor

The Preview Conveyor keeps source evidence, packaged bytes, installed bytes,
and external-trial readiness separate. It is intentionally a Rust `xtask`
command family so the same identity rules run on every host.

```text
cargo xtask preview status --json
cargo xtask preview preflight --package omen-mcp
cargo xtask preview package
cargo xtask preview install --artifact path/to/candidate.zip
cargo xtask preview prove
cargo xtask preview rollback
```

`package` creates a `local` candidate. Local proof is useful and may report
`READY_FOR_CI: YES`, but it always reports
`READY_FOR_EXTERNAL_TRIAL: NO`. The default `install` path requires an exact
CI artifact; `--artifact` is the explicit local-debugging escape hatch.

Installed slots contain preview version, abbreviated source SHA, provenance,
and the executable digest. Existing slots are never overwritten when their
manifest differs; the command returns `OMEN_INSTALL_SLOT_IDENTITY_CONFLICT`.
The stable executable is activated only after the candidate has been unpacked
and its manifest accepted.

`cycle` will not rebuild or fall back locally. It must locate a successful CI
run and its exact artifact in a future authenticated GitHub integration. Until
then it returns `OMEN_PREVIEW_CI_NOT_GREEN` with no false readiness claim.

Preview version and internal crate versions remain coupled during 0.8.
`PREVIEW_VERSION_DECOUPLING_0_9` is deliberately deferred to 0.9.
