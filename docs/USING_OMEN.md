# Using Omen

## What actually happens when you open it?

Omen is still a shell.

You do not open it and find an AI chat window asking what project you would like to work on today.

You get a terminal.

Just a rather more observant one.

A healthy Omen session might open looking something like this:

```text
~/Projects/Omen  main ✓
[shared] >
```

That is deliberately sparse.

The first line tells you where you are and whether Omen sees anything worth interrupting you about.

The second line tells you which runtime mode you are operating in:

```text
[shared] >
```

means the shell is attached to the Omen shared runtime.

If the daemon is unavailable:

```text
~/Projects/Omen  main ✓
[standalone] >
```

The shell still works.

It simply stops pretending it knows about the rest of the machine.

That distinction is very Omen.

---

## The prompt has three moods

Normal:

```text
~/Projects/Omen  main ✓
[shared] >
```

Something relevant has changed:

```text
~/Projects/Omen  main ! 2 dirty
[shared] >
```

The last operation failed:

```text
~/Projects/Omen  main ✕
[shared] >
```

No dashboard.

No spinning AI orb.

No weather report.

No GPU temperature unless you actually ask for it.

The prompt exists to answer:

> **Where am I, and is there anything I should know before I type?**

---

# Ordinary shell work

Most of the time you use Omen exactly the way you already use a terminal.

The difference is that Omen is watching the *meaning* of the work as well as the text produced by it.

Here are ten completely normal things.

---

## 1. Move around and look at files

In Bash, PowerShell, Fish, Nushell or almost anything else:

```text
cd src
ls
```

In Omen:

```text
cd src
ls
```

Nothing exotic happens merely because Omen exists.

The prompt changes:

```text
~/Projects/Omen/src ✓
[shared] >
```

Completion already knows about the local workspace, so typing:

```text
cd cr
```

may ghost-suggest:

```text
cd crates/
```

### What Omen adds

A conventional shell knows that your current directory string changed.

Omen treats the workspace as a resource.

That matters later when tests, processes, Facts and agent activity are attached to it.

You are not standing in an anonymous folder.

You are standing inside a known machine context.

---

## 2. Check Git

Normal shell:

```text
git status
```

Omen:

```text
git status
```

You can use Git exactly as intended.

A clean result might simply return you to:

```text
~/Projects/Omen  main ✓
[shared] >
```

Omen is deliberately quiet when nothing interesting happened.

You can also ask the semantic layer:

```text
:status
```

That is a different question.

`git status` asks Git:

> What does Git think?

`:status` asks Omen:

> What is the condition of this workspace?

That can eventually include things Git knows nothing about:

```text
Workspace    Omen
Git          main · 2 modified
Facts        31 current · 2 dirty
Services     dev running
Last failure cargo test auth
Runtime      shared
```

### What Omen adds

A normal shell has many independent truths scattered between tools.

Omen gives them somewhere to meet without pretending they are the same thing.

---

## 3. Search for something

Normal shell:

```text
rg "TODO" src
```

Omen:

```text
rg "TODO" src
```

Again, no need to relearn the command.

If the output is small, you see it.

If it is enormous, Omen can retain the full result as an artifact while keeping the terminal useful:

```text
83 matches · 19 files

src/auth.rs:44:    // TODO
src/runtime.rs:119: // TODO
...

full output:
artifact://sha256/8c93...
```

### What Omen adds

The terminal does not have to become the storage system for evidence.

Huge output can become:

```text
summary + reference
```

rather than:

```text
terminal avalanche
```

An agent can inspect the same artifact later without somebody copying 40,000 lines out of scrollback.

---

## 4. Run tests

Normal shell:

```text
cargo test auth
```

That works unchanged in Omen:

```text
cargo test auth
```

But Omen also has a semantic version:

```text
:test auth
```

The distinction is deliberate.

```text
cargo test auth
```

means:

> Run Cargo with these arguments.

