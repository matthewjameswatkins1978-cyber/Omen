"""Nox sessions for omen-shell. Nox is dev tooling only; never required to use the package."""

import nox

PYTHONS = ["3.11", "3.12", "3.13", "3.14"]
PACKAGE = "sdk/python/omen-shell"


@nox.session(python=PYTHONS)
def unit(session: nox.Session) -> None:
    session.install("pytest")
    session.install("-e", PACKAGE)
    session.run("pytest", f"{PACKAGE}/tests/unit", "-q")


@nox.session(python=PYTHONS)
def integration(session: nox.Session) -> None:
    session.install("pytest")
    session.install("-e", PACKAGE)
    session.run("pytest", f"{PACKAGE}/tests/integration", "-q")


@nox.session(python=["3.11", "3.14"])
def bats(session: nox.Session) -> None:
    session.install("pytest", "psutil")
    session.install("-e", PACKAGE)
    session.run("pytest", f"{PACKAGE}/tests/bats", "-q")


@nox.session(python="3.13")
def stateful(session: nox.Session) -> None:
    session.install("pytest", "hypothesis")
    session.install("-e", PACKAGE)
    session.run("pytest", f"{PACKAGE}/tests/stateful", "-q")


@nox.session(python="3.13")
def lint(session: nox.Session) -> None:
    session.install("ruff")
    session.run("ruff", "check", PACKAGE)
    session.run("ruff", "format", "--check", PACKAGE)


@nox.session(python="3.13")
def typecheck(session: nox.Session) -> None:
    session.install("mypy")
    session.run("mypy", f"{PACKAGE}/src")


@nox.session(python="3.13")
def build(session: nox.Session) -> None:
    session.install("build", "twine")
    session.run("python", "-m", "build", PACKAGE)
    session.run("twine", "check", f"{PACKAGE}/dist/*")
