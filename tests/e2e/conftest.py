"""Fixtures: the session (built binaries, a running headless compositor), and per test a
daemon with its own config and state, a persistent input connection, and optionally a
key logger. A failed test leaves a screenshot and the daemon log in .cache/e2e/."""

from __future__ import annotations

import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest

from harness import ROOT, SCREENIE, Daemon, Input, KeyLog, Session

ARTIFACTS = ROOT / ".cache/e2e"


@pytest.fixture(scope="session")
def session() -> Session:
    subprocess.run(["cargo", "build", "--release", "-q", "-p", "screenie"], cwd=ROOT, check=True)
    subprocess.run(
        ["cargo", "build", "--release", "-q", "--manifest-path", "tools/wlinput/Cargo.toml"], cwd=ROOT, check=True
    )
    started = subprocess.run([str(ROOT / "tools/session.sh"), "start"], capture_output=True, text=True, check=True)
    session = Session.load()
    # A daemon left over from development would own the socket.
    subprocess.run([str(SCREENIE), "quit"], env=session.env, capture_output=True, timeout=10, check=False)
    yield session
    if "already running" not in started.stdout and not os.environ.get("SCREENIE_E2E_KEEP_SESSION"):
        subprocess.run([str(ROOT / "tools/session.sh"), "stop"], capture_output=True, check=False)


@pytest.fixture(autouse=True)
def clean_desktop(session: Session) -> None:
    """Each test starts with no windows open (a failed one may leave a dialog behind)."""
    session.close_windows()


@pytest.fixture
def home(tmp_path: Path) -> Path:
    return tmp_path


@pytest.fixture
def daemon(session: Session, home: Path, input: Input, request: pytest.FixtureRequest) -> Daemon:
    """A running daemon with an empty config. Change it with `daemon.config(...)`
    (reloaded within a second) or before starting via the `config` marker.

    Comes with `input`: without a keyboard on the seat nothing gets keyboard focus, and
    the daemon's "has the keyboard" signals never come."""
    daemon = Daemon(session, home)
    marker = request.node.get_closest_marker("config")
    daemon.config(marker.args[0] if marker else "")
    daemon.start()
    yield daemon
    daemon.stop()


@pytest.fixture
def input(session: Session) -> Input:
    """Created before the test opens anything, so there's one keyboard throughout."""
    connection = Input(session)
    yield connection
    connection.close()


@pytest.fixture
def keylog(session: Session, home: Path) -> KeyLog:
    log = KeyLog(session, home / "wev.log")
    yield log
    log.close()


def pytest_configure(config: pytest.Config) -> None:
    config.addinivalue_line("markers", "config(yaml): the daemon's config.yaml for this test")


@pytest.hookimpl(wrapper=True)
def pytest_runtest_makereport(item: pytest.Item, call: pytest.CallInfo):
    report = yield
    if report.when == "call" and report.failed:
        keep_artifacts(item)
    return report


def keep_artifacts(item: pytest.Item) -> None:
    """Save what the screen looked like and what the daemon said, while it's still up."""
    dest = ARTIFACTS / re.sub(r"[^\w.-]+", "_", item.nodeid)
    shutil.rmtree(dest, ignore_errors=True)
    dest.mkdir(parents=True)
    fixtures = getattr(item, "funcargs", {})
    if session := fixtures.get("session"):
        try:
            session.screenshot(dest / "screen.png")
        except Exception as e:  # noqa: BLE001 - diagnostics only
            (dest / "screen.error").write_text(str(e))
    if daemon := fixtures.get("daemon"):
        (dest / "daemon.log").write_text(daemon.log())
    if keylog := fixtures.get("keylog"):
        shutil.copy(keylog.path, dest / "wev.log")
    item.add_report_section("call", "artifacts", str(dest))
