"""omen-shell: the official Python interface to Omen.

Python provides orchestration. Omen provides machine truth.
"""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError, version

from .async_client import AsyncOmen
from .client import Omen
from .errors import OmenError, OmenShellError
from .models import (
    ArtifactRef,
    ExecutionId,
    ExecutionResult,
    HistoryResult,
    LocalExecution,
    SemanticResult,
)

try:
    __version__ = version("omen-shell")
except PackageNotFoundError:  # source tree without install metadata
    try:
        import tomllib
        from pathlib import Path

        _pyproject = Path(__file__).resolve().parents[2] / "pyproject.toml"
        __version__ = str(
            tomllib.loads(_pyproject.read_text(encoding="utf-8"))["project"]["version"]
        )
    except (OSError, ValueError, KeyError):
        __version__ = "0.0.0+unknown"

__all__ = [
    "Omen",
    "AsyncOmen",
    "OmenError",
    "OmenShellError",
    "ExecutionResult",
    "SemanticResult",
    "ArtifactRef",
    "ExecutionId",
    "HistoryResult",
    "LocalExecution",
    "__version__",
]
