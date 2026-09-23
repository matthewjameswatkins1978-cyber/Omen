# Omen Completion Specification

> **Rule**: *Pretty must never make slow.*
> **Latency Budget**: *Keystroke completion is pure bounded in-memory work.*

---

## 1. Core Principles

Interactive auto-completion in Omen is:

- **Editing assistance, not a second grammar.** Completion helps a human reach
  canonical text that the accepted parser already understands. It never makes
  new words meaningful.
- **Deterministic before generative.** Same input/state produces the same
  candidates, edits, and ordering.
- **Typed.** One `CompletionCandidate` model with an explicit
  `CompletionEdit { replacement_range, insertion_text }`. The UI applies that
  edit; it does not guess spans from display text.
- **Calm.** At most one ghost, and only when the best candidate is clearly
  correct. Ambiguity stays quiet.

---

## 2. Prohibition of Keystroke Latency

Strictly prohibited on the keystroke completion path:

- Process spawning
- Recursive filesystem scans
- Network calls or remote lookups
- Cargo metadata or package manager invocations
- PATH scans (these happen out-of-band into a bounded cache)
- Database queries or migrations
- Model / AI provider calls
- Sleep or retry loops

One capped, non-recursive `read_dir` is permitted for path completion.

---

## 3. Architecture

```
CompletionContext (buffer, cursor, cwd, bounded hot state)
        |
        v
scan_words_with_spans + token_at_cursor   (ONE accepted grammar, with spans)
        |
        v
deterministic bounded sources -> Vec<CompletionCandidate>
        |
        v
ONE ranking authority -> sorted, bounded set
        |
        +--> ghost (single, high-confidence only)
        +--> explicit candidate menu
```

Command/action truth lives in `omen-interactive::commands`:

- `SHELL_INTRINSICS` — `cd`, `exit`, `quit` (session-owned)
- `OMEN_ACTIONS` — canonical `:action` names, shared with `SemanticDispatcher`

External executables come from a bounded out-of-band PATH cache
(`commands::list_path_commands`). On Windows only PATHEXT-recognised
executable forms are accepted. There is no handwritten "known tools" list.

See `docs/evidence/0.9-F1/COMPLETION_ARCHITECTURE.md` for kinds, sources,
ranking, bounds, and ghost confidence.

---

## 4. Quoting

Insertion is validated to round-trip through the accepted argv grammar:

```
candidate literal -> quote/insert -> canonical text -> accepted parser -> same literal
```

Anything the grammar cannot represent (empty literal, `it's\`) is declined
rather than corrupted. See `docs/evidence/0.9-F1/GRAMMAR_FINDINGS.md`.

---

## 5. Ghost Hinter

`OmenHinter` provides at most one inline ghost when the best candidate is
context-valid, prefix/suffix compatible, append-safe, and unique or decisively
better than the runner-up.

**Rule**: Ghost suggestions remain purely visual hints until explicitly
accepted via the editor's hint-accept key (Right/End). Pressing `Enter`
submits only what the human has visibly typed.

The accepted grammar is argv-only: `|`, `>`, `>>`, `2>&1`, `&&`, `;` are
literal argv text on accepted main and are never completed as operators.
