# Omen 0.7 Semantic Environment — Multi-Ecosystem Package Semantics Proof

## 1. Objective

Prove that:
1. Omen extracts canonical package records, dependency listings, build targets, and runnable tasks across multiple ecosystems without executing arbitrary shell scripts.
2. The URI scheme `@package://<ecosystem>/<name>` unambiguously represents workspace packages.

---

## 2. Cargo Workspace Semantics (Proof G)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_g_cargo_workspace_metadata_is_canonical`):

- `CargoSemanticProvider` executes `cargo metadata --format-version 1 --no-deps`.
- Discovers all workspace members:
  - Emits `package://cargo/omen-core`, `package://cargo/omen-engine`, etc.
  - Correctly associates build targets (`omen-core` lib, `omen-gremlin` bin).
  - Normalizes manifest paths and version strings.

---

## 3. Non-Rust Ecosystems: NPM & Python UV (Proof H)

In `crates/omen-cli/tests/semantic_environment_proofs.rs` (`test_proof_h_two_non_rust_ecosystems_produce_semantic_metadata`):

### 1. NPM (`package.json`)
- Parsed by `NpmSemanticProvider`:
  - Package: `omen-web-dashboard` (v1.4.0)
  - URI: `package://npm/omen-web-dashboard`
  - Dependencies: `react` (`^18.2.0`)
  - Tasks: `build` (`vite build`), `test` (`vitest`)

### 2. Python / UV (`pyproject.toml`)
- Parsed by `PythonUvSemanticProvider`:
  - Package: `omen-ml-pipeline` (v0.8.2)
  - URI: `package://python/omen-ml-pipeline`
  - Dependencies: `scikit-learn>=1.2.0`
  - Tasks: `train` (`pipeline.train:run`)

### 3. Additional Supported Ecosystems
- **Go (`go.mod`)**: Parsed by `GoSemanticProvider` for module identity and dependencies.
- **Docker (`Dockerfile`, `compose.yaml`)**: Parsed by `DockerSemanticProvider` for container services.
- **GitHub CLI (`.github/workflows/*.yml`)**: Parsed by `GitHubCliSemanticProvider` for CI tasks.
