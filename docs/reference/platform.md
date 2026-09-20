# Platform Reference

Omen is cross-platform by default.

## Targets

Windows — first-class target.

Linux — first-class target.

macOS — supported with guarantees reported truthfully.

WSL — distinct execution backend/path where configured.

## Assurance

Different systems provide different enforcement primitives.

Omen reports capability/assurance rather than flattening them:

ENFORCED
MEDIATED
OBSERVED
BEST_EFFORT
UNSUPPORTED

## Paths

Portable code should use platform-neutral path APIs.

Interactive grammar preserves Windows paths such as:

    C:\Users\Matthew\project
    \\server\share\project

Tests must not assume PowerShell, Bash, drive letters, /tmp, one executable suffix or one process-tree model.

## CI

Normal milestone acceptance expects hosted testing across Windows, Ubuntu and macOS.

External-provider acceptance may use a separate explicit environment when the provider is not a deterministic portable CI dependency.
