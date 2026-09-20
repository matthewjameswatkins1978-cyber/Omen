# 2. Why Omen

The shell was built around a relationship between a human and a computer.

Programs printed text. Humans read it, remembered what they had done, and decided what to do next.

Then we put AI agents on the other side of the same interface.

The result is often strange.

A model capable of reasoning about a large codebase may still spend time asking for the working directory, Git state, tool versions, process lists and long log files. It may scrape coloured terminal output, reread a transcript, infer whether a process is alive, guess whether an old test result still applies, and consume expensive context rediscovering facts the computer already knows.

## The central Omen question

Omen asks:

> What should the machine expose so a human or agent does not have to rebuild the computer from strings?

A normal shell is often excellent at answering:

> What did this program print?

Omen wants to make additional questions cheap and explicit:

- What operation actually ran?
- What resource did it affect?
- What changed?
- What is still current?
- What has become dirty or stale?
- Which process or service is this?
- What evidence supports this claim?
- What assurance did the platform really provide?
- Is this capability available?
- Is this caller currently admitted to use it?
- What part of the result is deterministic, observed, inferred or unknown?

## Keep the shell, change the machine-facing side

The human should still be able to type:

    git status
    cargo test
    cd crates
    rg TODO src

Omen does not want to turn every ordinary action into a ceremony.

The larger change is underneath.

Instead of making the terminal transcript the only practical record of reality, Omen maintains typed identities, Facts, observations, artifacts, executions, semantic results and process state.

The terminal becomes a view onto that reality.

It is no longer the only place reality exists.

## Why now

AI makes the weakness of implicit machine state expensive.

A human can glance at a prompt, remember the previous command and intuit that a test result is probably old.

An AI agent may need those facts placed into context repeatedly.

That means an interface designed for humans can impose a tax on machine reasoning.

Omen is an attempt to keep the best qualities of the shell while making the underlying computer dramatically more legible to software agents.

## The shortest statement

> Do not teach the AI to operate a terminal better when the machine can expose the truth directly.
