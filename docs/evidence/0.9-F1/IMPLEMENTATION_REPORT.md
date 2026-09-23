# 0.9-F1 Implementation Report — Deterministic Completion and Ghost Suggestions

Branch: `feature/omen-0.9-f1-completion-ghost`
Base: `888d38135f6b744075170c1137b5bf6a2fdf3fea`
Status: PARALLEL F PREP — do not merge before E2 acceptance / reconciliation.

## Existing shell architecture found (accepted main)

| Concern | Found |
|---------|-------|
| line editor | Reedline 0.51 (`Reedline::create()` in `InteractiveSession::run_loop`) |
| completion hooks | `reedline::Completer` + `CompleterAdapter`, `reedline::Hinter`, `ColumnarMenu` |
| key bindings | Reedline defaults preserved (Tab = menu, Right/End = accept hint, Escape = dismiss) |
| command authority | `SemanticDispatcher::dispatch` match arms (`:` actions); session intrinsics `cd`/`exit`/`quit`; no extracted registry (now `commands::`) |
| filesystem helpers | `std::fs::read_dir` in `HotSemanticIndex::refresh`; `omen_atlas::find_binary_on_path` |
| existing local knowledge | `HotSemanticIndex` (facts via `FactRegistry`, symbols/packages/tasks via `update_semantics`, service registry) |
| Tool Atlas | `omen-atlas` (`CommandSyntaxSpecification`, `find_binary_on_path`) — syntax/validation types, no populated tool registry |
| grammar | `GrammarScanner` + `split_words` (argv-only) + `TypedReference` |

## What changed

### `commands.rs` (new) — Omen-owned command authority
- `SHELL_INTRINSICS = ["cd", "exit", "quit"]`
- `OMEN_ACTIONS` — 24 canonical action names; `SemanticDispatcher`'s
  unknown-action diagnostic is generated from this list
- `list_path_commands` — bounded out-of-band PATH discovery (PATHEXT-aware)
- `tool_subcommands` / `omen_action_subcommands` — syntax metadata only

### `grammar.rs` — ONE grammar with spans
- `scan_words_with_spans` / `ScannedWord` / `CursorToken`
- `split_words` is a projection (existing tests pass unchanged)
- `quote_literal` / `encode_middle` / `round_trips` / `decode_single_word`

### `completion.rs` — typed pipeline
- `CompletionCandidate` + `CompletionEdit` (explicit replacement range)
- `CompletionEngine::complete` / `ghost` / `ghost_hint` (typed core, no terminal)
- mid-token suffix preservation; incompatible suffixes declined
- deterministic ranking authority (documented)
- centralised `bounds`
- `HotSemanticIndex` reworked: `path_commands` cache replaces `known_tools`;
  `known_actions`/`static_refs`/`workspace_entries` removed as competing lists
- `OmenCompleter` / `OmenHinter` Reedline adapters apply the typed edit

### Removed
- handwritten `known_tools` command list (and the AI-lane fallback copy)
- fragile CI timing asserts on completion paths
- non-authoritative `@symbol://` / `@package://` completion

## E2 dependency

**None.** This branch is based solely on `888d381`. No files from
`feature/omen-0.9-e2-truth-authority-history` were used. No rebase was
performed. Conflict risk with E2 is minimal (E2 does not touch
`omen-interactive` or `omen-ui`).

## Out of scope (unchanged)

E2, cancellation, durable history redesign, Tethers, AI completions,
Codex/OpenCode, command palette, Paste Guard, full @reference package,
service UX, PTY architecture, setup wizard, lifecycle/update, JS, Nushell,
pipes/redirection grammar.
