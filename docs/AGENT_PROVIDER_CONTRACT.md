# Omen Agent Provider Contract (G1)

Providers are engines behind the dashboard, not applications inside it.
Omen owns machine truth, routing, authority, failure meaning, bounds, and
presentation. Providers reason over a bounded request and return a typed
response. That is the whole job.

## Trait to implement

`omen_agent::AgentProvider` (see `crates/omen-agent/src/provider.rs`):

```rust
fn respond(&self, request: AgentRequest)
    -> Pin<Box<dyn Future<Output = Result<AgentResponse, AgentError>> + Send>>;
```

## Shapes

- In: `AgentRequest { prompt: String, context: AgentContext, conversation: Vec<AgentTurn> }`.
  The context is bounded and redacted before you see it. Never assume fields
  beyond the struct; never persist the request beyond the call.
- Out: `Result<AgentResponse, AgentError>`.
  `AgentResponse { kind, message, proposed_actions, references, uncertainty }`.
  `kind` is one of explanation / proposal / action-request / result /
  question / refusal. Refusal is a **success**, never an error.
- Proposals: `ProposedAction::{ChangeDirectory, ExecuteTool, ExecuteCommand,
  SemanticAction}`. Proposals are typed data. **You never execute them.**
  Omen/Tethers decides authority; unauthenticated, unapproved, or merely
  authenticated proposals all stop at the boundary.

## Error mapping

Map your failures into `AgentError` (see `provider.rs`) and keep vendor
concepts as bounded detail strings, never as new semantics:

| Situation | AgentError |
|---|---|
| missing/rejected credential | `AuthenticationRequired` |
| account/model/entitlement/endpoint unavailable | `ProviderUnavailable` |
| quota / 429 | `RateLimited` (retry is explicit, never automatic) |
| deadline exceeded | `Timeout` |
| capability you do not offer | `UnsupportedCapability` |
| transport / disconnect / 5xx | `Provider` |
| malformed, oversized, incomplete, or unknown-shaped output | `Rejected` |

Failure must never look like empty success or "not found". Unknown stays
unknown. The OpenAI Responses adapter (`openai_responses.rs`) is the worked
example: `OpenAiFailureClass -> AgentError`, strict completed-only status,
schema-validated payload, action count/field bounds, secret redaction.

## Capabilities

Descriptors carry `capabilities: Vec<String>` (stable JSON). Every string
must come from `ProviderCapability` (`reasoning`, `structured-response`,
`failure-diagnosis`, `navigation`, `proposal`, `tool-proposal`,
`deterministic`, `orientation`, `build-check`). No vendor-branded spellings.
Claim only what you implement; the harness runs the subset you claim.

## Credentials

- Read credentials from the environment (or explicit config), never from
  `AgentContext`. Credential presence means "configured enough to attempt
  use", never validity, entitlement, or reachability.
- Descriptors expose `credential_source` (e.g. `environment:OPENAI_API_KEY`),
  never the value. `Debug`/`Display`/errors/logs/status/history must redact.
- Authentication grants **nothing**: a valid key plus an `ExecuteCommand`
  proposal still executes nothing without Omen/Tethers authority.

## Timeouts and retries

- Every call is bounded (`TimeoutProvider`, default 30s). Never hang; never
  block the shell; sleeps only when explicitly bounded.
- **No automatic retry.** One user request is one provider call, even on
  transport failure. Tell the user a retry is available; do not spend their
  money silently.

## Bounds

Prompts, conversation (recent turns only), context JSON, response bodies,
messages, references, and action lists are all bounded or rejected. Do not
build an unbounded escape hatch around Omen's context doctrine.

## Registry

`ProviderRegistry::new()` must stay cheap: no network, no key validation, no
model probe, no credits. Remote truth is established at call time.

## Conformance harness

`omen_agent::conformance`: `ConformanceScenario` (21 cases),
`check_scenario`, `run_conformance`. `omen_agent::canary`:
`ConformanceProvider`/`ConformanceMode` — the deterministic hostile fixture
(no network, counted invocations, captured requests, scripted sequences).

To conform a new provider:

1. Drive it per scenario you claim (canary modes or scripted mock transport).
2. Call `check_scenario` — it judges Omen-side shapes only.
3. Add the provider to `provider_conformance_tests.rs` (subset) and, for
   session coherence, to `g1_provider_switching_tests.rs`.

## Normal CI vs live

Normal CI uses mocked transports only (`HttpPost` fakes). Live tests are
`#[ignore]`d, opt-in, single-call, and assert no-leak plus a deterministic
zero-call query. Never spend credits in CI.

## Forbidden

Executing proposals. Bypassing Tethers/authority. Silent retries. New shell
grammar (`:luna`, `@openai`, `provider://`, …). Provider-specific commands.
Vendor ontology (`finish_reason`, `stop_reason`, queue states, request IDs,
safety names, lifecycle terms) escaping the adapter as Omen meaning.
Branded capabilities. Secrets in any output. A second commercial provider
"for coverage" — the seam is proven by diagnostic + Luna + canary.
