"""External product test with the public harness.

The same public Omen client users receive, pointed at an isolated
workspace and isolated Omen state.
"""

from omen_shell.testing import OmenHarness

with OmenHarness() as harness:
    harness.write_file("sample.py", "answer = 42\n")
    omen = harness.omen

    result = omen.execute(["python", "-c", "print('hello')"])
    assert result.ok, result

    history = omen.history.query(limit=10)
    assert history.knows(result.execution_id), "our own work must be known"

    native = harness.run_native(["python", "-c", "print('hello')"])
    assert native.stdout.strip() == result.stdout_preview.strip()
    print("harness proof passed")
