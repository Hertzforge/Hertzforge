"""The curve palette takes a shade for its background (2026-09-27).

On white the dark-background palette stood at 1.8:1 (yellow) to 3.7:1,
and a one-pixel line that faint was hard to see. Wider lines would have
helped and cost ten times the repaint (PLAN.md, "Curves against a
specification"); a darker shade of the same hue costs nothing.
"""

from __future__ import annotations

import colorsys


def _luminance(color):
    channels = [int(color.lstrip('#')[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    linear = [c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
              for c in channels]
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]


def _contrast(a, b):
    high, low = sorted((_luminance(a), _luminance(b)), reverse=True)
    return (high + 0.05) / (low + 0.05)


def _hue(color):
    rgb = [int(color.lstrip('#')[i:i + 2], 16) / 255 for i in (0, 2, 4)]
    return colorsys.rgb_to_hls(*rgb)[0] * 360
