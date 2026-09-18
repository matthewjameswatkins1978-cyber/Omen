# Omen 0.3 Human Interface Specification

## 1. Vision & Core Philosophy

Omen is an agent-native developer runtime.
Omen 0.3 exposes that structured runtime reality to humans through an interactive shell that is:
- **Intelligent before it is AI-powered**: Leverages typed resources, Tool Atlas, Facts, Git state, and execution history deterministically before calling any model.
- **Calm by default**: Ordinary success produces minimal noise. No congratulatory messages, no narration of trivial success.
- **Progressive disclosure, not user personas**: UI density adapts to state uncertainty (Levels 0 through 3), not fixed user identities.
- **Substrate, not sovereign**: Respects all system boundaries. Omen never introduces a competing policy language, permission engine, approval system, or replay authority.
- **Deterministic resolution**: Infer what has already been decided; never invent what has not.

---

## 2. Progressive Disclosure Levels

- **Level 0 (Silent Flow)**: Normal successful operation. Prompt returns cleanly without chatty noise.
- **Level 1 (Useful State Change)**: Compact notification of notable events (e.g. `! Tests out of date · src/auth.rs changed`).
- **Level 2 (Diagnostic)**: Focused failure diagnostic with deterministic next actions (e.g. `:open @failed`, `:why @last`).
- **Level 3 (Explanation)**: On-demand plain-English explanation generated from structured machine state and provenance.

---

## 3. The 3 Interaction Lanes

1. **Ordinary Executable Invocation**: Direct PATH dispatch (`cargo test`, `git status`, `rg TODO src`). Zero shell string injection.
2. **Semantic Actions (`:`)**: Explicit Omen runtime operations (`:status`, `:test auth`, `:why @last`, `:history`, `:doctor`, `:services`).
3. **Typed References (`@`)**: Deterministic handles resolving against current runtime state (`@last`, `@failed`, `@fact.test`, `@last.artifact`).
4. **Optional AI Lane (`?`)**: Model reasoning lane (`? why does auth keep failing?`). Clear fallback when unconfigured.
