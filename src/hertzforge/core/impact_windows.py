"""The windows of an impact test's captured frames: the force window on the
references and the exponential window on every channel.

Both are applied to a frame taken around a trigger, and only there: a
modal test's measurement estimated again (`core.modal_test.reprocess`).
They are not windows for averaged spectra of a continuous record, which
is why `core.averaging.WINDOWS` does not hold them.

**The force window** holds one over the leading part of the frame, the
pre-trigger and the pulse after the trigger, then falls by a steep
half-cosine to zero and stays zero for the rest of the frame
(`force_window`). It keeps the force pulse and takes away what the force
channel holds after it (noise, and the ringing of the hammer's own
structure), which would otherwise be read as force over the whole frame.
It cures nothing about a second hit inside the frame: a double hit is a
second force the structure felt, and cutting it from the force channel
leaves it in every response.

**The exponential window** w(t) = exp(-t / tau), from the frame's first
sample, on the force and every response alike, makes a response that has
not died out by the end of the frame die out inside it (`exponential_window`).
It does that by adding damping: each mode's decay rate becomes its own
plus 1 / tau, so the damping ratio a fit reads is high by about
1 / (tau omega_n) (`core.modal_test.window_damping`).

`tests/data/modal_test_windows.json` holds both windows' samples, so
every implementation of them is checked against the same numbers.

References
----------
1. Fladung, W. A. (1997). "Windows Used for Impact Testing."
   *Proceedings of IMAC XV*. The force window (unity over the leading
   part of the record, a steep cosine taper to zero, zero for the
   remainder), and the exponential window applied to both the force and
   the response.
2. Trethewey, M. W., & Cafeo, J. A. (1992). "Tutorial: Signal
   Processing Aspects of Structural Impact Testing." *International
   Journal of Analytical and Experimental Modal Analysis* 7(2),
   129-149. The exponential window w(t) = exp(-t / tau) (equation 22).
"""

from __future__ import annotations

import math

import numpy as np

#: the force window's half-cosine fall, as a fraction of its width, when
#: no fall is given
FORCE_FALL = 0.1


def nearest_sample(value: float) -> int:
    """`value` samples rounded to the nearest whole sample, a half rounded
    up, never below zero: the one rounding these windows use, the same in
    every language (Python's own `round` rounds a half to even).

    Parameters
    ----------
    value : float
        Samples, finite.

    Returns
    -------
    int
    """
    return max(0, math.floor(float(value) + 0.5))


def force_fall(width: int) -> int:
    """The fall a force window of `width` samples takes when none is
    given: `FORCE_FALL` of its width, at least one sample.

    Parameters
    ----------
    width : int
        The pulse allowance after the trigger, samples.

    Returns
    -------
    int
    """
    return max(1, nearest_sample(FORCE_FALL * int(width)))


def force_width(fraction: float, block: int, pre: int) -> int:
    """The force window's width in samples from the fraction of the block
    after the trigger it is given as: ``nearest_sample(fraction * (block
    - pre))``.

    Parameters
    ----------
    fraction : float
        Above zero.
    block : int
        The frame's samples.
    pre : int
        The trigger's sample in the frame (the pre-trigger's length).

    Returns
    -------
    int
        Samples, at least one.

    Raises
    ------
    ValueError
        For a fraction that is not a finite number above zero, or one that
        rounds to no sample.
    """
    fraction = float(fraction)
    if not math.isfinite(fraction) or fraction <= 0.0:
        raise ValueError(f'the force window\'s width is a fraction of the block after the '
                         f'trigger, above zero; {fraction:g} is not')
    width = nearest_sample(fraction * (int(block) - int(pre)))
    if width < 1:
        raise ValueError(f'a force window of {fraction:g} of the {int(block) - int(pre)} '
                         'samples after the trigger holds no sample of the pulse')
    return width


