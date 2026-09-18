# Omen Roadmap

## Version 0.2: Runtime Proof (Complete)
- [x] M0: Workspace bootstrap, toolchain pinning, and CI scaffolding.
- [x] M1: Core domain types, typed IDs, URI grammar, strict wire schemas.
- [x] M2: Knowledge engine, SQLite WAL persistence, Fact Registry, lazy pessimism, CAS artifact store.
- [x] M3: Execution engine, argv-only spawning, closed stdin, bounded context, Job Objects/containment backends, `omen-gremlin`.
- [x] M4: Tool Atlas, TOML runtime profiles, controlled probing validator.
- [x] M5: Machine-readable Git and ripgrep adapters.
- [x] M6: ThreadMoth adapter with pre/post-image hashing and structural change detection.
- [x] M7: Cargo adapter with JSON diagnostic streaming.
- [x] M8: Full End-to-End proofs:
  - Auth test fixture: Cargo passing -> ThreadMoth mutate -> DIRTY fact refusal -> explicit Cargo failure -> why provenance -> ThreadMoth fix.
  - Gremlin torture: closed stdin, timeouts, process tree cleanup, CAS spooling.
- [x] M9: Tethers execution contract fixture.
- [x] M10: CLI commands and 0.2 Implementation Report.

## Version 0.3: Human Interface (Current)
- [ ] H0: Architecture & dependency freeze (Reedline, Crossterm, nu-ansi-term, fuzzy-matcher).
- [ ] H1: Interactive shell core & terminal lifecycle (raw mode, child process handoff, dumb degradation).
- [ ] H2: Semantic grammar (`:action`, `@reference`, `? query`).
- [ ] H3: Fact-aware completion & ghost suggestions.
- [ ] H4: Execution history, session identity (`InteractiveSessionId`), and typed references (`@last`, `@failed`).
- [ ] H5: Human diagnostics & explanations (Levels 0-3 progressive disclosure, `:why`).
- [ ] H6: Semantic blocks & terminal integration (OSC 7, OSC 8, OSC 133).
- [ ] H7: Blast radius preflight & Paste Guard.
- [ ] H8: Services & process UX.
- [ ] H9: Optional AI reasoning lane boundary.
- [ ] H10: Human/Agent shared-reality proof.
- [ ] H11: Performance benchmarks, cross-platform verification, docs & closeout.

## Future Versions (Post-0.3)
- **Omen 0.4**: Daemon architecture (`omend`) with local IPC/named pipes and cross-process coordination.
- **Omen 0.5**: Native Model Context Protocol (MCP) server projecting Omen tools and facts to agents.
- **Omen 0.6**: Richer sandbox containment (Linux Landlock fine-tuning, Windows driver/filter integrations, macOS Endpoint Security).
- **Omen 0.7**: Ast-grep adapter and Carapace grammar ingestion engine.
