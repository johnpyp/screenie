# wllock

A dev tool that stands in for a fullscreen game: a fullscreen window that locks the
pointer (`zwp_pointer_constraints_v1`, persistent) when told to, for testing overlays
against pointer locks.

It prints what it gets, a line each:

- `ready` once it's on screen;
- `keyboard enter|leave` and `pointer enter|leave`;
- `locked` / `unlocked` as the compositor activates or deactivates the lock;
- `motion DX DY` for relative motion (what a game reads while locked).

It reads commands from stdin, `lock` and `unlock`, and answers each with `ok` once the
compositor has it.

```sh
(sleep 1; echo lock; sleep 60) | wllock
```

The e2e harness drives it as the `game` fixture, to check that screenie's overlays
can't trap a locked pointer (`tests/e2e/test_overlays.py`).