```text
:test auth
```

means:

> Perform Omen's semantic test operation for `auth`.

A successful operation might leave behind something like:

```text
✓ auth
13 passed · 0 failed · 1.12s
fact://test/auth = CURRENT · VERIFIED
```

Later somebody edits `src/auth.rs`.

Your prompt becomes:

```text
~/Projects/Omen  main ! 1 dirty
[shared] >
```

Omen does **not** silently rerun the tests.

Ask:

```text
:why @fact.test
```

and it can answer:

```text
auth test result is DIRTY

Last verified:
  PASS · 13 tests
  1.12s

Changed witness:
  src/auth.rs

Recorded generation: 184
Current generation: 185

Recheck:
  :test auth
```

### What Omen adds

This is one of the biggest differences.

A normal terminal can tell you:

> Tests passed.

Omen can tell you:

> Tests passed, and that result was valid for the previous source state but no longer proves the current source state.

That is considerably more useful.

---

## 5. Deal with a failure

Suppose:

```text
cargo test auth
```

fails.

You see the useful failure, then return to:

```text
~/Projects/Omen  main ✕
[shared] >
```

Instead of scrolling backwards through terminal archaeology:

```text
:show @failed
```

Or:

```text
:open @errors
```

Or:

```text
:why @last
```

`@last` belongs to **your current interactive session**.

A background coding agent cannot suddenly steal it by doing something half a second later.

### What Omen adds

Ordinary shell history remembers strings.

Omen history can remember:

```text
what you ran
when
where
exit status
duration
artifacts
resources
session
effects
```

And gives that object a stable reference.

Instead of:

> “that command I ran about six commands ago”

you have:

```text
@failed
```

---

## 6. Open or edit a file

You can still do:

```text
nvim src/auth.rs
```

or:

```text
code src/auth.rs
```

or whatever editor you prefer.

For interactive terminal applications, Omen hands terminal ownership over properly:

```text
nvim src/auth.rs
```

Neovim behaves like Neovim.

When you exit, Omen takes the terminal back.

Nothing needs to pretend Neovim is JSON.

Omen can also expose objects directly:

```text
:open @errors
```

or:

```text
:open @last.artifact
```

### What Omen adds

There is a difference between:

> Open this filename I have manually copied.

and:

> Open the artifact associated with the failure I am currently inspecting.

Typed references remove little clerical steps.

Those little steps are exactly where humans and AI waste astonishing amounts of time.

---

## 7. Start something that stays running

Traditional shell:

```text
npm run dev &
```

Then later:

```text
jobs
```

And six hours later:

> Which one was `%2` again?

Omen prefers named services:

```text
:services
```

You might see:

```text
SERVICE       STATE       PID
dev           RUNNING     18420
```

Then:

```text
:logs @service.dev
```

```text
:restart @service.dev
```

```text
:stop @service.dev
```

Behind the friendly name is a process resource:

```text
proc://...
```

### What Omen adds

A long-running process stops being:

> whatever child process terminal number three happened to launch.

It becomes a machine object with identity.

This becomes particularly useful once another human shell or an agent needs to inspect the same development server.

---

## 8. Run an interactive program

You type:

```text
python
```

Omen recognises that this invocation needs a terminal.

You get Python normally:

```text
Python 3.x
>>> 
```

Exit:

```text
>>> exit()
```

and you return to Omen:

```text
~/Projects/Omen  main ✓
[shared] >
```

But:

```text
python build.py
```

does not need an interactive terminal and can be supervised normally.

Likewise:

```text
git commit
```

may require editor handoff.

While:

```text
git commit -m "fix parser"
```

does not.

### What Omen adds

Omen reasons about the **invocation**, not merely the executable name.

That sounds minor until you have watched an agent allocate a pseudo-terminal to every Python script because someone once wrote:

```text
python = interactive
```

The command is the meaning-bearing unit.

---

## 9. Repeat or inspect something you already did

