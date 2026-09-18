# Omen Roadmap

## Version 0.2: Runtime Proof (Current)
- [x] M0: Workspace bootstrap, toolchain pinning, and CI scaffolding.
- [ ] M1: Core domain types, typed IDs, URI grammar, strict wire schemas.
- [ ] M2: Knowledge engine, SQLite WAL persistence, Fact Registry, lazy pessimism, CAS artifact store.
- [ ] M3: Execution engine, argv-only spawning, closed stdin, bounded context, Job Objects/containment backends, `omen-gremlin`.
- [ ] M4: Tool Atlas, TOML runtime profiles, controlled probing validator.
- [ ] M5: Machine-readable Git and ripgrep adapters.
- [ ] M6: ThreadMoth adapter with pre/post-image hashing and structural change detection.
- [ ] M7: Cargo adapter with JSON diagnostic streaming.
- [ ] M8: Full End-to-End proofs:
  - Auth test fixture: Cargo passing -> ThreadMoth mutate -> DIRTY fact refusal -> explicit Cargo failure -> why provenance -> ThreadMoth fix.
  - Gremlin torture: closed stdin, timeouts, process tree cleanup, CAS spooling.
- [ ] M9: Tethers execution contract fixture.
- [ ] M10: CLI commands and 0.2 Implementation Report.

## Future Versions (Post-0.2)
- **Omen 0.3**: Daemon architecture (`omend`) with local IPC/named pipes and cross-process coordination.
- **Omen 0.4**: Native Model Context Protocol (MCP) server projecting Omen tools and facts to agents.
- **Omen 0.5**: Richer sandbox containment (Linux Landlock fine-tuning, Windows driver/filter integrations, macOS Endpoint Security).
- **Omen 0.6**: Ast-grep adapter and Carapace grammar ingestion engine.
