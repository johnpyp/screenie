"""Building blocks for driving screenie in the headless sway session (tools/session.sh).

Everything waits on events (status, log lines, windows, focus changes), never on
fixed sleeps. See README.md for the fixtures built from these.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import TypeVar

from PIL import Image

ROOT = Path(__file__).resolve().parents[2]
SCREENIE = ROOT / "target/release/screenie"
WLINPUT = ROOT / "target/release/wlinput"
ANSI = re.compile(r"\x1b\[[0-9;]*m")

T = TypeVar("T")


def wait_for(check: Callable[[], T], what: str, timeout: float = 5.0, interval: float = 0.02) -> T:
    """Poll `check` until it returns something truthy, and return that."""
    deadline = time.monotonic() + timeout
    while True:
        value = check()
        if value:
            return value
        if time.monotonic() > deadline:
            raise TimeoutError(f"timed out after {timeout}s waiting for {what}")
        time.sleep(interval)


@dataclass(frozen=True)
class Rect:
    x: float
    y: float
    width: float
    height: float

    @property
    def center(self) -> tuple[float, float]:
        return (self.x + self.width / 2, self.y + self.height / 2)


@dataclass(frozen=True)
class Output:
    name: str
    rect: Rect  # logical, in the layout
    scale: float


class Session:
    """The headless compositor: its environment, outputs and windows."""

    def __init__(self, env: dict[str, str]):
        self.env = env

    @classmethod
    def load(cls) -> Session:
        runtime = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
        env_file = Path(runtime) / "screenie-session.env"
        env = dict(os.environ)
        for line in env_file.read_text().splitlines():
            if line.startswith("export "):
                key, _, value = line.removeprefix("export ").partition("=")
                env[key] = value
            elif line.startswith("unset "):
                env.pop(line.removeprefix("unset ").strip(), None)
        return cls(env)

    def swaymsg(self, *args: str) -> object:
        out = subprocess.run(["swaymsg", "-r", *args], env=self.env, capture_output=True, text=True, check=True)
        return json.loads(out.stdout) if out.stdout.strip() else None

    def outputs(self) -> dict[str, Output]:
        return {
            o["name"]: Output(o["name"], Rect(**{k: o["rect"][k] for k in ("x", "y", "width", "height")}), o["scale"])
            for o in self.swaymsg("-t", "get_outputs")
        }

    def windows(self) -> list[dict]:
        """Every window (tiled or floating), with its name, app_id and focus."""
        found: list[dict] = []

        def walk(node: dict) -> None:
            if node.get("type") in ("con", "floating_con") and node.get("pid"):
                found.append(node)
            for child in node.get("nodes", []) + node.get("floating_nodes", []):
                walk(child)

        walk(self.swaymsg("-t", "get_tree"))
        return found

    def window(self, name: str) -> dict | None:
        return next((w for w in self.windows() if w.get("name") == name or w.get("app_id") == name), None)

    def close_windows(self) -> None:
        """Close every window, e.g. dialogs a failed test left open, and wait until they're gone."""
        for w in self.windows():
            self.swaymsg(f"[con_id={w['id']}] kill")
        wait_for(lambda: not self.windows(), "stray windows to close")

    def focus(self, app_id: str) -> None:
        self.swaymsg(f'[app_id="{app_id}"] focus')

    def screenshot(self, path: Path) -> Screenshot:
        """The whole layout at one pixel per logical pixel, so coordinates are logical."""
        subprocess.run(["grim", "-s", "1", str(path)], env=self.env, check=True)
        return Screenshot(Image.open(path).convert("RGB"), path)


class Screenshot:
    def __init__(self, image: Image.Image, path: Path):
        self.image = image
        self.path = path

    def pixel(self, x: float, y: float) -> tuple[int, int, int]:
        return self.image.getpixel((int(x), int(y)))

    def brightness(self, x: float, y: float) -> float:
        return sum(self.pixel(x, y)) / 3


