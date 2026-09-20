# 4. Ten-minute quick start

This chapter teaches the Omen habit before it teaches the whole command set.

The accepted 0.7 shell supports ordinary commands, semantic actions, typed references and an explicit AI lane.

The 0.8 development branch additionally introduces Machine Contract discovery such as orient, capabilities and describe. Those discovery surfaces remain in development until 0.8 is accepted.

## 1. Start by asking where you are

In the 0.8 development branch:

    omen --machine orient

The purpose is not a giant status dump.

It is a small bootstrap map.

For a human interactive session, begin with:

    :status

Then:

    :doctor

The first tells you about current Omen/workspace state.

The second asks whether the substrate itself appears healthy.

## 2. Run something ordinary

    git status
    cargo test

Ordinary commands remain ordinary commands.

## 3. Inspect instead of immediately rerunning

If something fails:

    :show @failed

Then:

    :inspect @last

If a Fact or result looks wrong or old:

    :why <reference>

The habit is:

> inspect before retrying.

## 4. Ask Omen a semantic question

    :symbol SessionToken
    :def SessionToken
    :refs SessionToken

For syntax structure:

    :structure "fn $NAME($$$ARGS) { $$$BODY }" rust

These operations prefer semantic machinery rather than asking a model to guess from text.

## 5. Use AI when the question becomes judgement

    ? what would you inspect next?

or:

    ? why might these failures be related?

The question mark is explicit.

## 6. Learn an unfamiliar Omen capability

In the 0.8 development branch:

    omen --machine capabilities semantic
    omen --machine describe semantic.definition

An unfamiliar agent should learn Omen from Omen itself, a little at a time.

## What to remember

    state
      ↓
    structured result
      ↓
    provenance/evidence
      ↓
    semantic understanding
      ↓
    judgement

Use the cheapest trustworthy layer that can answer the question.
