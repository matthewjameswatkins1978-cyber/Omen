# Omen Agent Adapter Kit

**Adapter protocol:** `omen.agent-adapter/0.1` (separate axis from Machine Contract `0.8`).

## Idea

Omen is agent-native, but no provider or harness gets to become part of
Omen's ontology. An adapter **translates** between an external agent /
harness and the Omen machine. It does not decide truth, grant itself
authority, redefine Omen errors, or invent machine state.

```
external agent / harness
        |
        v
Agent Adapter            <- translates only
        |
        v
versioned adapter boundary (this document)
        |
        v
Omen Machine Contract / Agent contract
        |
        v
Omen truth + authority boundary
        |
        v
execution / evidence / results
```

Extensions add capabilities. They do not enter the constitution. A package
may declare what it **needs**. It may never declare what it is **allowed**.

Related: `docs/AGENT_PROVIDER_CONTRACT.md` (the in-process provider
semantics adapters must preserve).

## Package, installed adapter, binding

These are distinct entities; do not collapse them:

- **Adapter package**: portable code/data/manifest.
- **Installed adapter**: exact local bytes (executable digest, manifest
  digest, version).
- **Binding**: the exact live connection — adapter, adapter version,
  adapter digest, protocol version, transport, configuration fingerprint,
  provider/harness identity.
- **Active provider**: the currently selected reasoning route.
- **Conformance result**: evidence that translation satisfies a contract.

Conformance does not install. Installation does not activate. Activation
does not create authority.

## Transport decision (why a new protocol)

Omen already has machine surfaces, and each was inspected:

- **MCP stdio JSON-RPC** (`omen-mcp`, `sdk/.../transport.py`): a tool-host
  surface — Omen *serves* tools to models. An adapter needs the opposite
  direction plus version negotiation, tool-request round trips, and
  cancellation. Reused as a *pattern* (argv spawn, id-keyed pending map,
  bounded shutdown), not as an endpoint.
- **Daemon IPC** (`omen-ipc`): a trusted local bus with workspace-attach
  state and daemon lifecycle coupling. Wrong trust shape for third-party
  code. Borrowed the `ClientHello/DaemonHello` negotiation idiom only.
- **LSP framing**: precedent for `Content-Length` + cancel semantics.
- **Process supervision** (`omen-engine` supervisor, Python transport):
  copied verbatim (argv-only, closed stdin where applicable, dual
  timeouts, cancel flag, kill-tree, bounded preview, redaction).

So the kit defines one tiny new wire protocol, `omen.agent-adapter/0.1`:
newline-delimited JSON frames over child stdio, transport-neutral (any
byte stream carrying lines works). Human logs go to stderr only and are
never parsed. Every frame carries a `protocol` tag; wrong tags refuse.
Frames are size-bounded (1 MiB frame, 256 KiB text field, 32 actions,
32 unknown fields). Unknown fields are preserved but semantically inert.

## Handshake and version negotiation

1. Omen spawns the adapter (argv only, explicit environment) and sends
   `omen.hello` with: Omen contract version, supported adapter protocol
   versions (most preferred first), required features, optional features.
2. The adapter answers `adapter.hello` with: adapter id/version,
   selected protocol version, capabilities, features, supported Omen
   contracts, credential *labels*.
3. Omen runs pure negotiation:
   - highest-preference common protocol wins, else explicit
     `incompatible_protocol` refusal;
   - adapter must list Omen's contract, else `incompatible_contract`;
   - every required feature must be present, else
     `missing_required_feature`;
   - missing optional features degrade truthfully (recorded in the
     negotiated set, never hidden).

Incompatible semantics are never silently reinterpreted.

## Request/response semantics

- `omen.reason { request_id, prompt, orientation, tool_allowlist }` —
  bounded prompt plus a *small* orientation (contract version, protocol,
  capabilities, roots). Never the whole contract, schemas, or history.
- `adapter.tool_request { request_id, call_id, tool, input }` — the tool
  name must be on Omen's allowlist or Omen answers
  `tool_result { ok: false, error: unsupported_capability }`. The adapter
  must still produce a final response afterwards.
- `omen.tool_result { request_id, call_id, ok, result }`.
- `adapter.response { request_id, kind, message, proposed_actions,
  references, uncertainty }` — kinds mirror `AgentResponse`; every
  proposed action is validated with the G1 gate before admission.
- `adapter.error { request_id?, code, message, detail? }` — closed code
  vocabulary (`auth_required`, `unavailable`, `rate_limited`, `timeout`,
  `unsupported_capability`, `malformed`, `incompatible`,
  `transport_failure`, `internal`). Unknown codes collapse to `internal`
  with bounded detail.
- `omen.cancel { request_id }` then bounded grace, then kill.
- `omen.shutdown {}` then bounded grace, then kill-tree.

One launch per request. No retries. No fallback substitution. A second
tool ask after a completed round trip is refused, never looped.

