# 11. Changing things safely

Reads and writes are not the same kind of operation.

Omen tries to make that difference visible before execution.

## Consequences first

Where Omen knows likely effects, it should expose them before the action runs.

Unknown effects remain UNKNOWN.

## Structural mutation

ThreadMoth is Omen's deterministic structural mutation boundary.

A safe structural change should preserve the selected candidate, pre-image, post-image, cryptographic hashes, and refusal when expected state no longer matches.

Omen can use ast-grep to discover candidates.

The mutation should still pass through the deterministic mutation boundary.

## Plan and preview

Omen 0.8 extends the same principle to composition.

A consequential composition should be plannable before it is runnable.

Planning itself must not mutate.

## Capability is not permission

The fact that Omen knows how to write a file does not mean the current actor may write it.

The fact that a recipe mentions a mutation does not grant it.

A previous admission is not a reusable permission slip.

Tethers remains the authority boundary.

## Paste Guard

Multi-line paste can execute faster than a human can visually parse it.

Paste Guard turns consequential paste into something reviewable before execution.

## The safe-change habit

    discover
      ↓
    inspect/plan
      ↓
    check effects and authority
      ↓
    mutate deterministically
      ↓
    verify
      ↓
    preserve evidence
