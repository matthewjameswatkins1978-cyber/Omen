# Omen Knowledge Model

## Overview
Omen maintains a session/workspace Fact Registry and Content-Addressed Storage (CAS) backed by embedded SQLite in WAL mode.

## Fact Domains
- `fact://git/branch`: Current checked-out Git branch
- `fact://git/clean`: Cleanliness status of the working tree
- `fact://test/status`: Outcome of the test suite (passing, failing)
- `fact://compiler/errors`: Compiler diagnostic state

## Fact States & Lazy Pessimism
- `CURRENT`: Fact is valid and dependencies are unmutated.
- `DIRTY`: A dependency has changed or may have changed. Omen refuses to automatically revalidate; caller must explicitly re-execute.
- `STALE`: Fact has expired.
- `SUPERSEDED`: Replaced by a newer Fact instance (historical record preserved).
- `HISTORICAL`: Historical archive.

## Invalidation Rule
When a resource generation increments (e.g. `generation://cargo/build-inputs` changes due to file modification), all dependent facts transition from `CURRENT` to `DIRTY`.
If a query specifies `require_current = true` on a `DIRTY` fact, Omen fails closed with a structured `FACT_DIRTY` error.

## Content-Addressed Storage (CAS)
- Identity: `artifact://sha256/<64 hex>`
- Layout: `<OMEN_STATE_HOME>/workspaces/<workspace-id>/cas/sha256/ab/abcdef...`
- Retention: `PINNED`, `REFERENCED`, `CACHE`, `EPHEMERAL`.
- Output bounding: Outputs above 4–8 KiB are spooled to CAS, returning only bounded previews to caller context.
