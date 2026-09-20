# Errors, validity and assurance

## Validity

CURRENT — supported by current witnesses/generation.

DIRTY — a dependency changed; revalidation required before current proof.

STALE — informative perhaps, but not current.

SUPERSEDED — replaced by newer knowledge.

HISTORICAL — retained as past evidence.

## Assurance

DETERMINISTIC — mechanically established.

VERIFIED — explicitly checked.

ENFORCED — actually enforced by platform/runtime.

MEDIATED — controlled through an intermediate boundary.

OBSERVED — directly observed without strongest enforcement.

BEST_EFFORT — attempted without full hard guarantee.

CLAIMED — reported without stronger verification.

INFERRED — derived by inference.

UNKNOWN — insufficient evidence for stronger claim.

## Machine errors

0.8 machine-facing errors should increasingly carry a stable code, operation, state_changed, retryable, required capability/context and legitimate next actions.

Important concepts include:

AUTHORITY_REQUIRED  
SEMANTIC_RESULT_STALE  
DELTA_UNAVAILABLE  
CAPABILITY_NOT_FOUND  
RECIPE_NOT_FOUND

A prose message may explain an error. It must not be the only machine identity of a consequential failure.
