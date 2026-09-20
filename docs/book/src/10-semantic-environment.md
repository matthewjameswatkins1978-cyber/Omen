# 10. Understanding code

Omen 0.7 introduced the Semantic Environment.

Not every question about source code is a text-search question.

## Use the right semantic layer

    text occurrence       -> ripgrep
    syntax structure      -> ast-grep
    symbol/reference      -> LSP or SCIP
    structural mutation   -> ThreadMoth
    package/task metadata -> native structured metadata
    raw execution         -> fallback

## Text search

If the question is where does this string occur, ripgrep is excellent.

## Structural search

If the question is where is this syntax structure, text becomes unreliable.

A function-looking string inside a comment is not a function node.

Interactive surface:

    :structure <pattern> [language]

## Symbols

For definition/reference questions use symbol intelligence rather than grep-and-hope.

    :symbol <query>
    :def <symbol>
    :refs <symbol>

Live LSP such as rust-analyzer can provide current semantic information.

SCIP provides an indexed/offline semantic path.

Omen tracks provenance and freshness so stale indexed knowledge does not quietly masquerade as current.

## Packages and tasks

Omen 0.7 understands structured package/workspace metadata across multiple ecosystems.

    :packages
    :tasks

The goal is not to become another package manager.

The goal is to understand the developer environment without forcing an agent to scrape human-formatted output.

## Semantic questions can be zero-model

A question such as where is SessionToken defined does not inherently require a language model.

If Omen's semantic providers can answer it, the deterministic path should answer it.
