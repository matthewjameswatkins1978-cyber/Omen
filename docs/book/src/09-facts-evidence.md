# 9. Facts, evidence and time

Omen's knowledge system exists because developer truth changes.

A statement can be correct at 10:00 and unsupported at 10:05 without ever having been false.

## Dirty is useful

Suppose a test passes and then a relevant source file changes.

The old result should not remain CURRENT.

Omen may mark it DIRTY.

DIRTY means something relevant changed and current proof now requires revalidation.

It does not mean the old result was a lie.

## Why not rerun automatically?

Automatic revalidation can be expensive, slow, state-changing, networked, authority-sensitive or irrelevant to the user's current work.

Omen therefore prefers explicit invalidation.

## Provenance

When a Fact is surprising, ask why.

    :why <reference>

A useful answer reveals what established the Fact, what dependencies it relied on, which generation was observed, which generation exists now, and what artifact/execution supports the claim.

## Artifacts

Large evidence belongs outside the conversational surface.

    omen artifact inspect <hash>
    omen artifact read <hash>
    omen artifact read <hash> --offset 8192 --length 8192

Read the part you need.

Keep the rest addressable.

## History is not authority

A previous execution helps reconstruct what happened.

It does not automatically grant permission to repeat a mutation now.

Likewise @last is scoped to the current interactive actor/session rather than becoming a global moving pointer.

## Shared reality

Omen 0.4 established shared runtime truth across processes.

Multiple clients can observe the same underlying workspace/process reality while retaining their own interaction history.
