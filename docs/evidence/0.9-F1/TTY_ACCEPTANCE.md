# 0.9-F1 Real-TTY Acceptance

Unit tests do not prove TTY UX. This document records which behaviours are
closed by which evidence class, and which remain for human feel sign-off.

## Evidence classes

| Class | Meaning |
|-------|---------|
| **ConPTY** | Genuine Windows ConPTY terminal-path proof — real `CreatePseudoConsole`, real Reedline, real keystrokes, real terminal output |
| **Deterministic** | Typed completion/edit proof — engine behaviour and edit application via controlled fixtures, not terminal rendering |
| **Human feel** | Keybinding acceptance and subjective calm — requires a person at a real terminal |

## Evidence matrix

| # | Behaviour | Evidence class | Status | Proof |
|---|-----------|---------------|--------|-------|
| 1 | `car` → `cargo` ghost visible | ConPTY | **PASS** | `pty_ghost_hint_is_visible_in_real_terminal` |
| 2 | `:doc` → `:doctor` ghost visible | ConPTY | **PASS** | `pty_omen_action_ghost_visible_in_real_terminal` |
| 3 | Tab menu shows `cargo` | ConPTY | **PASS** | `pty_tab_shows_completion_menu_with_candidates` |
| 4 | Escape dismisses ghost without insertion | ConPTY | **PASS** | `pty_escape_dismisses_ghost_without_insert` |
| 5 | NO_COLOR ghost remains plain and usable | ConPTY | **PASS** | `pty_no_color_ghost_is_plain_text` |
| 6 | ConPTY spawn + real Reedline I/O round-trip | ConPTY | **PASS** | `pty_conpty_spawn_and_basic_io` |
| 7 | Quoted/path completion (`file with spaces`, unicode, dot paths, Windows forms) | Deterministic | **PASS** | `f1_awkward_filenames_quote_correctly`, `f1_path_completion_file_and_directory_prefix`, `f1_dot_paths_and_parent_directory`, `f1_windows_and_relative_path_forms` |
| 8 | Mid-token RHS preservation (byte-for-byte) | Deterministic | **PASS** | `f1_middle_of_command_line_preserves_right_hand_arguments`, `f1_middle_of_token_compatible_inserts_missing_middle_only`, `f1_quoted_mid_token_inserts_middle_and_preserves_suffix`, `f1_single_quoted_mid_token_preserves_right_hand_text` |
| 9 | Ambiguity = quiet ghost | Deterministic | **PASS** | `f1_ambiguous_prefix_manufactures_no_confident_ghost`, `f1_empty_buffer_offers_authority_but_no_confident_ghost` |
| 10 | Bounded candidate/menu behaviour | Deterministic | **PASS** | `f1_huge_directory_fixture_stays_bounded`, `f1_candidate_generation_is_bounded_per_source` |
| 11 | Literal `2>&1` / `\|` / `>>` never completed as operators | Deterministic | **PASS** | `f1_redirection_and_pipes_are_never_completed`, `f1_pipe_characters_are_literal_argv_not_operators` |
| 12 | Grammar/span parity (span scanner projects `split_words`) | Deterministic | **PASS** | `f1_grammar_span_scanner_projects_split_words` |
| 13 | Windows PATH truth (`.exe`→stem, `.cmd`/`.bat`/`.com`→retain extension) | Deterministic | **PASS** | `windows_path_truth::*` (10 tests) |
| 14 | No destructive mid-token replacement | Deterministic | **PASS** | `f1_mid_token_incompatible_suffix_is_not_destroyed`, `f1_unquoted_space_middle_is_declined_not_corrupted`, `f1_already_matching_token_has_no_destructive_edit`, `f1_open_quote_unsafe_rewrites_are_declined` |
| 15 | Display/insertion safety (no ANSI, no unrepresentable corruption) | Deterministic | **PASS** | `f1_degraded_terminal_relies_on_plain_insertion_text`, `f1_insertion_text_is_never_the_display_text_when_quoting_is_needed`, `f1_unrepresentable_literals_are_unknown_not_corrupt` |
| 16 | Ghost hint acceptance via keybinding (Right/End) mutates buffer | ConPTY | **PASS** (was **FAIL** pre-repair) | `pty_right_arrow_accepts_ghost_into_buffer`, `pty_end_key_accepts_ghost_into_buffer` |

