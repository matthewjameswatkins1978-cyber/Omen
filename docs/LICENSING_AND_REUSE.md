# Licensing and Open-Source Reuse Policy

## Status

This document records Omen's current licensing direction and dependency/reuse policy.

The final project licence has **not yet been selected**. Do not add or replace a repository `LICENSE` file merely from this document.

Current direction:

- Omen should remain commercially usable open source.
- Businesses should be free to use Omen and build proprietary products around it.
- Matthew Watkins / Biscuit Logic attribution and provenance should remain durable.
- Distributed improvements to Omen itself should preferably remain open.
- MPL-2.0 is the current leading candidate because its file-level copyleft fits that shape well.
- Branding and trademark policy are separate from source-code licensing and should be handled separately when formalised.

Until the project licence is formally chosen, treat this as architectural and dependency policy, not as a grant of rights beyond the licences already attached to code.

## Core rule

> **Licence compatibility is an engineering constraint, not a reason to rewrite good open-source code by default.**

Omen is reuse-first. If mature open-source machinery already solves a commodity problem well, prefer legitimate reuse over reimplementation when the licence, integration shape, provenance, maintenance cost, binary/startup impact, and distribution obligations are acceptable.

A copyleft licence is not automatically a rejection.

Likewise, "open source" is not automatically compatible with every integration shape.

Review the actual combination.

## Dependency and code-reuse classes

### Permissive dependencies

MIT, Apache-2.0, BSD-family, ISC, Zlib and similar permissive licences are generally the simplest fit, subject to normal provenance and notice requirements.

### MPL-2.0 code

MPL-2.0 is explicitly admissible for consideration.

Its file-level copyleft can coexist with proprietary code in a larger work while requiring distributed modifications to MPL-covered files to remain available under MPL terms.

Do not reject MPL code merely because it is copyleft. Check:

- whether Omen is linking to, depending on, copying from, or modifying MPL-covered files;
- what notices and source-availability obligations apply;
- whether the proposed integration preserves clean provenance;
- whether the integration remains compatible with Omen's final project licence once selected.

Official reference: https://www.mozilla.org/MPL/2.0/

### EUPL code

EUPL-licensed code is also admissible for consideration.

Do not classify EUPL as "design reference only" by default.

For EUPL code, review the concrete integration and redistribution obligations before copying, adapting, linking, vendoring, or embedding it. Where the obligations fit Omen's chosen licence and distribution model, implementation reuse is allowed.

Official reference: https://commission.europa.eu/content/european-union-public-licence_en

## eza and modern `ls` work

eza is a valuable source of both implementation experience and UX design for Omen's human-facing `ls`:

- readable metadata;
- Git state;
- tree views;
- sorting and grouping;
- humanised sizes and times.

Its EUPL licensing is **not an automatic veto**.

When implementing Omen's listing surface, inspect whether direct reuse or adaptation of eza code materially reduces work. If it does, perform a bounded licence/provenance review and reuse it when the resulting obligations are acceptable.

Do not rewrite mature code merely to avoid doing the licence analysis.

The architectural boundary remains unchanged:

> **Borrow proven machinery; Omen owns semantics, truth, composition and projection.**

Omen's canonical listing result should remain typed runtime truth such as `FileEntry[]`. Any borrowed implementation or presentation machinery sits beneath or beside that semantic authority rather than replacing it.

## Audited tool/source reuse matrix — 25 September 2026

This matrix audits the current Omen portable-shell and first-class-tool candidates against the **current MPL-2.0-leading direction**.

It is deliberately stricter than asking whether Omen may execute a tool.

- **APPROVED** — direct source reuse, adaptation, linking or dependency use is compatible with the current direction, subject to normal file-level licence checks, notices and provenance.
- **APPROVED WITH CONDITIONS** — reuse is compatible in principle, but the specific integration has extra copyleft/provenance/source obligations that must be preserved.
- **EXTERNAL / REFERENCE ONLY BY DEFAULT** — Omen may execute, integrate with, test against and study the project, but its copyleft-covered source should not be copied into the MPL-oriented Omen core without a deliberate licensing architecture decision.

