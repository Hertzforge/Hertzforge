"""Every banded spectrum on the stage: channels receding, bands along.

The sine stage's reading, generalized (Brandon, 2026-08-22): the
depth axis is the channel, a specification alone shows every
channel's requirement with its zones at once, and a comparison shows
every channel's measurement with the lines outside an abort limit
boxed over their own bins — the 2-D shading, in depth.
"""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Psd, Specification

FREQ = np.linspace(20.0, 2000.0, 100)


def _specification(channels=('101Z+', '104Z+', '110Z+')):
    level = np.full((len(channels), len(FREQ)), 1e-2)
    return Specification(
        abscissa=FREQ, ordinate=level,
        response_dof=list(channels),
        ordinate_dim=['acceleration**2/frequency'] * len(channels),
        warning_lower=level * 10 ** (-3 / 10),
        warning_upper=level * 10 ** (3 / 10),
        abort_lower=level * 10 ** (-6 / 10),
        abort_upper=level * 10 ** (6 / 10))


def _measured(loud_lines=0, channels=('101Z+', '104Z+', '110Z+')):
    """Flat at the target, except `loud_lines` lines on the first
    channel pushed past the upper abort."""
    values = np.full((len(channels), len(FREQ)), 1e-2)
    if loud_lines:
        values[0, 40:40 + loud_lines] = 1e-2 * 10 ** (9 / 10)
    return Psd(abscissa=FREQ, ordinate=values,
               response_dof=list(channels),
               ordinate_dim=['acceleration**2/frequency'] * len(channels))


def _cross_term_target(coherence=0.5):
    """Three channels with every pair stated at `coherence`, in phase."""
    channels = ['M1', 'M2', 'M3']
    autos = np.array([1e-2, 2e-2, 3e-2])
    rows, resp, refs, limits = [], [], [], {n: [] for n in (
        'warning_lower', 'warning_upper', 'abort_lower', 'abort_upper')}
    for a, da in enumerate(channels):
        for b, db in enumerate(channels):
            level = autos[a] if a == b else np.sqrt(coherence * autos[a] * autos[b])
            rows.append(np.full(len(FREQ), level, dtype=complex))
            resp.append(da)
            refs.append(db)
            for name, db_ in (('warning_lower', -3), ('warning_upper', 3),
                              ('abort_lower', -6), ('abort_upper', 6)):
                limits[name].append(np.full(len(FREQ), autos[a] * 10 ** (db_ / 10))
                                    if a == b else np.full(len(FREQ), np.nan))
    return Specification(
        abscissa=FREQ, ordinate=np.asarray(rows), response_dof=resp,
        reference_dof=refs, ordinate_dim=['acceleration**2/frequency'] * 9,
        **{name: np.asarray(v) for name, v in limits.items()}), autos


def _cross_spectral(channels=('101Z+', '104Z+', '110Z+'), lines=None, cross=1e2):
    """A measured matrix: each channel's autospectrum flat at the
    target, and every cross term far above the abort limit."""
    freq = FREQ if lines is None else np.linspace(20.0, 2000.0, lines)
    resp = [c for c in channels for _ in channels]
    ref = [r for _ in channels for r in channels]
    values = np.array([np.full(len(freq), 1e-2 if a == b else cross)
                       for a, b in zip(resp, ref)])
    return Psd(abscissa=freq, ordinate=values, response_dof=resp,
               reference_dof=ref,
               ordinate_dim=['acceleration**2/frequency'] * len(resp))