Normal shell:

```text
↑
↑
↑
Enter
```

That still works.

Omen history suggestions are deliberately Fish-like: fast, quiet and based on what you have actually done.

But you can also say:

```text
:rerun @failed
```

or:

```text
:inspect @last
```

or:

```text
:history
```

Omen might expose:

```text
exec_8f12
cargo test auth
exit 101 · 1.19s

exec_a391
rg "token" src
exit 0 · 84ms
```

### What Omen adds

History is no longer merely:

```text
a list of old strings
```

It is a set of previous operations with evidence.

And importantly:

```text
:rerun @last
```

means:

> reconstruct this operation and submit it under the authority that exists **now**.

It does not mean:

> reuse whatever permission existed yesterday.

Tethers owns that boundary.

---

## 10. Ask for help

This is where Omen looks most different from either a traditional shell or an “AI terminal”.

Suppose something odd has happened.

Start deterministically:

```text
:why @last
```

If Omen already knows the answer, it explains from recorded state.

For example:

```text
Last test result is DIRTY.

Reason:
  src/auth.rs changed after verification.

Last verified:
  13 passed
  generation 184

Current:
  generation 185

Suggested:
  :test auth
```

No AI required.

But perhaps you have three strange failures and genuinely want judgement:

```text
? could these three failures have the same cause?
```

Now you have explicitly entered the AI lane.

The answer may use Omen's structured context, but it remains advice.

It may suggest:

```text
:show @failed
:open @errors
:test auth
```

It does not quietly start hammering the machine.

### What Omen adds

Most “AI shells” begin with:

> Ask the AI what command to type.

Omen begins with:

> Ask the machine what it already knows.

Only use AI for the part that actually requires reasoning.

That difference is almost the entire philosophy of Omen in miniature.

---

# What makes it feel different?

It is not supposed to feel alien.

That would be a failure.

Most days you should still be typing things such as:

```text
cd crates
git status
cargo test
rg "foo" .
nvim src/main.rs
```

The difference lives around those commands.

A normal shell is roughly:

```text
YOU
 │
 ▼
COMMAND
 │
 ▼
PROGRAM
 │
 ▼
TEXT
```

Omen becomes:

```text
                 ┌────────── Facts
                 │
                 ├────────── History
YOU → OPERATION ─┼────────── Resources
                 │
                 ├────────── Artifacts
                 │
                 ├────────── Processes
                 │
                 └────────── Evidence
                       │
                       ▼
                    TERMINAL
```

The terminal is still there.

It just stops carrying the impossible burden of being the computer's only memory of what happened.

---

# A small amount of theatre

Omen should have style.

Not fireworks.

Not an RGB spaceship bridge.

Something quieter.

Commands should form clean semantic blocks where the terminal supports it:

```text
~/Projects/Omen  main ✓
[shared] > cargo test auth

  cargo test auth                         exec_8f12

  13 passed · 0 failed                    1.12s
  fact://test/auth                        VERIFIED

~/Projects/Omen  main ✓
[shared] >
```

A failure might be:

```text
~/Projects/Omen  main ✕
[shared] > cargo test auth

  cargo test auth                         exec_42bd

  12 passed · 1 failed                    1.21s
  tests/auth.rs:88

  @failed
  @errors
  :why @last

~/Projects/Omen  main ✕
[shared] >
```

And state drift:

```text
~/Projects/Omen  main ! 1 dirty
[shared] >

  auth tests no longer current
  src/auth.rs changed

  :why @fact.test
```

No giant red banner.

No:

> **UH OH! SOMETHING WENT WRONG! 🤖**

The machine simply clears its throat.

---

# The advantage in one sentence

A conventional shell is excellent at:

> **running commands.**

Omen is trying to become excellent at:

> **remembering what those commands meant to the machine afterwards.**

That is why the shell itself can stay pleasantly ordinary.

The unusual part is underneath it.
