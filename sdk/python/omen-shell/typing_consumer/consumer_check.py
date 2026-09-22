"""External-consumer typing proof for omen-shell (Repair 4).

Checked by mypy --strict in CI against the INSTALLED package (not the
source tree), proving what a real consumer's type checker sees. Never
executed by pytest: module-level code that would spawn processes lives
behind `main()`.

Each `assert_type` fails the typecheck if the sync namespace API
collapses to `Any` — a dynamic `__getattr__` proxy cannot pass this.
"""

from typing import assert_type

from omen_shell import Omen
from omen_shell.models import ArtifactData, HistoryResult, SemanticResult
from omen_shell.protocol import JsonObject, JsonValue


def exercise(omen: Omen) -> None:
    assert_type(omen.artifacts.read("artifact://sha256/" + "00" * 32), ArtifactData)
    assert_type(omen.semantic.definition("some_symbol"), SemanticResult[JsonValue])
    assert_type(omen.history.query(), HistoryResult)
    assert_type(omen.actions.plan("some-action"), JsonObject)
    assert_type(omen.raw.list_tools(), list[JsonObject])
    assert_type(omen.facts.query(), list[JsonObject])
    assert_type(omen.describe("execution.run"), JsonObject)
    assert_type(omen.context(), JsonObject)


def main() -> None:
    with Omen(workspace=".") as omen:
        exercise(omen)


if __name__ == "__main__":
    main()