| Project / code source | Upstream licence | Omen source-reuse status | Practical rule |
|---|---|---|---|
| **uutils coreutils** | MIT | **APPROVED** | Preferred source for portable core-utility mechanics. Reuse crates/APIs or adapt source where useful; retain MIT notice/provenance. |
| **ripgrep / `ignore` / `globset`** | Unlicense OR MIT | **APPROVED** | Prefer the MIT option for a conventional provenance trail. Strong candidate for traversal, ignore and glob substrate. |
| **bat** | MIT OR Apache-2.0 | **APPROVED** | Library/source reuse is allowed. Preserve the selected licence and any applicable NOTICE/third-party material. |
| **zoxide** | MIT | **APPROVED** | Algorithm/source reuse is allowed with normal attribution. Exact `cd` semantics remain Omen-owned. |
| **fd** | MIT OR Apache-2.0 | **APPROVED** | Source reuse is allowed. Prefer shared Omen/ripgrep traversal machinery where architecturally cleaner. |
| **dust** | Apache-2.0 | **APPROVED** | Source reuse is allowed with Apache notice/attribution obligations. |
| **eza** | EUPL-1.2 | **APPROVED WITH CONDITIONS** | EUPL 1.2 explicitly lists MPL 2.0 as a Compatible Licence. Direct adaptation into an MPL-based Omen derivative is therefore available through the EUPL compatibility mechanism. Keep exact source provenance, notices and modification history. Do not simply relabel an untouched standalone eza copy as MPL. If a specific eza file also carries MIT, prefer that simpler grant where applicable. |
| **fzf** | MIT | **APPROVED** | Source/algorithm reuse is allowed with normal attribution. |
| **jq** | MIT for jq; some bundled/third-party material uses other permissive licences such as ICU | **APPROVED, FILE-CHECK REQUIRED** | jq-owned MIT source is reusable. Check the exact file before copying third-party/bundled components. |
| **delta** | MIT | **APPROVED** | Source reuse is allowed with normal attribution. |
| **GitHub CLI (`gh`)** | MIT | **APPROVED** | Source reuse is allowed with normal attribution. |
| **Cargo** | MIT OR Apache-2.0, with separately tracked third-party material | **APPROVED, FILE-CHECK REQUIRED** | Cargo-owned source is reusable under either offered licence. Check third-party/file-specific notices before copying. |
| **uv** | MIT OR Apache-2.0 | **APPROVED** | Source reuse is allowed under either offered licence, subject to ordinary file-level provenance checks. |
| **Nushell** | MIT | **APPROVED** | Source reuse is allowed. This includes useful shell/data-structure mechanisms where they fit Omen rather than importing Nushell's language wholesale. |
| **PowerShell** | MIT for the PowerShell repository | **APPROVED, FILE-CHECK REQUIRED** | PowerShell-owned source is reusable. Some dependencies/bundled packages have different terms, so copy only from files whose licence is clear. |
| **Git** | GPL-2.0 (main project; some parts have compatible different licences) | **EXTERNAL / REFERENCE ONLY BY DEFAULT** | First-class execution/integration is fine. Do not copy GPL-covered Git implementation code into MPL-oriented Omen files. A deliberate larger-work/GPL distribution strategy is possible in principle but does not fit Omen's current licensing goal. |
| **Fish** | Primarily GPL-2.0, with LGPL/MIT/PSF components | **EXTERNAL / REFERENCE ONLY BY DEFAULT** | Study behaviour and integrate externally. Do not blanket-copy Fish core. A specifically MIT/PSF-licensed file may be reviewed independently if its licence is unambiguous. |
| **GNU Bash** | GPL-3.0-or-later | **EXTERNAL / REFERENCE ONLY BY DEFAULT** | Behaviour/compatibility reference and external execution are fine. Do not copy Bash GPL code into the MPL-oriented Omen core under the current direction. |

### Important compatibility conclusions

