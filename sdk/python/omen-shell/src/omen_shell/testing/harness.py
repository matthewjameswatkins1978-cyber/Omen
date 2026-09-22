"""External product-test harness built on the public client.

:class:`OmenHarness` uses the same public :class:`Omen` client users
receive — no privileged secret test client. Each harness gets an
isolated workspace and, by default, an isolated Omen state root so
tests never pollute real Omen state.
"""

from __future__ import annotations

import json
import logging
import os
import shutil
import subprocess
import tempfile
import time
from collections.abc import Mapping, Sequence
from pathlib import Path
from types import TracebackType
from typing import Any

from ..client import Omen
from ..errors import OmenShellError
from ..protocol import JsonObject

_log = logging.getLogger("omen_shell")

#: Environment variable selecting the Omen executable for harness clients.
OMEN_EXE_ENV = "OMEN_EXE"
#: Supported Omen state-home override (verified against omen-knowledge).
OMEN_STATE_HOME_ENV = "OMEN_STATE_HOME"
#: Optional directory where harness evidence is retained.
EVIDENCE_DIR_ENV = "OMEN_TEST_EVIDENCE_DIR"


class OmenHarness:
    """Isolated Omen test environment with a connected public client."""

    def __init__(
        self,
        workspace: str | os.PathLike[str] | None = None,
        *,
        executable: str | None = None,
        state_home: str | os.PathLike[str] | None = None,
        isolate_state: bool = True,
        evidence_name: str | None = None,
        request_timeout: float = 120.0,
    ) -> None:
        self._owned_workspace = workspace is None
        self._workspace_dir: tempfile.TemporaryDirectory[str] | None = None
        if workspace is None:
            self._workspace_dir = tempfile.TemporaryDirectory(prefix="omen-harness-ws-")
            self._workspace = self._workspace_dir.name
        else:
            self._workspace = os.fspath(workspace)
        self._owned_state = isolate_state and state_home is None
        self._state_dir: tempfile.TemporaryDirectory[str] | None = None
        if state_home is not None:
            self._state_home: str | None = os.fspath(state_home)
        elif isolate_state:
            self._state_dir = tempfile.TemporaryDirectory(prefix="omen-harness-state-")
            self._state_home = self._state_dir.name
        else:
            self._state_home = None
        self._executable = executable
        self._evidence_name = evidence_name
        self._request_timeout = request_timeout
        self._omen: Omen | None = None
        self._evidence: list[JsonObject] = []

    @property
    def workspace(self) -> str:
        """The isolated (or explicit) workspace path."""
        return self._workspace

    @property
    def state_home(self) -> str | None:
        """The isolated state root, if isolation is enabled."""
        return self._state_home

    @property
    def omen(self) -> Omen:
        """The connected public client. Open the harness first."""
        if self._omen is None:
            raise OmenShellError("harness is not open; use 'with OmenHarness() as h'")
        return self._omen

    def write_file(self, relpath: str, content: str) -> str:
        """Write a fixture file under the workspace; return its path."""
        target = Path(self._workspace) / relpath
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content, encoding="utf-8")
        return str(target)

    def run_native(self, argv: Sequence[str], **kwargs: Any) -> subprocess.CompletedProcess[str]:
        """Run a command natively (no Omen) for parity comparisons.

        Testing infrastructure only — not part of normal client semantics.
        """
        if isinstance(argv, str) or not list(argv):
            raise OmenShellError("run_native requires a non-empty argv sequence")
        timeout = kwargs.pop("timeout", 120.0)
        # argv list, never shell=True; caller-provided test commands only.
        return subprocess.run(  # noqa: S603
            list(argv),
            cwd=kwargs.pop("cwd", self._workspace),
            capture_output=True,
            text=True,
            timeout=timeout,
            **kwargs,
        )

    def snapshot_processes(self) -> set[tuple[str, int]]:
        """Snapshot ``omen``-related processes (name, pid).

        Requires ``psutil`` (a dev/test dependency). Falls back to an
        empty set with a log record when psutil is unavailable.
        """
        try:
            import psutil
        except ImportError:
            _log.debug("omen_shell: psutil unavailable; process snapshot skipped")
            return set()
        found: set[tuple[str, int]] = set()
        for proc in psutil.process_iter(["name", "exe"]):
            try:
                name = (proc.info.get("name") or "").lower()
                exe = (proc.info.get("exe") or "").lower()
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                continue
            if "omen" in name or "omen" in exe:
                found.add((name, proc.pid))
        return found

    def wait_for(
        self,
        predicate: Any,
        *,
        timeout: float = 30.0,
        poll: float = 0.25,
        what: str = "condition",
    ) -> Any:
        """Poll ``predicate()`` until truthy; fail with evidence on timeout."""
        deadline = time.monotonic() + timeout
        last: Any = None
        while time.monotonic() < deadline:
            last = predicate()
            if last:
                return last
            time.sleep(poll)
        raise OmenShellError(f"timed out waiting for {what} after {timeout}s (last={last!r})")

    def record_evidence(self, label: str, payload: Mapping[str, Any]) -> None:
        """Record evidence kept when evidence mode is enabled."""
        entry: JsonObject = {"label": label, "payload": dict(payload)}
        self._evidence.append(entry)

    def open(self) -> OmenHarness:
        """Create dirs, connect the public client, return self."""
        if self._omen is not None:
            return self
        Path(self._workspace).mkdir(parents=True, exist_ok=True)
        env: dict[str, str] | None = None
        if self._state_home is not None:
            env = {OMEN_STATE_HOME_ENV: self._state_home}
        self._omen = Omen(
            workspace=self._workspace,
            executable=self._executable,
            request_timeout=self._request_timeout,
            env=env,
        )
        return self

    def close(self) -> None:
        """Close the client, persist evidence if configured, clean up."""
        if self._omen is not None:
            try:
                self._omen.close()
            finally:
                self._omen = None
        self._persist_evidence()
        if self._workspace_dir is not None:
            self._workspace_dir.cleanup()
            self._workspace_dir = None
        if self._state_dir is not None:
            self._state_dir.cleanup()
            self._state_dir = None

    def _persist_evidence(self) -> None:
        evidence_dir = os.environ.get(EVIDENCE_DIR_ENV)
        if not evidence_dir or not self._evidence:
            return
        name = self._evidence_name or f"harness-{int(time.time())}"
        target = Path(evidence_dir) / f"{name}.json"
        try:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps(self._evidence, indent=2), encoding="utf-8")
        except OSError as exc:
            _log.debug("omen_shell: could not persist evidence: %s", exc)

    def omen_processes_running(self) -> list[str]:
        """Names of omen executables found on PATH (diagnostic helper)."""
        found = shutil.which("omen")
        return [found] if found else []

    def __enter__(self) -> OmenHarness:
        return self.open()

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> None:
        self.close()
