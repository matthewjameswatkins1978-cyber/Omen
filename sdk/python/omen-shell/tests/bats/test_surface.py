"""High-level surface coverage through real Omen (Layer 3).

Exercises every public namespace beyond execute/history so the
transport/error/lifecycle machinery has proof, not just percentage.
"""

import pytest

from omen_shell import OmenError
from omen_shell.errors import OmenProtocolError
from omen_shell.models import ArtifactData, SemanticResult
from omen_shell.testing import OmenHarness
from tests.conftest import needs_omen


@needs_omen
def test_capabilities_and_groups() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        all_caps = omen.capabilities()
        assert len(all_caps) > 10
        ids = {c.id for c in all_caps}
        assert "execution.run" in ids
        grouped = omen.capabilities(group="semantic")
        assert grouped
        assert all(c.group == "semantic" for c in grouped)
        assert omen.capabilities(group="no-such-group") == []


@needs_omen
def test_describe_valid_and_invalid() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        projection = omen.describe("execution.run")
        assert projection
        with pytest.raises(OmenError) as exc_info:
            omen.describe("no.such.capability")
        assert exc_info.value.code == "CAPABILITY_NOT_FOUND"


@needs_omen
def test_how_valid_and_invalid() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        recipe = omen.how("execute-and-inspect")
        assert recipe.get("recipe_id", recipe.get("id")) == "execute-and-inspect"
        with pytest.raises(OmenError):
            omen.how("no-such-recipe")


@needs_omen
def test_context_snapshot_and_delta() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        snapshot = omen.context()
        assert "context_generation" in snapshot
        generation = snapshot["context_generation"]
        if generation is not None:
            same = omen.context(since=generation)
            assert same.get("changed") is False
        with pytest.raises(OmenError) as exc_info:
            omen.context(since=1 if generation != 1 else 2)
        assert exc_info.value.code == "DELTA_UNAVAILABLE"
        with pytest.raises(OmenProtocolError):
            omen.context(since=-1)  # type: ignore[arg-type]


@needs_omen
def test_workspace_status_and_facts() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        status = omen.workspace_status()
        assert "workspace_path" in status or "workspace_id" in status
        facts = omen.facts.query()
        assert isinstance(facts, facts.__class__) and isinstance(facts, list)


@needs_omen
def test_actions_namespaces() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        listing = omen.actions.list()
        assert "actions" in listing
        if listing.get("config_present") and listing["actions"]:
            action_id = listing["actions"][0]["action_id"]
            shown = omen.actions.show(action_id)
            assert shown
            plan = omen.actions.plan(action_id)
            assert "steps" in plan
        with pytest.raises(OmenError):
            omen.actions.show("no-such-action")


@needs_omen
def test_semantic_references_and_search_shapes() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        harness.write_file("refs_sample.py", "needle_value = 7\nprint(needle_value)\n")
        refs = omen.semantic.references("needle_value")
        assert isinstance(refs, SemanticResult)
        assert refs.outcome in ("FOUND", "NOT_FOUND", "AMBIGUOUS")
        assert refs.coverage in ("COMPLETE", "PARTIAL", "NONE", "UNKNOWN")
        search = omen.semantic.search("needle_value")
        assert search.operation == "symbol_search"
        with pytest.raises(OmenProtocolError):
            omen.semantic.definition("")


@needs_omen
def test_raw_resources_surface() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        resources = omen.raw.list_resources()
        uris = {r["uri"] for r in resources}
        assert "artifact://" in uris
        omen.raw.ping()


@needs_omen
def test_execution_status_shape() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        try:
            status = omen.execution_status("python-probe-request-id-1")
        except OmenError as exc:
            assert exc.code  # standalone without daemon may refuse explicitly
        else:
            assert "status" in status


@needs_omen
def test_artifact_slicing_bounds() -> None:
    with OmenHarness() as harness:
        omen = harness.omen
        result = omen.execute(["python", "-c", "print('0123456789abcdef')"])
        assert result.stdout_artifact is not None
        full = omen.artifacts.read(result.stdout_artifact)
        assert isinstance(full, ArtifactData)
        assert full.byte_length > 0
        part = omen.artifacts.read(result.stdout_artifact, offset=0, length=4)
        assert full.text[:4] == part.text
        with pytest.raises(OmenProtocolError):
            omen.artifacts.read(result.stdout_artifact, offset=-1)
        with pytest.raises(OmenProtocolError):
            omen.artifacts.read("not-a-uri")
        # Missing blob: JSON-RPC protocol truth, not a canonical OmenError.
        with pytest.raises(OmenProtocolError) as exc_missing:
            omen.artifacts.read("artifact://sha256/" + "ff" * 32)
        assert not isinstance(exc_missing.value, OmenError)
        assert "-32004" in str(exc_missing.value)


@needs_omen
def test_omen_error_to_dict_preserves_shape() -> None:
    with OmenHarness() as harness:
        with pytest.raises(OmenError) as exc_info:
            harness.omen.describe("no.such.capability")
        body = exc_info.value.to_dict()
        assert body["code"] == "CAPABILITY_NOT_FOUND"
        assert set(body) >= {
            "schema_version",
            "code",
            "message",
            "category",
            "state_changed",
            "retryability",
            "evidence",
            "details",
        }
