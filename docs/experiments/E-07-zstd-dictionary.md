# E-07: zstd with a trained dictionary

- Status: **to do**

**Question:** Would zstd with a dictionary trained on terminal output beat gzip on small 50 ms frames?

**How:** Capture 10 MB of real agent output. Train a 64 KB zstd dictionary and compare ratio and CPU against gzip-fast per frame. The .NET side would need ZstdSharp.

**Pass / decision rule:** Adopt only if it is ≥30% smaller at equal CPU; it adds a dependency on both sides.
