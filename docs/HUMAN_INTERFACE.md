# Omen 0.3 Human Interface Specification

> **Target**: *A state-aware interactive developer environment whose intelligence primarily comes from structured runtime truth.*  
> **Doctrine**: *Omen is substrate, not sovereign.*  
> **Comparative Formulation**:
> - Fish's humanity without Fish's language divergence.
> - Nushell's structural intelligence without visual overload.
> - Atuin's contextual history taken into typed runtime state.
> - Warp's command grouping without requiring a proprietary terminal.

---

## 1. Character & Philosophy

Omen 0.3 exposes structured runtime reality to human developers through an interactive shell that feels:
- **Fast, quiet, confident, forgiving, precise, modern, understated, discoverable.**
- **Not**: chatty, flashy, cluttered, magical, plugin-dependent, AI-dependent, or configuration-dependent.

### Core Principles:
1. **Intelligent before it is AI-powered**: Leverages typed resources, Tool Atlas, Facts, Git state, and execution history deterministically before calling any model.
2. **Calm by default**: Ordinary success produces minimal noise. No congratulatory messages, no narration of trivial success.
3. **Opinionated by default, adjustable at the edges**: Omen should not look customizable; it should look finished. Extensions add capabilities (adapters, parsers, witnesses, completion providers, resource providers, execution backends), not furniture.
4. **Persistent UI must earn its place**: Every visual element must provide orientation, communicate state, improve parsing, or prevent an error.
5. **Substrate, not sovereign**: Omen never introduces a competing policy language, permission engine, approval system, or replay authority.

---

## 2. Progressive Disclosure

Omen avoids rigid global user personas (FAST / STANDARD / GUIDED / LEARN). Expertise varies by task. Instead, progressive disclosure is driven by current runtime state, confidence, ambiguity, failure, and anomaly:

- **Level 0 (Silent Flow)**: Normal successful operation. Prompt returns cleanly without chatty noise (`✓ cargo test (exit 0, 340ms)`).
- **Level 1 (State Communication)**: Compact notification of notable state changes (e.g. `! Tests out of date · src/auth.rs changed`).
- **Level 2 (Diagnostic)**: Focused failure diagnostic with deterministic next actions (e.g. `:open @failed`, `:why @last`).
- **Level 3 (Explanation on Demand)**: Detailed causal explanation generated from structured machine state and provenance tree (`:why @fact.test`).

> **Dense first. Explanation on demand.**

### Configuration Surface:
The configuration surface remains deliberately minimal:
```toml
help = "quiet"           # quiet | balanced | explanatory
appearance = "system"     # system | dark | light
density = "compact"       # compact | comfortable
suggestions = true        # true | false
semantic_status = true    # true | false
ai = "ask"                # off | ask
```

---

## 3. Interaction Hierarchy & Grammar

Omen does not invent a general-purpose scripting language:

1. `cargo test auth` — **Raw executable**: Standard PATH invocation with closed stdin and CAS output spooling.
2. `:test auth` — **Omen semantic operation**: Substrate action dispatched through Tool Atlas and session state.
3. `:why @last` — **Deterministic explanation**: Inspection over known Omen truth and fact provenance.
4. `? why is auth flaky?` — **Optional AI reasoning**: Explicit advisory lane with deterministic local fallback.

See **[Interactive Grammar Specification](INTERACTIVE_GRAMMAR.md)** for grammar rules, Windows path parsing, and resolution mechanics.

---

## 4. Subordinate Physical History & Typed References

Omen distinguishes between personal interaction history and shared machine reality:
- `@last` means: *The most recent applicable operation initiated by the current interactive actor in the current interactive session.*
- A background agent changing files or Facts does not pollute Matthew's personal command history.
- Dynamic handles (`@last`, `@failed`, `@last.artifact`, `@errors`) resolve deterministically against SQLite session history.

See **[Semantic History Specification](SEMANTIC_HISTORY.md)** for schema, isolation invariants, and reference resolution.

---

## 5. Non-Blocking Completion & Hot Semantic Index

Keystroke responsiveness is sacred ("Pretty must never make slow"):
- Keystroke completion executes in memory across `HotSemanticIndex` (< 0.2 ms).
- Zero process spawning, recursive scans, network calls, Cargo metadata queries, or SQLite queries on the keystroke path.
- State is refreshed asynchronously prior to prompt rendering.
- Candidates ranked by: failed → dirty → recently touched → workspace relevant → recent → exact prefix → valid candidates.

See **[Completion Specification](COMPLETION.md)** for index design and ranking rules.

---

## 6. Terminal Integration & Semantic Blocks

Omen inhabits existing terminal emulators (Windows Terminal, WezTerm, Alacritty, Kitty, iTerm2, ConHost):
- Emits standard OSC sequences: OSC 7 (cwd sync), OSC 8 (hyperlinked CAS artifacts), OSC 133 / FTCS (semantic command boundaries).
- Semantic blocks: Every execution possesses structured identity independently of terminal rendering (`cargo test auth · op_8f12`).
- Invocation-specific interactive child process handoff (releasing raw mode for REPLs, pagers, editors, and interactive Git commands).

See **[Terminal Integration Specification](TERMINAL_INTEGRATION.md)** for capability negotiation, child handoff, and OSC protocols.

---

## 7. Protective Controls

### 7.1 Ghost Blast Radius Preflight
Before Enter is pressed, Omen inspects known consequences:
- `cargo clean` ↳ removes `target/**` (~2.4 GB).
- `threadmoth mutate` ↳ 4 files, 18 structural regions.
- For opaque operations: truthfully reports `effects unknown`.
- For partial knowledge: `known: writes target/**; unknown: additional effects possible`. Never manufactures certainty.

### 7.2 Multiline Paste Guard
Pasted multi-line buffers are intercepted before execution:
- Previews line count, external executables, network indicators, and known write effects.
- Prevents accidental immediate execution of destructive multi-line clipboard data.

---

## 8. Services & Process UX

Omen treats long-running background processes as semantic resources (`proc://`), replacing opaque job control (`%1`, `fg`, `bg`):
```text
:services
:status @service.dev
:stop @service.dev
:restart @service.dev
:logs @service.dev
```

---

## 9. Modular Specification Documents

- **[Interactive Grammar](INTERACTIVE_GRAMMAR.md)** — Grammar lanes, token splitting, Windows path support, and resolution rules.
- **[Semantic History](SEMANTIC_HISTORY.md)** — Subordinate physical history, `@last` actor scoping, and reference resolution.
- **[Completion Engine](COMPLETION.md)** — Non-blocking hot semantic index, ranking hierarchy, and ghost hints.
- **[Terminal Integration](TERMINAL_INTEGRATION.md)** — OSC protocols, semantic block identity, child handoff, and degradation.
- **[AI Reasoning Lane](AI_LANE.md)** — Advisory `?` lane, deterministic fallback, and suggested typed operations.

