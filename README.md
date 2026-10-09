# Omen

**A shell for humans. A machine-readable runtime for AI agents.**

*Keep the commands you know. Give the computer a better memory of what happened.*

Omen is an **agent-native developer runtime and human shell**, built in Rust. It runs familiar development tools, understands your workspace, preserves useful execution evidence, and exposes that same machine reality through both a quiet terminal and typed interfaces for agents.

A conventional shell is excellent at connecting programs. Omen keeps that strength and adds the missing context: **what ran, what it touched, what is still true, what failed, and what an agent can safely discover next**.

> **Omen is substrate, not sovereign.** It makes the machine legible and executable. It does not become your AI agent, your policy engine, or your programming language.

**Current stage:** `0.9.0-preview.25`, moving toward a Windows/Linux `1.0.0-rc.1`. Omen is under active development, not a finished 1.0 release. [What works today](#what-works-today-and-whats-next) · [Try Omen](#try-omen) · [Architecture](#one-machine-reality-five-clear-responsibilities)

---

## The terminal, but with a better idea of what is going on

Most of your day can still look like this:

```text
cd crates
ls -l
git status --short
rg "TODO" . | head -n 10
cargo test && echo "tests passed"
```

The commands are familiar. The surrounding environment is different.

- **A real shell for real tools.** Use Cargo, Git, ripgrep, Python, Node, editors and other installed programs. No bespoke command language is required for ordinary work.
- **25 native read-only commands.** `ls`, `cat`, `grep`, `sort`, `wc`, `find`, `tree` and friends run as Omen builtins with structured result truth. All-builtin pipelines can run without spawning external processes.
- **Pipelines without text corruption.** Omen's execution path respects raw stdout/stdin bytes. A byte stream is not silently turned into a pretty string.
- **A shell that can explain itself.** Typed references, Facts, execution history and content-addressed artifacts make previous work inspectable instead of leaving everything buried in scrollback.
- **Completion that knows the workspace.** Lens combines command names, options, files, aliases, history and semantic discovery, rather than relying only on PATH.
- **A machine interface, not terminal scraping.** CLI machine mode, MCP and a Python SDK let agents consume bounded structured information.
- **Honest execution boundaries.** Consequential operations require real authority. If permission or enforceability is missing, Omen reports that fact instead of inventing success.

Omen is designed to be **quiet for humans and explicit for machines**.

## See it in use

### 1. Work like you already do

```text
cd src
ls
git status
cargo test
```

Omen's portable shell also understands bounded composition:

```text
ls | grep rs | wc -l
sort < names.txt | uniq -c
cargo check && cargo test
```

And it supports standalone background jobs:

```text
npm run dev &
jobs
```

No new scripting language. No need to rewrite existing developer workflows.

**The important distinction:** file-reading and command composition are available in the current preview; ordinary file mutations and output-file writes are deliberately gated while their Tethers filesystem authority contract is completed. For example, `> output.txt` is currently validated but refused rather than being allowed without the required authority.

### 2. Ask the machine what happened

Omen has more than one way to interact. Ordinary commands run programs; the `:` lane asks Omen for semantic information; `@` points to recorded resources; `?` explicitly requests optional AI reasoning.

```text
:status
:doctor
:show @failed
:why @last
```

Instead of hunting through an old transcript, inspect the execution or the recorded evidence associated with it.

```text
:symbol SessionToken
:def SessionToken
:refs SessionToken
```

Semantic discovery uses tools such as AST analysis, language servers and indexes where available. Omen reports missing or partial coverage rather than hallucinating a symbol.

And when deterministic evidence is not enough:

```text
? why might these test failures be related?
```

The AI lane is **opt-in and advisory**. It does not silently execute suggestions or mint its own permission.

### 3. Keep evidence without flooding the terminal

Omen distinguishes a short, useful result from its full supporting evidence.

```text
command result
  ├── execution identity
  ├── exit status / timing
  ├── bounded stdout and stderr preview
  ├── artifact://...       full retained output when available
  └── Facts / provenance  what the result established
```

Its Fact Registry tracks semantic machine state with freshness and invalidation. A test result can be **CURRENT** for the source state it verified, then **DIRTY** after relevant files change. An old green result is not automatically proof about new code.

That means a human or agent can ask *what do we know now?*, not just *what did this command print?*

### 4. Let an unfamiliar agent discover Omen itself

An agent does not need a novel-length prompt to start. Omen exposes a bounded, versioned machine contract:

```sh
omen --machine orient
omen --machine capabilities
omen --machine describe semantic.definition
omen --machine context
```

The agent can discover available operations and then inspect a specific capability rather than downloading everything into its context window.

There is also an MCP interface:

```sh
omen mcp --workspace .
```

And a Python SDK for programs that prefer a typed client:

```python
from omen_shell import Omen

with Omen(workspace=".") as omen:
    info = omen.orient()
    result = omen.execute(["git", "status", "--short"])
    print(result.execution_id, result.exit_code)
```

See the [machine contract](docs/reference/machine-contract.md), [MCP and human walkthrough](docs/USING_OMEN.md), and [Python SDK](sdk/python/omen-shell/README.md).

## What makes Omen different?

We like other shells. Bash and zsh are excellent at scripting, Fish makes interactive command lines approachable, and Nushell demonstrates the value of structured data. Omen borrows good ideas, but aims at a different missing layer: **a shared, verifiable understanding of the machine for both people and agents**.

| Question | Typical command-shell approach | Omen's approach |
| --- | --- | --- |
| What happened? | Exit code, text output, shell history | Execution identities, typed results, history, Facts and evidence references |
| Is that old result still relevant? | You infer it or run the command again | Facts carry freshness/generation information and can become dirty |
| How does an agent learn its tools? | Documentation, prompts and terminal probes | Versioned discovery, structured machine output and MCP |
| What about giant output? | Scrollback, redirects and ad-hoc logs | Bounded previews with content-addressed artifacts |
| How are commands combined? | A shell language and OS pipes | Familiar limited shell composition with byte-preserving process semantics |
| Who decides whether effects are allowed? | Shell/OS privileges or external wrappers | An explicit Tethers authority boundary for consequential Omen operations |
| Do I have to surrender the terminal to an AI? | Depends on the product | No. AI is an optional, explicit `?` lane |

**Omen is not trying to beat Bash at Bash scripting, replace Nushell's entire data language, or make AI write more shell commands.** It is trying to make the result of using developer tools more trustworthy, discoverable and reusable.

There are no universal speed or token-savings claims here: those need workload-by-workload measurements. Omen's practical advantage is its architecture and its growing tested implementation, not a made-up benchmark.

## One machine reality, five clear responsibilities

Omen belongs to a family of complementary tools. They are **separate components with distinct jobs**, not five names for one giant daemon, nor all mandatory dependencies for a basic Omen session.

```text
                   HUMAN  /  CODING AGENT
                             │
                   ordinary shell / CLI / MCP
                             │
                      ┌────────────┐
                      │    OMEN    │
                      │   executes │
                      │  & explains│
                      └──────┬─────┘
                             │  consequential work
                    ┌────────▼───────┐
                    │    TETHERS     │
                    │  authorises    │
                    └────────────────┘

  LANTERN       knowledge, memory, provenance
  RESOLVE       coordination, guards, scope/fencing
  THREADMOTH    bounded, verified source mutations
```

| Component | Its job | Why it matters |
| --- | --- | --- |
| **Omen** | Physical execution, process/PTY supervision, typed resources, Facts, artifacts and machine-facing interfaces | A useful shared view of what the computer actually did |
| **Tethers** | Capability identity, policy, approval, durable intent, replay and outcome authority | Permission is explicit and auditable, not inferred from a suggested command |
| **Resolve** | Live coordination, scope locks, fencing and conflict admission | Multiple actors can coordinate without turning Omen into a competing policy server |
| **Lantern** | Durable memory, contextual knowledge and provenance | Long-lived project understanding belongs in a knowledge system, not shell scrollback |
| **ThreadMoth** | Deterministic, bounded structural code edits with pre/post checks | Source mutation can be precise and inspectable instead of blind search-and-replace |

**Lantern knows. Resolve coordinates. Tethers controls. Omen makes the machine legible and enforceable. ThreadMoth mutates deterministically.**

These boundaries are deliberate:

- **Tethers approval does not mean Omen can roll back every physical side effect.** Omen must report partial or uncertain outcomes truthfully.
- **Resolve coordination never grants Tethers permission.**
- **Lantern memory is not an execution ledger or permission record.**
- **ThreadMoth does not bypass authority because it knows how to edit a file.**

Under the hood, Omen's own modules add the practical machinery: **Lens** for semantic discovery/completion, **Atlas** for tool knowledge and profiles, **Facts + CAS** for state and artifacts, **Compat** for differential testing, and its shared runtime for multi-client machine state.

See [the architectural charter](docs/ROAD_TO_1_0.md) and [the Tethers boundary](docs/evidence/0.8/TETHERS_BOUNDARY.md).

## What works today, and what's next?

| Area | Current preview status |
| --- | --- |
| Ordinary external commands, interactive shell and native Windows/Linux execution | Implemented; undergoing RC hardening and compatibility testing |
| 25 read-only builtins, in-process dispatch, help and option discovery | Merged |
| Pipes, command sequencing, `&&` / `\|\|`, input redirects and failure semantics | Merged; differential coverage continues |
| Standalone background jobs and Lens completion | Merged; lifecycle and dogfood qualification continue |
| Typed Facts, CAS artifacts, semantic tooling, MCP and Python interfaces | Implemented; discoverability and product assurance continue |
| Host-executed process composition with Tethers bundle admission | Integrated and tested on its pinned authority seam |
| `cp`, `mv`, `rm` and other ordinary filesystem mutations | **Not yet enabled**: fail-closed pending a reviewed Tethers Host-filesystem authority contract |
| Output-file redirects | **Currently refused closed** pending that same authority work |
| Windows/Linux `1.0.0-rc.1` | **In progress**: Compat, security, packaged lifecycle, documentation and release qualification |
| macOS RC support | **Deferred** until after the Windows/Linux RC campaign |

No status in this table is a promise that a specific third-party tool, provider or backend is installed on your machine. Omen reports actual availability and supported assurance.

The distinction matters: a preview can have a tested subsystem without yet being a fully qualified release.

## Try Omen

Omen is currently best approached as a **developer preview**. The first RC is not yet a stable download promise.

### Build from source

Prerequisites: Git and a Rust toolchain meeting the workspace's declared MSRV (currently **Rust 1.98.1**). Platform-specific components may need their documented tools.

```sh
git clone https://github.com/matthewjameswatkins1978-cyber/Omen.git
cd Omen
cargo build --locked --release -p omen-cli
```

Run `target/release/omen` (or `target\release\omen.exe` on Windows) to open the interactive shell, then try:

```text
:status
:doctor
help
ls
git status
```

To inspect Omen outside the interactive shell:

```sh
omen doctor
omen --machine orient
omen --machine capabilities
```

To run the repository's verification workflow:

```sh
cargo run -p xtask -- verify
```

For real installation, update, doctor/repair and rollback semantics, see [Product Lifecycle](docs/PRODUCT-LIFECYCLE.md). For a guided tour of the shell, see [Using Omen](docs/USING_OMEN.md).

## Where we're heading

**RC1 is about making the existing ideas dependable, not bolting on another language.**

The immediate work is finishing safe filesystem authority, growing the Windows/Linux compatibility corpus, strengthening crash/recovery and security proofs, qualifying real installable packages, and making the everyday shell genuinely pleasant to use.

Longer term, the aim is simple: **one machine environment where humans can work naturally, agents can understand precisely, and neither has to guess what the computer did.**

Built with a reuse-first philosophy: borrow proven implementation, credit it properly, keep the semantics honest, and teach agents through discoverable interfaces rather than enormous instruction files.

See the [Roadmap](docs/ROADMAP.md), [Road to 1.0](docs/ROAD_TO_1_0.md), and [licensing/reuse policy](docs/LICENSING_AND_REUSE.md).

**Licensing note:** Omen's final project licence has not yet been selected. MPL-2.0 is the current leading candidate. See the policy before assuming particular distribution rights.

---

<sub>Omen · Agent-native developer runtime & human shell · \O/</sub>
