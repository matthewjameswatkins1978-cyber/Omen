# Omen Durable History

Omen history is evidence already durably recorded by Omen. It is not a complete
operating-system audit log, shell history, filesystem-change log, Tethers
decision log, or proof of external effects beyond the recorded execution result.

## Authority

The canonical query is `omen_knowledge::query_history` in
`crates/omen-knowledge/src/history.rs`. Its persistence source is the canonical
SQLite `execution_history` table plus the existing artifact references and
envelope field. The CLI and MCP are projections only:

```text
SQLite execution_history
        -> query_history(HistoryQuery)
        -> HistoryResult / HistoryEntry
        -> omen history --machine
        -> omen_history_query
```

The default limit is 20 entries; the maximum is 100. Results are ordered by
descending SQLite row identity (`sqlite_rowid_desc`), which is the durable
causal insertion order used by this query. There is no cursor pagination yet;
`pagination` is explicitly `none_bounded_limit`.

## CLI

```text
omen history
omen history --limit 50
omen history --machine
omen history --current-session --session-id sess_...
```

Human mode shows time, status, execution identity, and command. Machine mode
returns the canonical `HistoryResult` without requiring prose parsing.

## MCP

`omen_history_query` accepts `all_sessions`, `session_id`, and bounded `limit`
arguments. Its text payload is the same `HistoryResult` emitted by CLI machine
mode; only the JSON-RPC transport wrapper differs.

Each entry preserves the recorded execution identity, session identity, command,
status, recording time, duration, state-change witness when one was recorded,
and canonical evidence/artifact references. Missing historical fields remain
omitted or `UNKNOWN`; they are not reconstructed from unrelated data.

Direct local execution remains separately marked as
`UNJOURNALED_LOCAL_EXECUTION`. That marker means the action is outside durable
daemon history, not that nothing ran.
