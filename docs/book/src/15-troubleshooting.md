# 15. Troubleshooting

When Omen behaves strangely, climb the evidence ladder rather than jumping immediately to speculation.

## 1. Status

    :status

## 2. Doctor

    :doctor
    omen doctor

## 3. Inspect

    :inspect @last
    :show @failed

## 4. Provenance

    :why <reference>

## 5. Raw evidence

Inspect the artifact or structured provider output.

## 6. Judgement

Only then, if the remaining problem is ambiguous:

    ? what could explain this?

## Common conditions

A Fact is DIRTY: inspect which witness changed before rerunning everything.

Semantic result is STALE: useful perhaps, but not current.

Provider unavailable: do not silently pretend a weaker method has identical semantics.

Authority refused: inspect current admission instead of retrying the same forbidden operation.

Shared runtime disconnects: degrade capability before degrading truth.

Something appears hung: bounded waiting policy means a long silent wait is a bug.

## Retry rule

A retry should change something: evidence, strategy, state, provider, decomposition or environment.
