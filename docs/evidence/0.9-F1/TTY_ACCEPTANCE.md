# 0.9-F1 Real-TTY Acceptance

Unit tests do not prove TTY UX. This checklist must be exercised in a real
terminal before Lucy's review.

## Manual checklist (human, interactive terminal)

Run `omen` (bare) on a TTY and try each row. Record PASS/FAIL and notes.

| # | Exercise | Expect | Status |
|---|----------|--------|--------|
| 1 | `\O/ › car` then completion key (Tab) | menu/ghost offers `cargo` if on PATH | pending human |
| 2 | `cd Pro` then Tab | directory `Program Files` (or similar) offered, quoted on accept | pending human |
| 3 | `cat "file with spa` then Tab | completes inside the quote; right-hand text intact | pending human |
| 4 | `:st` then Tab | `:status` / `:stop` / `:structure` menu (ambiguous, no aggressive ghost) | pending human |
| 5 | `:doc` | ghost `tor` shown dim; Enter submits only `:doc` | pending human |
| 6 | `:stat` then Right/End | ghost `us` accepted -> `:status` | pending human |
| 7 | edit in the middle of an existing line | completion inserts without eating right-hand args | pending human |
| 8 | ambiguous prefix `car` with cargo+card+carbon on PATH | no ghost; Tab shows bounded menu | pending human |
| 9 | directory with many entries | menu bounded, stays responsive | pending human |
| 10 | `2>&1` typed literally | no redirection ghost/insertion | pending human |
| 11 | Escape after menu/ghost | dismisses cleanly, buffer untouched | pending human |
| 12 | `NO_COLOR=1 omen` | ghost still usable (plain), no broken styling | pending human |
| 13 | narrow terminal (80 cols) | display text truncated, editing intact | pending human |
| 14 | ordinary typing after a ghost | does not fight the ghost; Enter uses typed text | pending human |

## What was exercised in this packet

### Non-TTY / machine smoke (actually run)

- `omen --help` — unchanged CLI surface
- `omen --machine --json doctor` — machine output unchanged; `git_sha` reports
  the F1 base `888d38135f6b744075170c1137b5bf6a2fdf3fea`
- `omen` on non-TTY stdin — prints `Omen: substrate, not sovereign. Run 'omen
  --help' for usage.` and does **not** enter the REPL (unchanged guard at
  `main.rs` `IsTerminal` check). Machine/non-TTY behaviour is unchanged.

### Engine + editor adapter proofs (actually run)

The following were exercised through the typed core and Reedline adapters
(`OmenCompleter::complete_items`, `OmenHinter::handle`) with deterministic
fixtures — this proves engine behaviour and edit application, not terminal
rendering:

- `car` -> `cargo` from PATH cache
- `fakecommand` -> no invented candidate
- `:doc` -> ghost hint `tor`
- `:stat` -> `:status`
- `cd Pro` style directory completion (quoted insertion)
- `cat "file with spa|ces.txt'` mid-token suffix preservation
- mid-line edit leaves right-hand arguments intact
- ambiguous `car` -> no ghost
- `2>&1` / `|` / `>>` never completed
- Escape/dismiss semantics (engine never auto-applies edits)
- NO_COLOR / degraded: insertion and display text contain no ANSI

## Known UI limitations (F1)

1. **Ghost is append-safe only.** A candidate that must re-quote the typed
   token (e.g. `cat file` -> `"file with spaces.txt"`) does not produce a
   ghost, because the line editor hint is append-only. The explicit menu
   applies the correct replace-edit. Declining beats corrupting.
2. **No destructive mid-token replace.** Incompatible suffixes are refused
   (`foo|bar` + `foobaz`).
3. **Ghosts only at end of input.** Mid-line the explicit menu is the calm
   interaction.
4. **Empty literal and `it's\` are unrepresentable** in the accepted grammar
   and are declined.
5. **Reedline default keybindings are preserved** (Tab = completion menu,
   Right/End = accept hint, Escape = dismiss). No custom binding was added.

## Result

Engine and edit application: **proven by tests**.
Real-terminal rendering and key feel: **pending human row-by-row sign-off**
above.
