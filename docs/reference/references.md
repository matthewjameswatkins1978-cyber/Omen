# Typed References

Current interactive grammar includes:

@last  
Most recent applicable operation in the current interactive session.

@last.failed  
Most recent failing target in the session.

@last.artifact  
Primary artifact of the previous operation.

@last.changed  
Resources changed by the previous operation.

@last.output  
Bounded output slice.

@failed  
Recent failed target.

@errors  
Known current compiler/runtime errors where available.

@fact.<name>  
Named Fact reference.

@service.<name>  
Managed service/process reference.

## Logical URI families

Architectural resource families include:

    workspace://
    tool://
    fact://
    observation://
    inference://
    artifact://
    proc://
    secret://
    net://
    actor://
    trace://

Omen 0.7 also introduced semantic symbol/package identities.

## Scoping

@last is session/actor-scoped interaction history.

Shared machine state is not.