1. **eza is not blocked.** EUPL 1.2 Article 5 contains a compatibility clause, and the EUPL appendix explicitly names **MPL 2.0** as a Compatible Licence. This means eza's EUPL is a real reuse path, not merely a design-reference licence.
2. **MPL 2.0 can coexist with permissive code easily.** MIT, Apache-2.0 and similar permissive code can be combined with MPL-covered Omen while preserving their notices.
3. **GPL code is the real boundary for Omen's present product direction.** MPL 2.0 has a mechanism for Larger Works with GPL-family code, but that route requires distribution under the GPL terms as well. That is legally useful, but it is not the current Omen goal of file-level copyleft with proprietary products allowed around Omen.
4. **External integration is different from source incorporation.** Omen can discover, execute, supervise and consume structured output from Git, Fish, Bash or any other appropriately installed tool without importing that tool's source licence into Omen.
5. **File-level truth outranks repository-level shorthand.** Before copying source, inspect the exact file/module and any third-party notices. Repository headline licences are not permission to ignore a differently licensed embedded file.

### Upstream licence references used for this audit

- uutils coreutils: https://github.com/uutils/coreutils
- ripgrep and crates: https://github.com/BurntSushi/ripgrep
- bat: https://github.com/sharkdp/bat
- zoxide: https://github.com/ajeetdsouza/zoxide
- fd: https://github.com/sharkdp/fd
- dust: https://github.com/bootandy/dust
- eza: https://github.com/eza-community/eza
- EUPL 1.2 compatibility clause and appendix: https://eupl.eu/1.2/en
- fzf: https://github.com/junegunn/fzf
- jq: https://github.com/jqlang/jq
- delta: https://github.com/dandavison/delta
- GitHub CLI: https://github.com/cli/cli
- Cargo: https://github.com/rust-lang/cargo
- uv: https://github.com/astral-sh/uv
- Nushell: https://github.com/nushell/nushell
- PowerShell: https://github.com/PowerShell/PowerShell
- Git: https://github.com/git/git
- Fish: https://fishshell.com/docs/current/license.html
- Bash: https://www.gnu.org/software/bash/
- MPL 2.0 compatibility guidance: https://www.mozilla.org/en-US/MPL/2.0/FAQ/

### Agent rule for implementation

Before reimplementing commodity shell/tool behaviour, consult this matrix.

If the relevant upstream is **APPROVED**, reuse should be actively considered before writing a fresh implementation.

If it is **APPROVED WITH CONDITIONS**, perform the bounded licence/provenance check and reuse it when those conditions are satisfied.

If it is **EXTERNAL / REFERENCE ONLY BY DEFAULT**, do not copy its copyleft-covered source into Omen without an explicit licensing decision.

## Provenance requirements

When Omen borrows or adapts code, keep provenance visible.

At minimum, record where applicable:

- upstream project and repository;
- exact licence and version;
- source file/module or crate used;
- whether code is linked, depended upon, copied, adapted, or vendored;
- upstream commit/tag/version when practical;
- required copyright and licence notices;
- any material local modifications.

Do not imply that borrowed work originated in Omen.

## cargo-deny policy

`deny.toml` currently uses a deliberately conservative permissive-licence allow-list.

That is a mechanical gate, not a statement that every other open-source licence is forbidden.

Until Omen's final project licence is selected:

1. do not broadly relax the allow-list just to make a build pass;
2. review a proposed MPL/EUPL or other copyleft dependency in context;
3. add a narrow, documented exception or policy change only when that dependency is intentionally accepted;
4. preserve the reason and provenance alongside the change.

Once Omen's own licence is formally selected, revisit `deny.toml` so the automated policy reflects the actual legal/reuse policy rather than an older permissive-only default.

## Current licensing direction

The present working preference is:

> **Commercially usable open source with durable attribution, proprietary products allowed around Omen, and distributed modifications to Omen itself kept open where practical.**

MPL-2.0 currently appears to fit that goal particularly well, but it remains a candidate until Matthew explicitly locks the project licence.

No agent should silently make that final licensing decision.
