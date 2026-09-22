"""Semantic lookup through omen-shell.

Outcome and coverage are preserved: NOT_FOUND is a result, never None.
"""

from omen_shell import Omen

with Omen(workspace=".") as omen:
    definition = omen.semantic.definition("omen_orient")
    print(f"outcome={definition.outcome} coverage={definition.coverage}")
    if definition.found:
        print(definition.data)

    missing = omen.semantic.definition("no_such_symbol_xyz")
    print(f"missing: outcome={missing.outcome} coverage={missing.coverage}")

    refs = omen.semantic.references("omen_orient", limit=10)
    print(f"references: outcome={refs.outcome}")
