"""The CLI's contract: states and exit codes."""


def test_status_follows_the_daemon(daemon, input):
    assert daemon.cli("status").stdout.split("\t")[0] == "idle"
    shot = daemon.open_selector()
    assert daemon.status() == "selecting"
    input.keys("escape")
    assert shot.wait(timeout=5) == 1, "cancelled"
    daemon.wait_status("idle")


def test_region_capture_writes_the_file(daemon):
    out = daemon.tmp / "region.png"
    result = daemon.cli("shot", "region", "100,100 320x200", "--no-preview", "-o", str(out))
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == str(out)
    assert out.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n"
