#!/usr/bin/env python3
"""Detect ridge structures in heightmap: ridge pixels, ridge continuity (connectivity of adjacent ridge pixels)."""
import struct
import sys

def load_raw(path):
    raw = open(path, 'rb').read()
    return struct.unpack('<%df' % (len(raw) // 4), raw)

vals = load_raw(sys.argv[1])
n = len(vals)
w = h = int(n ** 0.5)

# Ridge = pixels higher than both sides (horizontally higher than left and right, or vertically higher than above and below)
ridge = [[False] * w for _ in range(h)]
for y in range(1, h - 1):
    for x in range(1, w - 1):
        v = vals[y * w + x]
        h_ridge = v > vals[y * w + x - 1] and v > vals[y * w + x + 1]
        v_ridge = v > vals[(y - 1) * w + x] and v > vals[(y + 1) * w + x]
        if h_ridge or v_ridge:
            ridge[y][x] = True

total_ridge = sum(sum(row) for row in ridge)
print(f'ridge pixels: {total_ridge} ({100.0 * total_ridge / n:.1f}%)')

# Connectivity: how many ridge pixels in 8-neighborhood of each ridge pixel
# Isolated point (0 neighbors) = noisy ridge; continuous ridge (>=2 neighbors) = mountain range
isolated = 0
connected = 0
for y in range(1, h - 1):
    for x in range(1, w - 1):
        if not ridge[y][x]:
            continue
        cnt = 0
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                if dx == 0 and dy == 0:
                    continue
                if ridge[y + dy][x + dx]:
                    cnt += 1
        if cnt == 0:
            isolated += 1
        elif cnt >= 2:
            connected += 1

print(f'isolated ridges (noise): {isolated} ({100.0 * isolated / max(total_ridge, 1):.1f}%)')
print(f'connected ridges (mountain): {connected} ({100.0 * connected / max(total_ridge, 1):.1f}%)')
