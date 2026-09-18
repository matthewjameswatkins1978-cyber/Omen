# Omen AI Reasoning Lane Specification

> **Doctrine**: *Omen should be intelligent before it is AI-powered.*  
> **Core Principle**: *Can this be answered from structured state, schemas, history or provenance? If yes, do it deterministically. Use AI only where judgement is genuinely required.*

---

## 1. Architectural Role

Omen is an agent-native developer runtime substrate, not an autonomous agent or chat assistant. The AI Reasoning Lane (`?`) is an explicit, optional advisory interface that allows developers to request machine judgement without polluting the deterministic command flow:

```text
? why did the auth test fail?
? how do I reformat modified rust files?
```

---

## 2. Invariants & Guardrails

1. **Explicit Invocation Only**: AI is never ambient, intrusive, or unsolicited. It activates solely when the user explicitly prefixes an input line with `?`.
2. **Zero Authority Grant**: AI suggestions are advisory text. An LLM output cannot grant permissions, approve transactions, or silently execute commands.
3. **Fully Functional Without AI**: Omen operates completely and cleanly with AI turned off (`ai = "off"`). No core runtime feature, completion, diagnostic, or tool execution requires an LLM.
4. **Suggests Typed Substrate Actions**: When reasoning about failures, the AI lane suggests concrete, typed Omen operations:
   - `:show @failed`
   - `:why @last`
   - `:open @errors`
   - `:rerun @failed`
5. **Bounded Context**: Rather than scraping megabytes of raw terminal ANSI text, the AI lane supplies bounded structured context:
   - Command line and exit code.
   - Bounded stdout/stderr previews from CAS.
   - Active dirty/current Facts and dependencies from SQLite.

---

## 3. Deterministic Local Fallback

If no LLM provider is configured (e.g. `ai = "ask"`, or when offline), `AiLaneDispatcher` provides high-utility deterministic assistance based purely on local runtime truth:

```text
? why did the build fail?

[Omen Substrate Diagnostic (Deterministic Fallback)]
Last command: cargo test --test auth (Exit code: 101, Duration: 420ms)
Stderr artifact: artifact://sha256/7b2e...
Known dirty facts: fact://test/status [DIRTY]

Suggested actions:
  :show @failed     - Display error transcript from CAS
  :why @fact.test   - View causal dependency invalidation tree
  :rerun @last      - Re-execute the failed test suite
```

---

## 4. Configuration

The AI lane is governed by a simple user configuration toggle:

```toml
# ai = "off" | "ask"
ai = "ask"
```

- `"off"`: Suppresses external model queries entirely; `?` always resolves using the local deterministic fallback engine.
- `"ask"`: Queries an external model provider when configured, with seamless fallback on failure or network absence.
