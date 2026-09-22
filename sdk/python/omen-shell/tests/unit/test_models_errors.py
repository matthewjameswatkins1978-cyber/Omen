"""Unit tests: pure Python, no Omen required (Layer 1)."""

import shutil

from _pytest.monkeypatch import MonkeyPatch

from omen_shell import (
    ArtifactRef,
    ExecutionId,
    ExecutionResult,
    HistoryResult,
    OmenError,
    SemanticResult,
)
from omen_shell.compatibility import SUPPORTED_MACHINE_CONTRACTS, check_contract
from omen_shell.errors import (
    OmenCompatibilityError,
    OmenConnectionClosedError,
    OmenNotFoundError,
    OmenProcessError,
    OmenProtocolError,
    OmenRequestTimeout,
    OmenShellError,
    OmenTransportError,
)
from omen_shell.transport import resolve_executable, start_command


def test_error_hierarchy() -> None:
    for cls in (
        OmenNotFoundError,
        OmenTransportError,
        OmenProtocolError,
        OmenCompatibilityError,
        OmenConnectionClosedError,
        OmenRequestTimeout,
        OmenError,
    ):
        assert issubclass(cls, OmenShellError)
    assert not issubclass(OmenProcessError, OmenError)


def test_omen_error_round_trip_lossless() -> None:
    envelope = {
        "schema_version": 1,
        "code": "FUTURE_CODE",
        "message": "m",
        "category": "Validation",
        "state_changed": "NO",
        "retryability": "NEVER",
        "evidence": {"available": False, "artifacts": []},
        "details": {"x": 1},
        "brand_new_field": "kept?",
    }
    err = OmenError.from_envelope(envelope)
    assert err.code == "FUTURE_CODE"
    assert err.category == "Validation"
    assert err.state_changed == "NO"
    assert err.retryability == "NEVER"
    assert err.evidence == {"available": False, "artifacts": []}
    assert err.details == {"x": 1}
    assert err.raw["brand_new_field"] == "kept?"
    assert err.to_dict()["code"] == "FUTURE_CODE"


def test_omen_error_unknown_values_stay_unknown() -> None:
    err = OmenError.from_envelope({"code": "X", "message": "y"})
    assert err.category == "Unknown"
    assert err.retryability == "UNKNOWN"
    assert err.state_changed == "UNKNOWN"


def test_execution_result_ok_definition() -> None:
    base = {
        "execution_id": "exec_1",
        "runtime_status": "COMPLETED",
        "exit_code": 0,
    }
    assert ExecutionResult.from_payload(dict(base)).ok is True
    assert ExecutionResult.from_payload({**base, "exit_code": 2}).ok is False
    assert ExecutionResult.from_payload({**base, "runtime_status": "TIMED_OUT"}).ok is False
    assert ExecutionResult.from_payload({**base, "exit_code": None}).ok is False


def test_child_nonzero_returns_result_not_error() -> None:
    result = ExecutionResult.from_payload(
        {
            "execution_id": "exec_2",
            "runtime_status": "COMPLETED",
            "exit_code": 2,
            "stdout_preview": "out",
            "stderr_preview": "err",
        }
    )
    assert result.exit_code == 2
    assert result.stdout_preview == "out"
    try:
        result.raise_for_status()
    except OmenProcessError as exc:
        assert exc.result is result
    else:
        raise AssertionError("raise_for_status must raise for exit 2")


def test_execution_result_extra_preserved() -> None:
    result = ExecutionResult.from_payload(
        {
            "execution_id": "exec_3",
            "runtime_status": "COMPLETED",
            "exit_code": 0,
            "future_field": "here",
        }
    )
    assert result.extra == {"future_field": "here"}
    assert result.to_dict()["future_field"] == "here"


def test_execution_result_requires_identity() -> None:
    try:
        ExecutionResult.from_payload({"runtime_status": "COMPLETED"})
    except OmenProtocolError:
        pass
    else:
        raise AssertionError("missing execution_id must fail")


def test_artifact_ref_parse() -> None:
    ref = ArtifactRef.parse("artifact://sha256/" + "ab" * 32)
    assert ref.algorithm == "sha256"
    assert ref.digest == "ab" * 32
    assert str(ref) == ref.uri
    for bad in ("", "http://x", "artifact://", "artifact://sha256/", "artifact://onlyone"):
        try:
            ArtifactRef.parse(bad)
        except OmenProtocolError:
            pass
        else:
            raise AssertionError(f"must reject {bad!r}")


def test_history_wrapped_vs_bare() -> None:
    bare = HistoryResult.from_payload({"entries": [], "limit": 20})
    assert bare.history_status == "CURRENT"
    assert bare.local_execution is None
    wrapped = HistoryResult.from_payload(
        {
            "history": {"entries": []},
            "history_status": "UNJOURNALED_LOCAL_EXECUTION",
            "local_execution": {"command": ["rg"]},
        }
    )
    assert wrapped.history_status == "UNJOURNALED_LOCAL_EXECUTION"
    assert wrapped.local_execution == {"command": ["rg"]}


def test_semantic_not_found_is_result() -> None:
    result = SemanticResult.from_payload(
        {"operation": "definition", "outcome": "NOT_FOUND", "coverage": "NONE", "data": {}}
    )
    assert result.found is False
    assert result.outcome == "NOT_FOUND"
    assert result.coverage == "NONE"


def test_semantic_unknown_values_preserved() -> None:
    result = SemanticResult.from_payload(
        {"operation": "definition", "outcome": "MAYBE_LATER", "coverage": "SOMEHOW"}
    )
    assert result.outcome == "MAYBE_LATER"
    assert result.coverage == "SOMEHOW"
    assert result.found is False


def test_compat_gate() -> None:
    assert "0.8" in SUPPORTED_MACHINE_CONTRACTS
    assert check_contract("0.8", runtime_version="x") == "0.8"
    try:
        check_contract("99.99", runtime_version="x")
    except OmenCompatibilityError as exc:
        assert exc.found_contract == "99.99"
        assert "0.8" in exc.supported_contracts
    else:
        raise AssertionError("unsupported contract must raise")


def test_start_command_is_argv_not_shell() -> None:
    argv = start_command("omen", "ws-dir")
    assert argv == ["omen", "mcp", "--workspace", "ws-dir"]
    assert all(isinstance(a, str) for a in argv)


def test_resolve_executable_override(monkeypatch: MonkeyPatch) -> None:
    monkeypatch.setenv("OMEN_EXE", "/custom/omen")
    assert resolve_executable() == "/custom/omen"
    assert resolve_executable("/explicit/omen") == "/explicit/omen"
    monkeypatch.delenv("OMEN_EXE")


def test_resolve_executable_missing(monkeypatch: MonkeyPatch) -> None:
    monkeypatch.delenv("OMEN_EXE", raising=False)
    monkeypatch.setattr(shutil, "which", lambda *a: None)
    try:
        resolve_executable()
    except OmenNotFoundError as exc:
        assert "OMEN_EXE" in str(exc)
    else:
        raise AssertionError("must raise when no omen found")


def test_execution_id_is_distinct_type() -> None:
    eid: ExecutionId = ExecutionId("exec_abc")
    assert isinstance(eid, str)
    assert str(eid) == "exec_abc"
