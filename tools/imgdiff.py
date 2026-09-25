#!/usr/bin/env python3
"""Compare two images pixel-wise. Usage: imgdiff.py a.png b.png  -> prints size and diff stats.
Exit 0 if identical (RGB), 1 otherwise."""
import sys
import numpy as np
from PIL import Image

a = np.asarray(Image.open(sys.argv[1]).convert("RGB")).astype(int)
b = np.asarray(Image.open(sys.argv[2]).convert("RGB")).astype(int)
if a.shape != b.shape:
    print(f"shape differs: {a.shape} vs {b.shape}")
    sys.exit(1)
d = np.abs(a - b)
n = int((d.max(axis=2) > 0).sum())
print(f"shape {a.shape}, differing pixels {n} ({100*n/(a.shape[0]*a.shape[1]):.3f}%), max delta {d.max()}")
sys.exit(0 if n == 0 else 1)
