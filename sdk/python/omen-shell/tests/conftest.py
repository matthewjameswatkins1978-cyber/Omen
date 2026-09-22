"""Pytest fixtures for real-Omen tests (Layers 3/4).

Real-Omen tests need an ``omen`` executable: explicit ``OMEN_EXE``
env, or ``omen`` on PATH. They are skipped (not failed) when absent
so unit/transport layers stay runnable everywhere.
"""

import shutil

import pytest

omen_exe = shutil.which("omen")

needs_omen = pytest.mark.skipif(
    omen_exe is None,
    reason="no Omen executable (set OMEN_EXE or put omen on PATH)",
)
