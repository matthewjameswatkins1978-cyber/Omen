# Omen verification latency evidence

This change preserves the complete verification gate and changes only its orchestration, CI scheduling, and a redundant nested Cargo test fixture.

## Before

- Canonical base: `9561fce4b5b4912729a836d50507188168f99ee6` (`origin/main`, 2026-09-24).
- Toolchain: Cargo 1.98.1; rustc 1.98.1.
- Cargo metadata target directory: `D:\Omen Shell\target` (the baseline command's captured `cargo metadata` result; the study worktree itself was `D:\Omen verification latency`).
- `CARGO_TARGET_DIR`: unset. `RUSTC_WRAPPER`: unset.
- Untouched `cargo run -p xtask -- verify`: 451.526 seconds, failed in schema stage (exit 101). On Windows, xtask tried to launch Cargo to rebuild the currently running `xtask.exe`; replacing that locked executable failed with access denied. Formatting, Clippy, and all workspace tests had passed before this failure. Benchmarks and cargo-deny were not reached.
- Local Windows target footprint at baseline: 14,999,667,395 bytes total; `target/debug/incremental`: 5,888,764,290 bytes. These are the local clean-worktree build results, not hosted-runner measurements.
- Representative hosted run: [CI run 36067882902](https://github.com/matthewjameswatkins1978-cyber/Omen/actions/runs/36067882902), exact product SHA `fa08afcc139241b23a0d29db90ded0e95c2f51d5`, all jobs passed. `check-and-lint` took 6m29s and redundantly ran formatting, Clippy, and schema verification before full xtask verification. It spent 1m30s compiling pinned cargo-deny on a miss.
- In that same run, Windows matrix took 14m31s, including 7m44s for workspace tests, 1m31s for the Windows rust-analyzer acceptance proof, and 3m42s in post-job cache save. The Rust cache was a miss and the uploaded cache payload measured 2,826,856,605 bytes. GitHub's logs did not expose the restored-cache size or separately measured hosted `target` and incremental directory sizes.
- Hosted jobs were serialized behind `check-and-lint`: rust-analyzer acceptance, all three matrix tests, and Windows preview candidate each declared `needs: check-and-lint` despite checking out and proving the exact product SHA independently.
- Feature branches also triggered one push workflow and another PR workflow for the same commit.

## After

- Final local canonical verification: PASS. Stage summary: fmt 1.1s, Clippy 10.8s, workspace tests 182.8s, schemas 0.0s, benchmarks 34.2s, cargo-deny 10.8s; 239.8s total within xtask. Full command wall clock including the initial xtask build was 251.281s.
- This warm local final run is not a directly comparable speedup against the cold 451.526s baseline: the baseline failed before benchmarks and cargo-deny, while the final run reused the target directory built by the baseline attempt. Hosted before/after CI timings are the comparable scheduling and duplicate-work evidence.
- Focused `test_cargo_check_and_test`: PASS in 0.98s (the old `cargo_tests` test binary took 30.94s locally, including both tests). Both paths call the real production adapter and real Cargo; the replacement changes only the crate fixture.
- Hosted target-cache instrumentation records cache hit/miss and resulting `target` and incremental byte sizes without changing the cache key, paths, or save/restore policy. Final hosted job measurements are reported from the dispatched exact-SHA run.