class Daemon:
    """A screenie daemon with its own config and state directories."""

    def __init__(self, session: Session, home: Path):
        self.session = session
        self.config_home = home / "config"
        self.state_home = home / "state"
        self.tmp = home / "tmp"
        for d in (self.config_home / "screenie", self.state_home, self.tmp):
            d.mkdir(parents=True, exist_ok=True)
        self.home = home
        self.log_path = home / "daemon.log"
        # HOME too, so default folders (~/Pictures/Screenshots…) land in the test's.
        self.env = {
            **session.env,
            "HOME": str(home),
            "XDG_CONFIG_HOME": str(self.config_home),
            "XDG_STATE_HOME": str(self.state_home),
            "TMPDIR": str(self.tmp),
            "SCREENIE_LOG": "debug,gpui=info,gpui_linux=info,gpui_wgpu=warn,naga=warn,zbus=warn,wgpu_core=warn,wgpu_hal=warn",
        }
        self.process: subprocess.Popen | None = None

    @property
    def config_file(self) -> Path:
        return self.config_home / "screenie/config.yaml"

    @property
    def screenshots(self) -> list[Path]:
        """Screenshots saved to the default folder."""
        return sorted((self.home / "Pictures/Screenshots").glob("*.png"))

    @property
    def state_file(self) -> Path:
        return self.state_home / "screenie/state.yaml"

    def config(self, text: str) -> None:
        self.config_file.write_text(text)

    def start(self) -> None:
        log = self.log_path.open("a")
        self.process = subprocess.Popen(
            [str(SCREENIE), "daemon"], env=self.env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True
        )
        wait_for(lambda: self.status_json().get("pid") == self.process.pid, "the daemon to answer", timeout=10)

    def stop(self) -> None:
        if not self.process:
            return
        if self.process.poll() is None:
            subprocess.run([str(SCREENIE), "quit"], env=self.env, capture_output=True, timeout=5, check=False)
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        self.process = None

    def restart(self) -> None:
        self.stop()
        self.start()

    @property
    def pid(self) -> int | None:
        return self.process.pid if self.process and self.process.poll() is None else None

    def cli(self, *args: str, timeout: float = 15) -> subprocess.CompletedProcess:
        """Run `screenie ARGS` to completion."""
        return subprocess.run(
            [str(SCREENIE), *args], env=self.env, capture_output=True, text=True, timeout=timeout, check=False
        )

    def spawn(self, *args: str) -> subprocess.Popen:
        """Start `screenie ARGS` without waiting (e.g. `shot --edit`, which lasts the edit)."""
        return subprocess.Popen(
            [str(SCREENIE), *args], env=self.env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True
        )

    def status_json(self) -> dict:
        out = self.cli("status", "--json")
        return json.loads(out.stdout) if out.returncode == 0 and out.stdout.strip() else {}

    def status(self) -> str:
        return self.status_json().get("state", "")

    def wait_status(self, state: str, timeout: float = 5.0) -> None:
        wait_for(lambda: self.status() == state, f"status {state!r} (last: {self.status()!r})", timeout)

    def log(self) -> str:
        return ANSI.sub("", self.log_path.read_text()) if self.log_path.exists() else ""

    def mark(self) -> int:
        """A position in the log, for `wait_log(after=...)`."""
        return len(self.log())

    def wait_log(self, pattern: str, after: int = 0, timeout: float = 5.0) -> int:
        """Wait for a log line past `after` matching `pattern`; return the mark past it."""
        regex = re.compile(pattern, re.MULTILINE)

        def find() -> int | None:
            match = regex.search(self.log(), after)
            return match.end() if match else None

        return wait_for(find, f"daemon log /{pattern}/", timeout)

    def open_editor(self, *args: str) -> subprocess.Popen:
        """`screenie shot ARGS --edit`, once the editor has the keyboard."""
        mark = self.mark()
        shot = self.spawn("shot", *args, "--edit")
        self.wait_log(r"editor keyboard focus active=true", after=mark)
        return shot

    def open_selector(self, *args: str) -> subprocess.Popen:
        """`screenie shot ARGS` (interactive), once the selector has the keyboard."""
        mark = self.mark()
        shot = self.spawn("shot", *args)
        self.wait_log(r"selector keyboard focus .*active=true", after=mark)
        return shot

    def assert_healthy(self) -> None:
        """Still running, and its Wayland connection never broke."""
        assert self.pid is not None, "the daemon exited"
        assert "Protocol error" not in self.log(), "the daemon hit a Wayland protocol error"


