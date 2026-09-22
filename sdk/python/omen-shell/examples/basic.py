"""Basic omen-shell usage: connect, orient, execute, inspect evidence."""

from omen_shell import Omen

with Omen(workspace=".") as omen:
    info = omen.orient()
    print(f"Omen {info.omen_version} (contract {info.contract_version})")

    result = omen.execute(["python", "-c", "print('hello from Omen')"])
    print(f"execution: {result.execution_id} exit={result.exit_code} ok={result.ok}")
    print(f"stdout: {result.stdout_preview.strip()!r}")

    if result.stdout_artifact is not None:
        data = omen.artifacts.read(result.stdout_artifact)
        print(f"artifact bytes: {data.byte_length}")

    history = omen.history.query(limit=5)
    print(f"history: {history.history_status} ({len(history.entries)} durable entries)")