## Genuine ConPTY proofs (8)

All eight run through real Windows ConPTY (`NativePtyHandle::spawn` →
`CreatePseudoConsole`), real Reedline (`OmenCompleter` + `OmenHinter`), real
keystroke injection, and real terminal output reading with ANSI stripping.

1. **ConPTY spawn + real Reedline I/O** — `pty_conpty_spawn_and_basic_io`
2. **`car` → `cargo` ghost visible** — `pty_ghost_hint_is_visible_in_real_terminal`
3. **`:doc` → `:doctor` ghost visible** — `pty_omen_action_ghost_visible_in_real_terminal`
4. **Tab menu shows `cargo`** — `pty_tab_shows_completion_menu_with_candidates`
5. **Escape dismisses ghost without insertion** — `pty_escape_dismisses_ghost_without_insert`
6. **NO_COLOR ghost remains plain and usable** — `pty_no_color_ghost_is_plain_text`
7. **Right Arrow accepts ghost into buffer** (`:stat` → `:status`) — `pty_right_arrow_accepts_ghost_into_buffer`
8. **End key accepts ghost into buffer** (`:stat` → `:status`) — `pty_end_key_accepts_ghost_into_buffer`

### Pre-repair failure (recorded)

Before the `OmenHinter` repair, the build at `89348a2` exhibited a real
correctness defect: `complete_hint()` returned `String::new()`, so Reedline's
`HistoryHintComplete` path inserted nothing. The ghost was **visible** but
**not acceptable**. Matthew's human terminal check confirmed: typing `:stat`
showed ghost `us`, but pressing Right or End did not mutate the buffer.
This was repaired by storing the safe ghost suffix in `OmenHinter::current_hint`
and returning it from `complete_hint()`.

## Deterministic proofs (already established)

These are typed completion/edit proofs — engine behaviour and edit
application through controlled fixtures. They are **not** real-terminal
evidence.

- Quoted/path completion: spaces, apostrophes, unicode, dot/parent paths, Windows drive forms
- Mid-token RHS preservation: byte-for-byte right-hand argument survival
- Ambiguity = quiet ghost: close candidates produce no confident hint
- Bounded candidate/menu: 400-entry fixture → ≤256 FS entries → ≤24 final
- Literal `2>&1` / `|` / `>>`: never offered as operator completions
- Grammar/span parity: `scan_words_with_spans` projects `split_words` exactly
- Windows PATH truth: `.exe`→stem, `.cmd`/`.bat`/`.com`→retain extension, unsupported forms excluded
- No destructive mid-token replacement: incompatible suffixes and quote surgery declined
- Display/insertion safety: no ANSI in insertion/display, unrepresentable literals declined
- Ghost acceptance payload: `complete_hint()` matches rendered ghost exactly
- Stale hint clearing: buffer change, ambiguity, unsafe input, empty input all clear the accept payload
- `next_hint_token` policy: returns whole hint (F1 hints are single-token suffixes)

## Remaining human check

Matthew's original human check **failed** on the pre-repair build
(`89348a2`): Right/End did not accept the ghost. This is now **repaired and
proven by ConPTY** (`pty_right_arrow_accepts_ghost_into_buffer`,
`pty_end_key_accepts_ghost_into_buffer`).

The remaining human check is a **feel confirmation** only:

> Type `:stat`, observe ghost `us`, press **Right** or **End**, confirm the
> buffer becomes `:status` naturally.

This is not a correctness proof — correctness is proven by ConPTY. It
verifies the interaction feels calm and natural.

## Known UI limitations (F1)

1. **Ghost is append-safe only.** A candidate that must re-quote the typed
   token (e.g. `cat file` → `"file with spaces.txt"`) does not produce a
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

| Layer | Status |
|-------|--------|
| Engine and edit application | **proven** (deterministic) |
| Real-terminal rendering | **proven** (ConPTY) |
| Ghost visibility and dismissal | **proven** (ConPTY) |
| Tab menu | **proven** (ConPTY) |
| Ghost acceptance (Right/End) | **proven** (ConPTY) — was **FAIL** pre-repair |
| Keybinding acceptance feel | **pending human** (1 row, feel only) |
