"""Public-API-only smoke test: no `omen_shell._internal` imports.

Exercises connect, orient, execute, evidence, history, one semantic
lookup, and close through documented imports alone.
"""

import asyncio
import sys
import tempfile

from omen_shell import (
    ArtifactRef,
    AsyncOmen,  # noqa: F401 (import surface proof)
    ExecutionResult,
    HistoryResult,
    Omen,
    OmenError,
    OmenShellError,
    SemanticResult,
)


def main() -> int:
    workspace = tempfile.mkdtemp(prefix="omen-public-api-")
    with Omen(workspace=workspace) as omen:
        info = omen.orient()
        assert info.contract_version == "0.8", info.contract_version

        result: ExecutionResult = omen.execute(["python", "-c", "print('public-api')"])
        assert isinstance(result.execution_id, str) and result.execution_id
        assert result.ok, result

        assert isinstance(result.stdout_artifact, ArtifactRef)
        data = omen.artifacts.read(result.stdout_artifact)
        assert "public-api" in data.text

        history: HistoryResult = omen.history.query(limit=5)
        assert history.knows(result.execution_id)

        semantic: SemanticResult = omen.semantic.definition("no_such_symbol_xyz")
        assert semantic.outcome == "NOT_FOUND"

        async def _async_probe() -> None:
            async with AsyncOmen(workspace=workspace) as aomen:
                r = await aomen.execute(["python", "-c", "print('async-ok')"])
                assert r.ok

        asyncio.run(_async_probe())

    try:
        with Omen(workspace=workspace) as omen:
            omen.describe("no.such.capability")
    except OmenError as exc:
        assert exc.code == "CAPABILITY_NOT_FOUND"
    except OmenShellError as exc:
        raise AssertionError("expected OmenError, not a transport failure") from exc
    print("PUBLIC_API_SMOKE: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
