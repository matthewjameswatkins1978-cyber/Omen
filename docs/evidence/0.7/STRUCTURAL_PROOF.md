# Omen 0.7 Semantic Environment — Structural Search & Mutation Proof

## 1. Objective

Prove that:
1. Structural search via `ast-grep` parses real Abstract Syntax Trees (ASTs) rather than naive text matching, distinguishing code syntax from comments and string literals.
2. Structural rewrites derived by `ast-grep` route safely through `ThreadMothBridge` to enforce pre-image hash checking, bounded surgical edits, and post-mutation certificates.

---

## 2. AST vs Pure Text Search (Proof A)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_a_structural_search_distinguishes_syntax_from_text`):

Source fixture `src/auth.rs`:
```rust
// fn refresh_token() -> bool { false }

fn refresh_token() -> bool {
    let _str = "fn refresh_token() -> bool { false }";
    true
}
```

### Results
- **Pure Text Search (`matches("fn refresh_token")`)**:
  - Found: **3 occurrences** (1 line comment, 1 function definition, 1 string literal).
  - Incapable of distinguishing AST nodes from comments or literals.
- **Structural Search (`ast-grep run --pattern "fn refresh_token() -> bool { $$$ }" --lang rust --json`)**:
  - Found: **1 match** (`structural_matches.len() == 1`).
  - Matched range: `src/auth.rs:3:0-6:1`.
  - Content: `fn refresh_token() -> bool {\n    let _str = "fn refresh_token() -> bool { false }";\n    true\n}`.
  - Zero false positives in comments or string literals.

---

## 3. Structural Rewrite Routed Through ThreadMoth (Proof B)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_b_structural_rewrite_routes_through_threadmoth`):

Source fixture `src/calc.rs`:
```rust
fn old_calculator(x: i32) -> i32 {
    x + 1
}
```

### Workflow
1. `AstGrepAdapter::derive_rewrite_candidates`:
   - Pattern: `fn old_calculator($$$) -> i32 { $$$ }`
   - Rewrite: `fn new_calculator($$$) -> i32 { $$$ }`
   - Yields candidate with target range and exact replacement snippet.
2. `ThreadMothBridge::apply_rewrite`:
   - Inspects file before modification, computing SHA-256 pre-hash.
   - Dispatches deterministic bounded mutation via `ThreadMothAdapter::exact_replace`.
   - Validates that unrelated bytes outside the replacement chunk are uncollateralized.
   - Computes SHA-256 post-hash and records a signed/attested mutation receipt.
3. Verification:
   - `src/calc.rs` contains `fn new_calculator(x: i32) -> i32 { x + 1 }`.
   - Mutation certificate confirms bounded scope and pre/post checksums.
