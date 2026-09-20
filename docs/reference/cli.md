# CLI Reference

Source checked against the 0.8 development branch CLI definitions.

Global form:

    omen [--json] [--machine] [--workspace <path>] <command>

## Global options

--json  
Request structured JSON where supported.

--machine  
0.8 development surface for bounded deterministic machine-facing output.

--workspace <path>  
Operate against a specific workspace root.

## Discovery

    omen --machine orient
    omen --machine orient --since <contract-digest>
    omen --machine capabilities
    omen --machine capabilities semantic
    omen describe
    omen --machine describe <capability-id>
    omen --machine how <recipe>
    omen --machine context
    omen --machine context --since <generation>

0.8 discovery surfaces remain development APIs until 0.8 acceptance.

## Health

    omen doctor
    omen --json doctor

## Tool Atlas

    omen tool list
    omen tool inspect <tool-id>
    omen tool validate <tool-id>

## Facts

    omen fact get <uri>
    omen fact get <uri> --require-current
    omen fact why <uri>

## Execution

    omen exec [--timeout-ms <n>] [--budget <bytes>] <argv...>
    omen exec --contract <file>

## Artifacts

    omen artifact inspect <hash>
    omen artifact read <hash> [--offset <n>] [--length <n>]

## Storage

    omen gc --dry-run
    omen gc

## Shared runtime

    omen daemon start [--foreground]
    omen daemon stop
    omen daemon status
    omen daemon ping

## MCP

    omen mcp [--workspace <path>]

MCP is an edge adapter, not Omen's internal semantic architecture.
