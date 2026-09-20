# Omen 0.7 Semantic Environment — Platform Matrix & Assurance

## 1. Truthful Assurance Doctrine

Omen reports capabilities truthfully:
- If a semantic provider binary (`ast-grep`, `rust-analyzer`, `cargo`, `npm`, `uv`, `go`) is missing or unsupported on a given operating system, Omen reports `Unsupported` or falls back to available substrates.
- It **never** fakes `Enforced` or pretends language intelligence is present when it cannot be executed.

---

## 2. Platform Capability Matrix

| Semantic Capability | Windows Native (x86_64) | Linux (x86_64 / ARM64) | macOS (ARM64 / x86_64) |
|---|---|---|---|
| **Hot Keystroke Completion Cache** | `ENFORCED` (< 1ms) | `ENFORCED` (< 1ms) | `ENFORCED` (< 1ms) |
| **Generational Witness Invalidation** | `ENFORCED` (SHA-256) | `ENFORCED` (SHA-256) | `ENFORCED` (SHA-256) |
| **Structural Pattern Search (`ast-grep`)** | `ENFORCED` (when installed) | `ENFORCED` (when installed) | `ENFORCED` (when installed) |
| **ThreadMoth Mutation Bridge** | `ENFORCED` (cryptographic certs) | `ENFORCED` (cryptographic certs) | `ENFORCED` (cryptographic certs) |
| **LSP Client (`rust-analyzer`)** | `ENFORCED` (async JSON-RPC) | `ENFORCED` (async JSON-RPC) | `ENFORCED` (async JSON-RPC) |
| **Hostile LSP Bounding & Discard** | `ENFORCED` (stdio cancel) | `ENFORCED` (stdio cancel) | `ENFORCED` (stdio cancel) |
| **SCIP Index Parsing & Staleness** | `ENFORCED` (in-memory parse) | `ENFORCED` (in-memory parse) | `ENFORCED` (in-memory parse) |
| **Cargo Workspace Metadata** | `ENFORCED` (`cargo metadata`) | `ENFORCED` (`cargo metadata`) | `ENFORCED` (`cargo metadata`) |
| **NPM / Python / Go Manifests** | `ENFORCED` (pure Rust parsers) | `ENFORCED` (pure Rust parsers) | `ENFORCED` (pure Rust parsers) |
| **Zero-Model AI Query Classification** | `ENFORCED` (`provider_calls = 0`) | `ENFORCED` (`provider_calls = 0`) | `ENFORCED` (`provider_calls = 0`) |
| **Model Context Protocol Semantic Tools** | `ENFORCED` (5 typed tools) | `ENFORCED` (5 typed tools) | `ENFORCED` (5 typed tools) |
