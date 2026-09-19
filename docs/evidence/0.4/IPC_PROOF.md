# Omen 0.4 Evidence — IPC Protocol Proof

**Document**: `docs/evidence/0.4/IPC_PROOF.md`  
**Status**: VERIFIED  
**Crates**: `omen-ipc`, `omen-daemon`, `omen-client`

---

## 1. Framing & Boundary Invariants

Omen Local IPC uses a deterministic length-prefixed JSON wire format:
- **Header**: 4-byte big-endian unsigned integer (`u32`) indicating payload byte length.
- **Payload**: Canonical UTF-8 JSON wire frame.
- **Frame Limit Ceiling**: Strictly enforced at `1 MiB` (`MAX_FRAME_SIZE = 1024 * 1024 = 1,048,576` bytes).
- **Enforcement**:
  - Payloads exceeding `MAX_FRAME_SIZE` are immediately rejected at the transport layer with `LocalIpcError::FrameTooLarge`.
  - Malformed or truncated framing returns EOF or `LocalIpcError::MalformedFrame`.

### Automated Test Evidence
- `crates/omen-ipc/tests/ipc_tests.rs`:
  - `test_codec_frame_roundtrip`: Verifies bidirectional serialize/deserialize framing.
  - `test_codec_frame_too_large_rejection`: Verifies frames > 1 MiB are refused without memory exhaustion.
  - `test_codec_malformed_json_rejection`: Verifies garbage input fails closed.

---

## 2. Handshake & Version Negotiation

Connection lifecycle begins with a typed version negotiation exchange:
1. **ClientHello**:
   - `protocol_version_family`: `"omen.local-ipc"`
   - `supported_versions`: `[1]`
   - `client_instance_id`: Unique client UUID/label
   - `product_version`: Cargo package version
   - `platform`: Operating system identifier
   - `requested_features`: `["events", "services", "execution", "hot_index"]`
2. **DaemonHello**:
   - `selected_protocol_version`: Highest mutually agreed protocol version (`1`)
   - `daemon_instance_id`: Unique daemon UUID (`dmn_<uuid>`)
   - `max_frame_size`: `1,048,576`
   - `supported_features`: Advertised server capabilities
3. **Refusal on Mismatch**:
   - If family is not `"omen.local-ipc"`, daemon closes with `ProtocolVersionUnsupported`.
   - If no common version is found, connection is rejected.

### Automated Test Evidence
- `crates/omen-daemon/tests/e2e_shared_runtime_proofs.rs`:
  - `proof_i_protocol_mismatch_refusal`: Demonstrates clean refusal when client advertises unsupported version family.
- `crates/omen-ipc/tests/ipc_tests.rs`:
  - `test_handshake_version_negotiation_success`
  - `test_handshake_unsupported_family`
  - `test_handshake_unsupported_version`

---

## 3. Streaming Transport Abstraction

The `PlatformStream` abstraction provides a unified async stream across OS-specific IPC transports:
- **Windows**: Windows Named Pipes (`\\.\pipe\omen-<username>-1`) with message security and TokenUser verification.
- **Unix**: Unix Domain Sockets (`$XDG_RUNTIME_DIR/omen/1/omend.sock` or `~/.omen/run/1/omend.sock`) with filesystem permissions `0700` and `SO_PEERCRED` UID checking.
- **In-Memory Duplex**: `PlatformStream::duplex_pair(65536)` for deterministic, race-free integration testing without socket filesystem artifacts.
