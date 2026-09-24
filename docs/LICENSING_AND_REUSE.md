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
