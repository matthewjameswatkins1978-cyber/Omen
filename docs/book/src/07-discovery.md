# 7. Orientation and discovery

Status: Omen 0.8 development work.

Omen 0.8 adds a principle that matters especially for agents:

> An agent should not need to be taught Omen. Omen should teach the agent just enough to do the current job.

## orient

The development bootstrap surface is:

    omen --machine orient

It should answer only the first questions: what contract is this, what workspace is this, what broad capabilities exist, what important references exist, and where should I ask for more?

It should not dump every schema.

## Progressive disclosure

    orient
      ↓
    capabilities
      ↓
    capabilities semantic
      ↓
    describe semantic.definition

The client begins with a map and loads detailed contracts only when the task touches them.

## Static contract vs live context

Static contract truth includes capability definitions, schemas, effect semantics, recipe definitions and contract version.

Live context truth includes provider availability, current backend, current admission, workspace generation, services, dirty state and session references.

The static contract can be fingerprinted and cached.

Live state changes without rewriting what a capability means.

## available is not admitted

AVAILABLE asks whether Omen can perform this kind of operation here.

ADMITTED asks whether this caller may perform it now.

Tethers remains the authority layer.

## recipes

The how surface exposes advisory recipes.

Recipes explain useful sequences. They do not execute and do not grant authority.

## The larger goal

Machine discovery should make Omen cheaper to learn than a traditional shell transcript.

The best first response is not the largest response.

It is the smallest dependency-complete response.
