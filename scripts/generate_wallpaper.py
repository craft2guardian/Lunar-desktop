#!/usr/bin/env python3
"""Generate the bundled wallpaper: an original, Sonoma-inspired abstract landscape.

Soft twilight sky, a low warm sun glow and layered rolling hills with
atmospheric haze. It is procedural artwork, not a copy of Apple's wallpaper.

    python3 scripts/generate_wallpaper.py            # writes assets/wallpaper.jpg

Requires: numpy, Pillow.
"""
from pathlib import Path

import numpy as np
from PIL import Image

W, H = 3840, 2160
OUT = Path(__file__).resolve().parent.parent / "assets" / "wallpaper.jpg"

rng = np.random.default_rng(2026)

u = np.linspace(0.0, 1.0, W, dtype=np.float32)          # 0..1 across the width
v = np.linspace(0.0, 1.0, H, dtype=np.float32)[:, None]  # 0..1 down the height
aspect = W / H


def smoothstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def mix(a, b, t):
    """Blend colour `a` into `b` by t. a: (H,W,3) or (3,), t: (H,W) or (1,W)."""
    return a + (np.asarray(b, np.float32) - a) * t[..., None]


def gradient(stops, x):
    pos = [s[0] for s in stops]
    return np.stack(
        [np.interp(x, pos, [s[1][c] for s in stops]) for c in range(3)], axis=-1
    ).astype(np.float32)


# ---------------------------------------------------------------- sky
sky_stops = [
    (0.00, (0.20, 0.26, 0.56)),
    (0.22, (0.40, 0.42, 0.72)),
    (0.40, (0.74, 0.58, 0.78)),
    (0.55, (0.98, 0.70, 0.66)),
    (0.70, (1.00, 0.84, 0.62)),
    (1.00, (1.00, 0.88, 0.66)),
]
sky = gradient(sky_stops, v[:, 0])                      # (H,3)
img = np.broadcast_to(sky[:, None, :], (H, W, 3)).copy()

# Soft sun glow, low on the right-hand side.
sun_u, sun_v = 0.70, 0.40
d = np.sqrt(((u[None, :] - sun_u) * aspect) ** 2 + (v - sun_v) ** 2)
glow = np.exp(-((d / 0.30) ** 2)) * 0.42 + np.exp(-((d / 0.07) ** 2)) * 0.28
img = mix(img, (1.00, 0.90, 0.72), np.clip(glow, 0, 1))


# ---------------------------------------------------------------- hills
def ridge(base, amps, freqs, tilt=0.0):
    r = np.full_like(u, base)
    for a, f in zip(amps, freqs):
        r = r + a * np.sin(2 * np.pi * f * u + rng.uniform(0, 2 * np.pi))
    return (r + tilt * (u - 0.5)).astype(np.float32)


# (baseline, amplitudes, frequencies, tilt, top colour, bottom colour, haze, rows)
layers = [
    (0.470, (0.032, 0.012), (0.9, 2.3), -0.02, (0.64, 0.56, 0.76), (0.80, 0.64, 0.72), 0.55, 0.0),
    (0.550, (0.036, 0.014), (1.2, 2.9), 0.03, (0.52, 0.60, 0.68), (0.72, 0.70, 0.64), 0.42, 0.0),
    (0.640, (0.042, 0.016), (1.1, 2.4), -0.04, (0.50, 0.66, 0.54), (0.70, 0.74, 0.50), 0.30, 0.012),
    (0.740, (0.048, 0.018), (0.8, 2.0), 0.05, (0.84, 0.76, 0.40), (0.60, 0.62, 0.32), 0.18, 0.014),
    (0.850, (0.050, 0.020), (0.7, 1.7), -0.05, (0.40, 0.58, 0.36), (0.20, 0.36, 0.26), 0.08, 0.016),
]

fog_colour = (1.00, 0.88, 0.74)
aa = 1.5 / H

for i, (base, amps, freqs, tilt, top, bottom, haze, rows) in enumerate(layers):
    r = ridge(base, amps, freqs, tilt)[None, :]          # (1,W)
    slope = np.gradient(r[0], u)[None, :]                # dv/du
    depth = v - r                                        # >0 below the ridge

    # Valley mist rising just above this ridge, drawn over what is behind it.
    if i > 0:
        mist = smoothstep(-0.14, 0.0, depth) * (0.30 - 0.04 * i)
        img = mix(img, fog_colour, mist.astype(np.float32))

    t = smoothstep(0.0, 0.22, depth)
    col = mix(np.asarray(top, np.float32)[None, None, :] + 0 * depth[..., None],
              bottom, t.astype(np.float32))

    # Light from the sun side: warm rim on ridges, gentle shading on slopes.
    facing = np.clip(-slope * 1.4, -1.0, 1.0)
    rim = np.exp(-np.maximum(depth, 0) / 0.010) * (0.35 + 0.35 * facing)
    col = col + np.asarray((1.0, 0.86, 0.62), np.float32) * (rim[..., None] * 0.5)
    col = col * (1.0 + 0.10 * facing * np.exp(-np.maximum(depth, 0) / 0.08))[..., None]

    # Faint contour rows on the nearer hills (vineyard-like), fading in with depth.
    if rows:
        stripe = 0.5 + 0.5 * np.cos(2 * np.pi * depth / rows)
        fade = smoothstep(0.02, 0.10, depth)
        col = col * (1.0 - 0.035 * stripe * fade)[..., None]

    # Atmospheric perspective: distant hills drift toward the sky colour.
    col = mix(col, (0.96, 0.80, 0.74), np.full((1, W), haze, np.float32))

    mask = smoothstep(0.0, aa, depth).astype(np.float32)
    img = img + (col - img) * mask[..., None]

# ---------------------------------------------------------------- finish
# Very slight tonal softening plus dithering to prevent banding in smooth gradients.
img = np.clip(img, 0.0, 1.0)
img = img ** (1 / 1.02)
img += rng.normal(0.0, 0.7 / 255.0, size=(H, W, 1)).astype(np.float32)
img += rng.normal(0.0, 0.3 / 255.0, size=(H, W, 3)).astype(np.float32)
img = np.clip(img, 0.0, 1.0)

OUT.parent.mkdir(parents=True, exist_ok=True)
Image.fromarray((img * 255.0 + 0.5).astype(np.uint8), "RGB").save(
    OUT, quality=92, subsampling=0, optimize=True
)
print(f"wrote {OUT} ({OUT.stat().st_size / 1e6:.2f} MB, {W}x{H})")
