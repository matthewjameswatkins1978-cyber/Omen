# Building and publishing the Omen Book

The source tree is mdBook-compatible, but Omen does not currently require mdBook as a build dependency.

That is deliberate. Documentation tooling must not become a runtime dependency.

## Desired outputs

The eventual documentation release should publish:

1. The Omen Book as normal HTML.
2. A single printable full-book HTML page.
3. A downloadable PDF.
4. EPUB where the toolchain remains low-maintenance.
5. A local/offline copy installed with Omen.
6. The exact Omen Reference as searchable HTML and terminal help.

The ideal product experience is eventually:

    omen help
    omen help facts
    omen help semantic.definition
    omen docs

where omen docs opens the local Book.

## Source-of-truth rule

Do not manually maintain generated command or schema facts in multiple places.

The Book may explain a capability.

The Reference should increasingly project the canonical Machine Contract, CLI definitions, schemas and structured errors.

If generated reference and prose disagree, the canonical executable contract wins and the prose must be repaired.
