# 6. Speaking Omen

Omen deliberately avoids making every interaction look the same.

## Ordinary executable

No prefix:

    cargo test auth
    git status --short
    rg TODO src

Meaning: run this program with these arguments.

## Semantic action

Colon:

    :status
    :doctor
    :def SessionToken
    :refs SessionToken
    :packages

Meaning: perform an Omen operation whose semantics Omen understands.

## Typed reference

At sign:

    @last
    @failed
    @errors
    @last.artifact
    @last.changed
    @fact.test
    @service.dev

Meaning: refer to a known object or relation in Omen state.

Think of references as nouns.

## AI reasoning

Question mark:

    ? what should I inspect next?
    ? why might these errors be connected?

Meaning: apply judgement.

The AI lane is explicit by design.

AI is advisory unless another deterministic authority admits an operation.

## Why the lanes matter

The grammar keeps command, semantic operation, object/reference and judgement visibly distinct.

That makes it harder for fuzzy interpretation to silently cross a consequence boundary.

## Windows paths

Omen's grammar preserves Windows path separators rather than pretending every backslash is Bash syntax.

    C:\Users\Matthew\project
    \\server\share\project
    "C:\Program Files\App"

Cross-platform support means preserving real platform meaning behind explicit boundaries.
