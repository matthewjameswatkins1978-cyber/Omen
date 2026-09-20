# 1. Why shells exist

For decades, one of the most effective ways to operate a computer has been a line of text and a cursor.

That survived windows, icons, IDEs, web applications, touchscreens and voice interfaces for a reason.

A shell is dense.

A few characters can express an exact operation. The operation can be repeated, copied, stored, scripted, inspected and sent over a remote connection. Small programs can be combined without each program needing to know about every other program.

## The shell as command interpreter

At its simplest, a shell accepts a command and arranges for the operating system to run it.

Traditional Unix shells grew into more than launchers. They became programming languages with variables, quoting, redirection, pipelines, functions, conditionals and loops. They also accumulated interactive conveniences such as history, command-line editing, aliases and job control.

The Bourne shell, sh, became a major part of the Unix lineage. Bash later built on that lineage while borrowing ideas from other shells.

The important point for Omen is not historical trivia. It is that the shell solved several problems exceptionally well:

- precise expression of intent;
- composition of small tools;
- automation;
- reproducibility;
- remote operation;
- low-bandwidth use;
- a durable textual record;
- access to almost every program on the machine.

## Why people still use shells

A graphical interface is excellent when recognition is easier than recall: choosing a colour, arranging windows, scanning photographs, moving objects spatially.

A shell is excellent when the desired operation is already precise.

The shell can encode that operation directly.

That property became even more important for software development because developer tools already expose powerful command-line interfaces.

Git, compilers, package managers, build systems, debuggers, containers and remote systems all meet naturally at the command line.

## The cost of the traditional model

The traditional shell also carries assumptions.

Its primary currency is usually strings:

- command text;
- stdin;
- stdout;
- stderr;
- environment variables;
- exit codes;
- terminal state.

Humans became very good at reconstructing machine state from those strings.

We read the output, remember what we ran, infer what changed, scroll backwards, rerun commands, and build a mental model.

That works remarkably well.

It is also the crack through which Omen enters.

## Omen is not a rejection of the shell

Omen does not begin with the claim that shells failed.

It begins with almost the opposite claim:

> Shells survived because their directness and composability are extremely good.

The question is what should change when the other operator is no longer always a human reading a terminal.
