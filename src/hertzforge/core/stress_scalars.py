"""Scalars of a stress tensor, and stress through modal superposition.

A stress tensor is held as its six components, xx, yy, zz, xy, yz, xz, the shear components being
the tensor's own (not engineering strains). Its scalars are what a model is colored by: von Mises,
Tresca, the three principal stresses, or one component.

Stress is linear in the modal coordinates, so an element's stress at an instant is the sum over
modes of its stress shape times the mode's coordinate then, and a response never has to be stored
point by point in time: the shapes are as large as the modes. Von Mises and the principal stresses
are not linear, so they are formed from the summed tensor, never summed mode by mode.

These are the reference formulas. The 3-D view forms the same scalars each frame from the summed
tensor (`ui/src/lib/viz/stress.ts`), and both are held to `schema/golden/stress_scalars.json`. The
principal stresses here are the eigenvalues of the symmetric tensor; the shell uses the closed
form through the invariants, so the two are independent checks of each other.
"""

from __future__ import annotations

from typing import Any

import numpy as np

#: the six components, in the order a tensor holds them
COMPONENTS = ('xx', 'yy', 'zz', 'xy', 'yz', 'xz')
#: every scalar a model can be colored by
SCALARS = ('von_mises', 'tresca', 'max_principal', 'mid_principal', 'min_principal', *COMPONENTS)
#: how each scalar is named on a legend
LABELS = {'von_mises': 'Von Mises stress', 'tresca': 'Tresca stress',
          'max_principal': 'Largest principal stress', 'mid_principal': 'Middle principal stress',
          'min_principal': 'Smallest principal stress',
          **{c: f'Stress {c}' for c in COMPONENTS}}
#: how many samples of a clip are summed at once when its peak is found, which bounds the work array
CHUNK = 256


def signed(scalar: str) -> bool:
    """Whether the scalar can be below zero, so its color scale runs from minus its peak."""
    return scalar not in ('von_mises', 'tresca')


def _checked(scalar: str) -> str:
    if scalar not in SCALARS:
        raise ValueError(f'{scalar!r} is not a stress scalar; one of ' + ', '.join(SCALARS))
    return scalar


def _tensor(t: Any) -> np.ndarray:
    t = np.asarray(t, dtype=np.float64)
    if t.shape[-1:] != (6,):
        raise ValueError(f'a stress tensor has six components (xx yy zz xy yz xz), not {t.shape[-1:]}')
    return t


def von_mises(tensor: Any) -> np.ndarray:
    """Von Mises stress of each tensor (..., 6): the square root of three times J2."""
    t = _tensor(tensor)
    xx, yy, zz, xy, yz, xz = np.moveaxis(t, -1, 0)
    j2 = ((xx - yy) ** 2 + (yy - zz) ** 2 + (zz - xx) ** 2) / 6 + xy ** 2 + yz ** 2 + xz ** 2
    return np.sqrt(3 * j2)


def principal_stresses(tensor: Any) -> np.ndarray:
    """The principal stresses of each tensor (..., 6) as (..., 3), largest first."""
    t = _tensor(tensor)
    xx, yy, zz, xy, yz, xz = np.moveaxis(t, -1, 0)
    full = np.stack([np.stack([xx, xy, xz], -1), np.stack([xy, yy, yz], -1),
                     np.stack([xz, yz, zz], -1)], -2)
    return np.linalg.eigvalsh(full)[..., ::-1]


def stress_scalar(tensor: Any, scalar: str) -> np.ndarray:
    """One scalar of each tensor (..., 6): von Mises, Tresca, a principal stress or a component."""
    scalar = _checked(scalar)
    t = _tensor(tensor)
    if scalar == 'von_mises':
        return von_mises(t)
    if scalar in COMPONENTS:
        return t[..., COMPONENTS.index(scalar)]
    p = principal_stresses(t)
    if scalar == 'tresca':
        return p[..., 0] - p[..., 2]
    return p[..., ('max_principal', 'mid_principal', 'min_principal').index(scalar)]


def modal_stress(shapes: Any, coordinates: Any) -> np.ndarray:
    """Each element's stress tensor from its stress shapes and the modal coordinates.

    Args:
        shapes: (modes, elements, 6), each mode's stress per unit modal coordinate.
        coordinates: (modes,) at one instant, or (modes, samples) over a clip.

    Returns:
        (elements, 6) for one instant, or (samples, elements, 6).
    """
    s = _tensor(shapes)
    q = np.asarray(coordinates, dtype=np.float64)
    if s.ndim != 3 or q.shape[0] != s.shape[0]:
        raise ValueError(f'{s.shape[0] if s.ndim == 3 else "no"} stress shapes for '
                         f'{q.shape[0]} modal coordinates')
    if q.ndim == 1:
        return np.einsum('m,mec->ec', q, s)
    return np.einsum('ms,mec->sec', q, s)


def clip_peak(shapes: Any, coordinates: Any, scalar: str) -> float:
    """The largest value of a scalar over every element and every sample of a clip, its magnitude
    for a scalar that can be negative: the top of a color scale every frame of the clip shares.
    The samples are summed a chunk at a time, so a long clip never holds every frame at once.
    NaN samples are passed over; 0.0 when nothing is finite."""
    scalar = _checked(scalar)
    q = np.asarray(coordinates, dtype=np.float64)
    if q.ndim == 1:
        q = q[:, None]
    peak = 0.0
    for start in range(0, q.shape[1], CHUNK):
        values = stress_scalar(modal_stress(shapes, q[:, start:start + CHUNK]), scalar)
        values = np.abs(values) if signed(scalar) else values
        finite = values[np.isfinite(values)]
        if finite.size:
            peak = max(peak, float(finite.max()))
    return peak


def mode_peaks(shapes: Any, scalar: str, shapes_imag: Any = None) -> np.ndarray:
    """Each mode's largest value of a scalar over its elements and its cycle: a mode's own color
    scale. A real mode's stress is its shape times the sine of the phase, so its peak is the
    shape's; a complex mode's tensor turns through its cycle, which is sampled at 64 points."""
    s = _tensor(shapes)
    if shapes_imag is None:
        values = stress_scalar(s, scalar)
        values = np.abs(values) if signed(scalar) else values
        return np.nan_to_num(values, nan=0.0).max(axis=1, initial=0.0)
    im = _tensor(shapes_imag)
    theta = np.linspace(0, 2 * np.pi, 64, endpoint=False)
    turned = (np.sin(theta)[:, None, None, None] * s[None] + np.cos(theta)[:, None, None, None] * im[None])
    values = stress_scalar(turned, scalar)
    values = np.abs(values) if signed(scalar) else values
    return np.nan_to_num(values, nan=0.0).max(axis=(0, 2), initial=0.0)
