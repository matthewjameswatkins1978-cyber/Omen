# Omen 0.4 Evidence — Local Peer Admission & Security Proof

**Document**: `docs/evidence/0.4/SECURITY_PROOF.md`  
**Status**: VERIFIED  
**Crates**: `omen-ipc`, `omen-daemon`

---

## 1. Local Peer Admission Doctrine

Omen is a single-user local substrate. Cross-user access or untrusted access is rejected closed at the transport boundary:
1. **Network Denial**:
   - Omen IPC never binds to network interfaces (TCP/UDP, localhost ports).
   - All communication is strictly local via OS-mediated IPC (Unix domain sockets or Windows Named Pipes).
2. **Unix Security (Linux & macOS)**:
   - Socket directory created with mode `0700` (`rwx------`).
   - Server inspects peer credentials using `SO_PEERCRED` / `getpeereid` via `rustix::process`.
   - Incoming connection is rejected immediately if `peer_uid != current_uid`.
3. **Windows Security**:
   - Named pipe created with security attributes accessible only to the user's security context.
   - Upon connection, server obtains client process ID via `GetNamedPipeClientProcessId`.
   - Server opens client token with `OpenProcessToken` and queries `TokenUser`.
   - Server compares client `TokenUser` SID against server process `TokenUser` SID via `EqualSid`.
   - Non-matching user accounts are rejected with `PermissionDenied`.

### Automated Test Evidence
- `crates/omen-daemon/tests/peer_admission_tests.rs`:
  - `test_peer_admission_same_user_allowed`: Verifies that same-user connections are admitted, handshaked, and can exchange typed messages.

---

## 2. Multi-Workspace Isolation

Omen daemon serves multiple workspaces concurrently, but boundaries between them are absolute:
- Workspace IDs are deterministic SHA-256 hashes of canonical workspace root paths (`ws-<sha256>`).
- Database connections, in-memory hot semantic indices, file watchers, managed service registries, and event broadcast channels are strictly partitioned by `WorkspaceId`.
- Facts published in Workspace A are never visible in Workspace B.
- Events broadcast in Workspace A are never received by clients attached solely to Workspace B.

### Automated Test Evidence
- `crates/omen-daemon/tests/workspace_isolation_tests.rs`:
  - `test_multi_workspace_isolation`: Attaches two distinct workspaces; verifies fact registries and snapshots do not leak across boundaries.
- `crates/omen-daemon/tests/e2e_shared_runtime_proofs.rs`:
  - `proof_j_strict_workspace_isolation`: Complete isolation proof across two workspaces with concurrent mutations.
