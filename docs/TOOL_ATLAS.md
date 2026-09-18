# Omen Tool Atlas

## Overview
The Tool Atlas maintains discovery and runtime operational knowledge about developer tools present on the host system.

## Tool Validation Lifecycle
- `UNVALIDATED`: Binary discovered, profile loaded, but no active probing has occurred.
- `VALIDATED`: Probed inside an isolated test harness; capabilities verified.
- `STALE_VALIDATION`: Binary path, timestamp, or SHA-256 hash changed; requires re-validation.
- `INCOMPATIBLE`: Binary fails baseline compatibility checks.

## Runtime Profiles
Runtime Profiles (`profiles/*.toml`) describe:
- Binary naming and search paths
- Structured output flags (e.g. `--format-version 1`, `--json`)
- Default stdio expectations
- Capability probes (`--version`, `doctor --json`, `capabilities --json`)

Runtime profiles provide descriptive execution hints, not trusted permissions (which belong to Tethers).
