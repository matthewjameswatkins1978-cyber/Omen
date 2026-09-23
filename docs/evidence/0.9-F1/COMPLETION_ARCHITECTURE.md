# 0.9-F1 Completion Architecture and Ranking

## Pipeline

```
CompletionContext (buffer, cursor, cwd, bounded hot state)
        |
        v
scan_words_with_spans + token_at_cursor   (ONE accepted grammar, spans)
        |
        v
deterministic bounded sources  -->  Vec<CompletionCandidate>
        |
        v
ONE ranking authority  -->  sorted, bounded candidate set
        |
        +--> ghost (single, high-confidence only)
        |
        +--> explicit candidate menu (explicit completion key)
```

`CompletionCandidate` carries `display_text`, `literal`, `kind`, `source`,
`score`, and an explicit `CompletionEdit { replacement_range, insertion_text }`.
The UI applies that edit. Reedline does not infer spans from display text.

## Candidate kinds

`Command`, `Intrinsic`, `OmenAction`, `Option`, `Path`, `Directory`,
`TypedReference`, `WorkspaceTarget`, `Service`, `Resource`.

Only kinds backed by accepted-main data are used.

## Sources (deterministic, bounded)

| Source | Authority | Hot-path cost |
|--------|-----------|---------------|
| OmenActionAuthority | `commands::OMEN_ACTIONS` (shared with SemanticDispatcher) | memory |
| IntrinsicAuthority | `commands::SHELL_INTRINSICS` (cd/exit/quit) | memory |
| PathCommandCache | bounded out-of-band PATH scan | memory read |
| Filesystem | bounded single-directory listing | one `read_dir`, capped |
| TypedReferenceAuthority | `TypedReference::STATIC_HANDLES` + live fact/service names | memory |
| SyntaxMetadata | static tool/action subcommand tables | memory |
| HotSemanticIndex | facts, symbols, packages, tasks, services | memory |

**Not sources (never consulted on a keystroke):** LLM/AI provider, Codex,
OpenCode, remote API, web, git subprocess, cargo subprocess, LSP startup,
recursive filesystem walk, database migration, daemon startup, network
discovery.

`known_tools` is gone as command truth. External executables come from the
bounded PATH cache (`commands::list_path_commands`). On Windows only
PATHEXT-recognised executable forms count; PowerShell aliases are not
invented.

## Ranking authority

Deterministic, documented, non-learned:

1. kind base priority (OmenAction 900 > Intrinsic 880 > Command 860 >
   TypedReference 840 > Resource 760 > Service 740 > WorkspaceTarget 720 >
   Directory 700 > Path 680 > Option 600)
2. prefix quality (exact +200 > case-sensitive prefix +100 > case-insensitive +60)
3. length penalty (capped, prefer less remaining typing)
4. locality bias (directories +15 once a path parent is typed)

Tie-break: lexicographic `(kind, literal, source)`. Alphabetical order is only
this stability tie-breaker, never a preference.

Same input/state => same ordering. Proof: `f1_identical_context_produces_identical_ordering_and_edits`.

## Ghost confidence

A ghost is not `candidate[0]`. All must hold:

- context-valid kind
- prefix-compatible
- suffix-compatible when mid-token
- safe edit (round-trips through the accepted grammar)
- append-safe for the line editor (pure insertion at cursor, or full-span
  replace that extends the raw prefix)
- unique **or** best.score - second.score >= 30

Ambiguous => no ghost. Calm beats twitchy.

Ghosts only appear at the live end of input. Mid-line, the explicit menu is the
calm interaction.

## Bounds (centralised in `completion::bounds`)

| Constant | Value |
|----------|-------|
| MAX_PATH_DIRS | 64 |
| MAX_PATH_ENTRIES_PER_DIR | 256 |
| MAX_COMMAND_NAMES | 512 |
| MAX_FS_SCAN | 1024 |
| MAX_FS_ENTRIES | 256 |
| MAX_CANDIDATES_PER_SOURCE | 32 |
| MAX_FINAL_CANDIDATES | 24 |
| MAX_DISPLAY_CHARS | 80 |
| GHOST_MIN_SCORE_MARGIN | 30 |
| PATH_CACHE_TTL | 60s |

Sources truncate deterministically (sorted then capped) and expose truncation
flags rather than pretending coverage is complete.

## Hot path

Keystroke path may read: parsed buffer, cwd, immutable command authority,
bounded local completion state. One bounded `read_dir` per path completion
(non-recursive, capped).

It must not: scan PATH, recurse the filesystem, spawn, call git/cargo/AI,
touch the network, sleep, or retry.

PATH discovery runs out-of-band (session startup, prompt boundary, after
execution, explicit refresh) with a TTL.

## Failure behaviour

Unreadable directory, deleted cwd, malformed partial quote, permission denied,
invalid UTF-8 boundary, or an unrepresentable literal all degrade to "no
candidate" / "no ghost". No keystroke errors. No panics
(`f1_malformed_partial_input_never_panics`).