def force_window(block: int, pre: int, width: int, fall: int | None = None) -> np.ndarray:
    """The force window of a frame triggered at sample `pre`: one over the
    pre-trigger and `width` samples after the trigger, a half-cosine fall
    over `fall` samples, zero after.

    For sample n of a frame of `block` samples, with E = pre + width:

        w[n] = 1                                  for n < E
        w[n] = (1 + cos(pi (n - E) / F)) / 2      for E <= n < E + F
        w[n] = 0                                  for n >= E + F

    so the fall starts at one on sample E and would reach zero on sample
    E + F, the first zero.

    Parameters
    ----------
    block : int
        The frame's samples.
    pre : int
        The trigger's sample in the frame, the pre-trigger's length, zero
        or more.
    width : int
        The pulse allowance after the trigger, samples, above zero.
    fall : int, optional
        The samples of the fall; `force_fall(width)` when omitted.

    Returns
    -------
    numpy.ndarray
        One value per sample.

    Raises
    ------
    ValueError
        For a width that is not a whole number of samples above zero, a
        pre-trigger or fall below zero, or a window that runs past the
        block (it is refused, not cut short).
    """
    for name, value in (('width', width), ('pre-trigger', pre), ('fall', fall)):
        if value is not None and not (isinstance(value, (int, np.integer))
                                      or (isinstance(value, float) and value.is_integer())):
            raise ValueError(f'the force window\'s {name} is a whole number of samples; '
                             f'{value!r} is not')
    block, pre, width = int(block), int(pre), int(width)
    fall = force_fall(width) if fall is None else int(fall)
    if width <= 0:
        raise ValueError(f'the force window\'s width is above zero samples; {width} is not')
    if pre < 0 or fall < 0:
        raise ValueError('the force window\'s pre-trigger and fall are zero or more samples')
    if pre + width + fall > block:
        raise ValueError(f'the force window runs past the block: the trigger at sample {pre}, '
                         f'{width} samples of pulse and a fall of {fall} end at sample '
                         f'{pre + width + fall}, and the block is {block} samples')
    n = np.arange(block, dtype=np.float64)
    end = pre + width
    out = np.zeros(block)
    out[n < end] = 1.0
    if fall > 0:
        falling = (n >= end) & (n < end + fall)
        out[falling] = 0.5 * (1.0 + np.cos(np.pi * (n[falling] - end) / fall))
    return out


def exponential_tau(end: float, length: int) -> float:
    """The time constant, in samples, of an exponential window whose value
    at the last sample of a frame of `length` samples is `end`:
    ``(length - 1) / -ln(end)``, infinite for an end of one (no decay).

    Parameters
    ----------
    end : float
        The window's last value, above 0 and at most 1.
    length : int
        The frame's samples.

    Returns
    -------
    float
        Samples.

    Raises
    ------
    ValueError
        For an end outside (0, 1].
    """
    end = float(end)
    if not 0.0 < end <= 1.0:
        raise ValueError(f'an exponential window ends between 0 and 1; {end:g} does not')
    if end == 1.0 or int(length) < 2:
        return float('inf')
    return (int(length) - 1) / -math.log(end)


def exponential_window(length: int, tau: float) -> np.ndarray:
    """The exponential window ``w[n] = exp(-n / tau)``, n from the frame's
    first sample, `tau` in samples (a time constant tau_s seconds at a
    rate fs is ``tau_s * fs`` samples).

    Parameters
    ----------
    length : int
        The frame's samples, at least one.
    tau : float
        The time constant, samples, above zero; infinite is no decay.

    Returns
    -------
    numpy.ndarray
        One value per sample.

    Raises
    ------
    ValueError
        For a length under one or a time constant not above zero.
    """
    length, tau = int(length), float(tau)
    if length < 1:
        raise ValueError('a window needs at least one sample')
    if not tau > 0.0:
        raise ValueError('an exponential window\'s time constant is above zero')
    return np.exp(-np.arange(length, dtype=np.float64) / tau)
