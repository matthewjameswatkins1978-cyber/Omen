# 5. The Omen mental model

When using Omen, keep four questions separate.

1. What did I ask the machine to do?
2. What did Omen actually observe?
3. What does Omen currently consider established?
4. Why does it consider that established?

## Facts are not memories

A Fact is not true merely because Omen stored it.

A Fact is current while the witnesses that support it remain valid.

Imagine cargo test auth passes. Then src/auth.rs changes.

Omen should not keep presenting the old result as current proof of the new source.

The result may become DIRTY or STALE.

That does not mean the old test secretly failed. It means its evidentiary relationship to the current workspace changed.

## Observation, Fact, Inference, Artifact

Observation is something Omen directly saw.

Fact is a deterministic claim established under Omen's rules and current witnesses.

Inference is a conclusion produced by an adapter or model.

Artifact is immutable supporting evidence, often stored in CAS.

## Validity and assurance are different axes

Validity asks whether this still applies to current state.

Examples:

CURRENT
DIRTY
STALE
SUPERSEDED
HISTORICAL

Assurance asks what kind of support exists behind the claim.

Examples include:

DETERMINISTIC
VERIFIED
ENFORCED
OBSERVED
CLAIMED
INFERRED
UNKNOWN

## Personal history and shared machine reality

Your @last is yours.

It should not suddenly become another agent's command because that agent changed the workspace in the background.

At the same time, the workspace really can change under you.

Omen therefore separates session/actor-scoped interaction history from shared machine truth.

## The operator habit

> Ask for state before guessing. Ask for provenance before arguing with state. Re-run only when revalidation is actually required.
