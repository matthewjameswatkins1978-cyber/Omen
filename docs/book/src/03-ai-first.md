# 3. AI-first engineering and the Harness Tax

AI-first does not mean placing a model inside every feature.

In Omen it usually means the opposite.

Use intelligence where intelligence adds value.

Use deterministic machinery where the answer is mechanically knowable.

## Harness Tax

Harness Tax is Omen's working term for avoidable token, latency, compute and attention cost introduced by the environment around an agent rather than by the intrinsic difficulty of the task.

Examples include repeated rediscovery of workspace state, feeding enormous stdout transcripts into a model, asking a model to interpret facts a parser already knows, losing useful state between turns, and making the model remember policy the environment could enforce.

Not all harness cost is waste.

Verification, containment, evidence and recovery can be expensive and still be valuable.

The target is avoidable excess.

## Deterministic before generative

A simple Omen law is:

> Spend tokens in proportion to uncertainty.

Questions such as where am I, what branch is active, where is this symbol defined, did this process exit, is this Fact current, and which backend is active should not require a general-purpose model when deterministic machinery can answer them.

Judgement belongs higher up:

- Why might these failures be related?
- Which trade-off is preferable?
- Is this refactor worth doing?
- What should we investigate next?

## Context is a resource

A large context window is not a reason to fill it.

Omen prefers bounded summaries, stable references, lazy detail retrieval, artifacts outside conversational context, machine deltas instead of repeated rediscovery, and progressive capability discovery.

Large results become references rather than conversational floods.

## Consequential truth must be explicit

Omen tries to avoid several kinds of convenient fiction:

- OBSERVED must not become ENFORCED because the UI looks reassuring.
- STALE must not become CURRENT because a result is probably still right.
- an available capability must not become an admitted permission;
- an inference must not become a Fact;
- an unavailable historical delta must not become an invented empty change set.

Unknown is allowed.

## The deeper engineering rules

Omen's AI-first doctrine includes one semantic authority for each important fact, bounded waiting, failure returning control, strong types and schemas, constraints below the model, zero-model paths that really use zero model calls, explicit state transitions, evidence over confidence, cross-platform defaults, recovery, and tests that prove claims.

The result is useful:

> The more agent-native the lower layers become, the less AI they often need.
