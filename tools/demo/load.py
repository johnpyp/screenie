#!/usr/bin/env python3
"""A gently varying CPU load, so btop's graphs have something to show on camera.

tools/demo/load.py [WORKERS]   # until killed
"""

import math
import multiprocessing
import sys
import time


def worker(i):
    phase = i * 0.9
    while True:
        t = time.monotonic()
        duty = 0.45 + 0.4 * math.sin(t / 3.5 + phase) * math.sin(t / 9.0 + phase / 2)
        end = t + 0.05 * max(0.05, duty)
        while time.monotonic() < end:
            pass
        time.sleep(0.05 * (1 - duty))


if __name__ == "__main__":
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 10
    procs = [multiprocessing.Process(target=worker, args=(i,), daemon=True) for i in range(n)]
    for p in procs:
        p.start()
    try:
        for p in procs:
            p.join()
    except KeyboardInterrupt:
        pass
