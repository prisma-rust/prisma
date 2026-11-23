#!/usr/bin/env python3
"""Analyze heightmap terrain structure: spatial autocorrelation, gradient distribution, terrain relief stats."""
import struct
import sys
import statistics

def load_raw(path):
    raw = open(path, 'rb').read()
    return struct.unpack('<%df' % (len(raw) // 4), raw)

def analyze(path, label):
    vals = load_raw(path)
    n = len(vals)
    w = h = int(n ** 0.5)
    print(f'=== {label} ({w}x{h}) ===')
    s = sorted(vals)
    q = lambda p: s[int(n * p / 100)]
    print(f'  quantiles: p1={q(1):.0f} p10={q(10):.0f} p25={q(25):.0f} '
          f'p50={q(50):.0f} p75={q(75):.0f} p90={q(90):.0f} p99={q(99):.0f} '
          f'min={s[0]:.0f} max={s[-1]:.0f}')
        # Adjacent pixel difference (horizontal direction) — terrain roughness
    hdiffs = [abs(vals[y*w+x] - vals[y*w+x+1]) for y in range(h) for x in range(w-1)]
        # Large-scale relief sampled every 8 pixels (mountain range scale)
    big = [abs(vals[y*w+x] - vals[y*w+x+8]) for y in range(h) for x in range(w-8)]
    print(f'  adj diff: mean={statistics.mean(hdiffs):.0f} '
          f'p90={sorted(hdiffs)[int(len(hdiffs)*0.9)]:.0f}')
        print(f'  8px diff (mountain scale): mean={statistics.mean(big):.0f} '
          f'p90={sorted(big)[int(len(big)*0.9)]:.0f}')
        # Sea level ratio
    sea = sum(1 for v in vals if v < 0) / n * 100
    print(f'  sea: {sea:.1f}%')

analyze(sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else sys.argv[1])