## Lifecycle

`inspect -> validate -> conform -> install -> bind -> active`, with side
states `stale`, `incompatible`, `degraded`, `unavailable`, `disabled`,
`quarantined`. Transitions are explicit (`AdapterLifecycle::advance`);
illegal transitions fail visibly. See `crates/omen-agent-adapter/src/lifecycle.rs`.

## Manifest

Rigid `AdapterManifest` (schema version 1): id, version, argv
entrypoint, supported protocols, required contracts, capabilities,
required programs/config labels, credential labels, transports,
streaming flag, platforms, optional/required features. Forbidden:
shell syntax in argv, permission/authority-shaped keys, secret-shaped
keys, secret values as labels. Unknown keys are bounded and inert.

## Auth hooks and environment policy

Credential *labels* document which external authenticator the adapter
consults itself (e.g. `codex-auth`). Credential *values* never cross
into manifests, frames, or environments. `EnvPolicy` is an explicit
allowlist plus non-secret literals; the parent environment is otherwise
dropped entirely. The Codex policy passes only loader/scratch/identity
variables (`SystemRoot`, `SystemDrive`, `USERPROFILE`, `CODEX_HOME`,
`TEMP`, `TMP`, `PATH`, `NO_COLOR`) — conspicuously not `OPENAI_API_KEY`
or any other credential. Tests prove isolation with synthetic secrets.

## Errors, cancellation, streaming, disconnect

- Failures map into stable `AgentError`; vendor strings stay bounded
  underneath and never widen semantics.
- User cancel: `omen.cancel`, bounded grace, kill-tree, truthful
  `Timeout`/transport error. No phantom completion, no hidden replay.
- Crash/disconnect: explicit `Provider` error, no synthetic response, no
  silent substitution, no auto-retry; the binding goes `unavailable`;
  rebind recovers with machine truth intact.
- Streaming is declared truthfully (`streaming: bool`); absence is not a
  product failure. Final meaning always resolves to canonical
  `AgentResponse` semantics.

## Bounds and backpressure

Oversized frames/messages, diagnostic floods, partial stalls, invalid
UTF-8, duplicate/unknown ids, and unread-output exits all resolve to
bounded waits and truthful failures (hostile suite in
`tests/adapter_roundtrip_tests.rs`). No shell hang, no unbounded memory.

## Conformance

Reuse G1: `check_scenario` runs unchanged against any `AgentProvider`,
including external routes. The fixture route passes the applicable
battery (explanation, proposal, malformed, unknown-kind, auth). The
deterministic canary remains the independent witness. Do not invent a
G2 conformance framework.

## Binding identity

Every live binding records adapter id/version/digest, manifest digest,
protocol version, provider id, model (or UNKNOWN — never invented),
transport, Omen contract, and a sha256 configuration fingerprint over
non-secret shape only. `BindingIdentity::model_or_unknown()` renders the
honest default.

## Startup performance

Registry construction resolves adapter binaries via PATH only —
filesystem scan, no spawn, no network, no credits. Versions, digests,
and account validity resolve at first use (cached) or explicit probe.
Cold startup inspects bounded local metadata only.

## Human and machine surfaces

Humans keep `:agent providers`, `:agent use <id>`, `:agent status`,
`? <question>`. No provider-specific commands, no second grammar.
Machines get structural descriptors and binding records — never secret
values. No `omen adapter ...` CLI exists yet; add the minimum only when
a genuine need appears (none did for G2).

## Reference implementations

- `omen-fixture-adapter` (in-crate bin): hello, capabilities, reasoning,
  typed response, tool proposal, error, prompt shutdown. Behavior varies
  by `OMB_FIXTURE_MODE`. Copy this, not the Codex driver.
- `omen-mock-codex` (in-crate bin, CI only): canned Codex JSONL
  transcripts for translation tests without credentials.
- The real Codex route (`crates/omen-agent/src/codex.rs`) drives the
  official `codex exec --json` machine surface with pinned argv,
  `--ignore-user-config` (personal config cannot change Omen semantics),
  sandbox `read-only`, and a JSON-Schema-constrained final message that
  Omen validates itself (instruction, not trust).

## Codex reference notes

- Codex authentication stays in Codex's own store; Omen discovers
  configured-enough-to-attempt at use time.
- Server-assigned model (Omen passes no `-m`): recorded as UNKNOWN.
- Sandbox `read-only` plus ungranted proposals prove Codex cannot bypass
  Omen: consequential actions emerge as typed proposals or not at all.
- Context economy: bootstrap prompt is budget-capped (4 KiB, tested);
  detail arrives via `omen.describe_capability` inside the tool round trip.

## Package G closeout

G2 completes Package G when: provider-neutral conformance (G1) +
adapter kit + Codex reference route with a real structured tool/action
round trip + disconnect/recovery + credential isolation + no ontology
leakage — all with evidence. Text compatibility never certifies a route.
