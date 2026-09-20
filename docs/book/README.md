# The Omen Book

## A guide to a shell that can explain the machine

The Omen Book is the readable manual.

It is designed to be read from the beginning, but each chapter should also be useful on its own.

This structure deliberately follows a lesson from documentation ecosystems such as Rust, Fish, Nushell, Bash and PowerShell: learning material and exact reference material are different jobs.

The Book teaches the model.

The Reference answers exact questions.

Omen itself should eventually answer machine-facing questions through its own Machine Contract.

## Current edition

Accepted product baseline covered:

Omen 0.7 — Semantic Environment

Development material covered and clearly marked:

Omen 0.8 — Composition & Discoverability

## Start reading

Open:

src/00-preface.md

or use:

src/SUMMARY.md

as the table of contents.

## Formats

The canonical source is Markdown.

The intended publication formats are:

- browsable HTML;
- local/offline HTML;
- one printable full-book HTML page;
- downloadable PDF;
- EPUB where practical.

book.toml establishes an mdBook-compatible source layout. Publication automation can be added without changing the prose source.

## Documentation doctrine

1. Current behavior, development behavior and design targets must never be blurred.
2. Human prose is not a second semantic authority.
3. Command/reference tables should increasingly be generated or validated against canonical Omen definitions.
4. Examples should prove one idea at a time.
5. Explain why a mechanism exists, not only the incantation needed to operate it.
6. Unknown is preferable to confident documentation fiction.