class Input:
    """One virtual pointer and keyboard for a whole test (`wlinput -`).

    One keyboard matters: a keyboard added while an overlay has the keyboard makes sway
    re-enter the focused window, which muddles focus-sensitive tests.
    """

    def __init__(self, session: Session):
        self.process = subprocess.Popen(
            [str(WLINPUT), "-"], env=session.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1
        )
        assert self.process.stdout.readline().strip() == "ready", "wlinput didn't start"

    def do(self, commands: str) -> None:
        """Run a wlinput command chain (`key escape , click 10 10`) and wait until delivered."""
        self.process.stdin.write(commands + "\n")
        self.process.stdin.flush()
        reply = self.process.stdout.readline().strip()
        if reply != "ok":
            raise RuntimeError(f"wlinput {commands!r}: {reply or 'exited'}")

    def keys(self, *names: str) -> None:
        self.do("key " + " ".join(names))

    def chord(self, *modifiers: str, key: str) -> None:
        """Press `key` with `modifiers` held, e.g. chord("ctrl", "shift", key="s")."""
        held = " , ".join(f"hold {m}" for m in modifiers)
        released = " , ".join(f"release {m}" for m in reversed(modifiers))
        self.do(f"{held} , key {key} , {released}")

    def type(self, text: str) -> None:
        self.do("type " + text)

    def move(self, x: float, y: float) -> None:
        self.do(f"move {x} {y}")

    def click(self, x: float, y: float) -> None:
        self.do(f"click {x} {y}")

    def drag(self, x1: float, y1: float, x2: float, y2: float) -> None:
        self.do(f"drag {x1} {y1} {x2} {y2}")

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.stdin.close()
            self.process.wait(timeout=5)


class KeyLog:
    """A `wev` window logging the keyboard events it receives, to catch keys that
    leak out of screenie's overlays into the app beneath."""

    def __init__(self, session: Session, path: Path):
        if not shutil.which("wev"):
            raise RuntimeError("wev is needed for key-leak tests")
        self.session = session
        self.path = path
        self.out = path.open("w")
        cmd = "stdbuf -oL wev -f wl_keyboard:key -f wl_keyboard:enter -f wl_keyboard:leave"
        self.process = subprocess.Popen(shlex.split(cmd), env=session.env, stdout=self.out, stderr=subprocess.STDOUT)
        wait_for(lambda: session.window("wev"), "the wev window")

    def lines(self) -> list[str]:
        return self.path.read_text().splitlines()

    def mark(self) -> int:
        return len(self.lines())

    def wait(self, pattern: str, after: int, timeout: float = 5.0) -> int:
        """Wait for a line past mark `after` matching `pattern`; return the mark just past it."""

        def find() -> int | None:
            for i, line in enumerate(self.lines()[after:], start=after):
                if re.search(pattern, line):
                    return i + 1
            return None

        return wait_for(find, f"wev /{pattern}/", timeout)

    def since(self, mark: int) -> list[str]:
        return self.lines()[mark:]

    def focus(self) -> None:
        self.session.focus("wev")

    def close(self) -> None:
        self.process.send_signal(signal.SIGTERM)
        self.process.wait(timeout=5)
        self.out.close()
