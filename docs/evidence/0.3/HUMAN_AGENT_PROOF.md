# Omen 0.3 — Human / Agent Shared-Reality Proof

**Verification Test**: `crates/omen-interactive/tests/h10_shared_reality_proof_tests.rs`  
**Status**: VERIFIED & DETERMINISTIC  

---

## 1. Thesis & Problem Statement

In traditional development environments:
- An AI agent mutating files in the background operates as a ghost.
- The human developer is unaware that previous build or test facts have become invalid until they explicitly run commands.
- Shared terminal histories get contaminated: the human's "up-arrow" or `@last` executes the AI's internal commands, destroying human workflow continuity.

Omen solves this through **Shared Reality with Strict Session Isolation**:
1. **Subordinate Session Isolation**: Each interactive human session has an explicit `InteractiveSessionId`. Dynamic references (`@last`, `@failed`) are strictly scoped to the human's own actions.
2. **Lazy Pessimistic Shared Reality**: When a background agent mutates an underlying workspace dependency (e.g., increments `fs:workspace` generation via ThreadMoth), any active fact relying on that generation immediately transitions from `CURRENT` to `DIRTY`.
3. **Calm Ambient Awareness**: The human's prompt updates automatically (`! 1 dirty`), alerting them without intrusive popups or disruptive focus stealing.
4. **Provable Invalidation**: The human can interrogate `:why @fact` to inspect the exact causal chain of invalidation.
5. **Explicit Revalidation**: Running the test suite regenerates the fact as `CURRENT`, returning the prompt to clean (`✓`).

---

## 2. Test Execution Walkthrough

The following sequence is formally verified in `test_human_agent_shared_reality_and_session_isolation`:

```
           HUMAN SESSION (sess-human-proof)                 AGENT SESSION (sess-agent-worker)
                          │                                                 │
(1) Runs `cargo test`    │                                                 │
    Publishes Fact        │ [fact://test:suite = PASSING (gen 1)]           │
    Prompt: clean (✓)     │                                                 │
    @last: "cargo test"   │                                                 │
                          │                                                 │
                          │                              (2) Runs `threadmoth mutate`
                          │                                  Mutates workspace (gen 1 -> 2)
                          │                                  @last: "threadmoth mutate..."
                          │                                                 │
(3) Inspects @last        │                                                 │
    STILL "cargo test"    │ [Session history isolated]                      │
                          │                                                 │
(4) Prompt updates:       │                                                 │
    `! 1 dirty`           │ [Fact transition: CURRENT -> DIRTY]            │
                          │                                                 │
(5) Dispatches `:why`     │                                                 │
    Inspects Provenance:  │                                                 │
    `fs:workspace [DIRTY]`│ [Recorded gen: 1, Current gen: 2]               │
                          │                                                 │
(6) Re-runs `cargo test`  │                                                 │
    Publishes Fact (gen 2)│ [fact://test:suite = PASSING (gen 2)]           │
    Prompt: clean (✓)     │                                                 │
```

---

## 3. Test Assertions & Evidence

### Step 1: Initial Clean State
Human executes `cargo test` and records clean passing fact:
```rust
assert!(p_clean.contains('✓'));
assert!(!p_clean.contains("dirty"));
assert_eq!(resolved_last, "cargo test");
```

### Step 2: Background Agent Mutation
Agent executes `threadmoth mutate` and increments `fs:workspace` generation from 1 to 2:
```rust
let new_gen = FactRegistry::increment_generation(&mut db, "fs:workspace").unwrap();
assert_eq!(new_gen, 2);
```

### Step 3: Subordinate History Isolation
Human `@last` remains unaffected by agent commands:
```rust
let human_last = ReferenceResolver::resolve("@last", &human_sid, &db).unwrap();
assert_eq!(human_last, "cargo test"); // NOT "threadmoth mutate"

let agent_last = ReferenceResolver::resolve("@last", &agent_sid, &db).unwrap();
assert_eq!(agent_last, "threadmoth mutate --request req.json");
```

### Step 4: Ambient Dirty Fact Awareness
Human prompt discovers the dirty fact on next render:
```rust
human_session.update_prompt_state();
let p_dirty = human_session.prompt.render_prompt_left();
assert!(p_dirty.contains("1 dirty"));
```

### Step 5: Causal Invalidation Explanation
Human interrogates provenance via `:why @fact://test:suite`:
```rust
let why = FactRegistry::why_fact(&db, &fact_uri).unwrap();
assert_eq!(why.fact.validity, ValidityState::Dirty);
assert!(why.dependencies.iter().any(|d| d.is_dirty));
```
Rendered diagnostic output:
```
Fact:       fact://test:suite
Value:      all 42 tests passed
Validity:   DIRTY
Assurance:  Verified
Dependencies:
  - fs:workspace [DIRTY]: recorded=1, current=2
```

### Step 6: Explicit Revalidation
Human re-runs `cargo test`, publishing refreshed fact with dependency generation 2:
```rust
human_session.update_prompt_state();
let p_reclean = human_session.prompt.render_prompt_left();
assert!(!p_reclean.contains("dirty"));
assert!(p_reclean.contains('✓'));
```

---

## 4. Conclusion

This proof demonstrates that Omen achieves true human-agent symbiosis without confusion, lock contention, or history pollution. Both human and agent operate against a single underlying causal truth while maintaining distinct, sovereign identities and execution trails.
