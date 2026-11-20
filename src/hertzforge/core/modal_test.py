"""A modal test's measurements on the analysis side: the fixed-reference
form of a roving-excitation test, the damping an exponential window adds,
and the frames of a test estimated again.

**The fixed-reference form.** A modal fit needs every response against
every reference (`core.modal_fit`). A roving hammer test measures the
other way round: one or a few fixed responses, each against a force at
every point the hammer visits. `fixed_reference` turns such a set round
by reciprocity: the record of the motion at j for a force at k becomes
the record of the motion at k for a force at j, the same values, so the
fixed DOF is the reference and the hammered points are the responses.
That is valid when

- every record is a translational motion (acceleration, velocity or
  displacement) per force, so the turned record is the same quantity
  pair: a motion along the force's line per a force along the motion's
  line;
- both DOFs of every record name a direction, since the turned record
  measures along the force's direction;
- the structure is linear and reciprocal: its mass, stiffness and
  damping matrices are symmetric, which a passive structure's are and a
  spinning, actively controlled or aeroelastically loaded one's need not
  be. Then the receptance (K - omega^2 M + i omega C)^-1 is the inverse
  of a symmetric matrix and is symmetric itself, and so are its
  velocity and acceleration forms. Nothing in the records can show
  this; it is the test's assumption, and each turned record says it was
  turned.

Anything else is refused by name (`fixed_reference_refusal`). Each
turned record's comment begins `TRANSPOSED` and names what it was
measured as, so the form travels with the object through a save.

**The exponential window's damping.** A response windowed by
exp(-t / tau) decays faster by 1 / tau: each mode's pole moves left by
exactly 1 / tau, so a fit of the windowed FRF reads a decay rate of
zeta omega_n + 1 / tau and a damping ratio high by 1 / (tau omega_n) to
first order (`window_damping`). The first-order correction holds for
light damping, and is badly off when the damping is very small and
poorly estimated; capturing more of the decay is better than correcting.
`less_window_damping` takes the shift back out exactly, keeping the
damped frequency, and `note_window_damping` writes it beside each mode
of a published fit whose FRF carries the window (`frf_tau`), leaving the
fitted values as the fit made them.

**Estimating again.** `reprocess` estimates a measurement's FRFs and
coherence again from its frames, with the windows, block and frames
chosen, by the analysis core's own estimators (`TimeHistory.compute_frfs`,
`TimeHistory.compute_multiple_coherence`): the windows
(`core.impact_windows`) are applied to each frame first, the force
window to the references only from the frame's trigger, and each frame
is then one average.

References
----------
1. Trethewey, M. W., & Cafeo, J. A. (1992). "Tutorial: Signal
   Processing Aspects of Structural Impact Testing." *International
   Journal of Analytical and Experimental Modal Analysis* 7(2),
   129-149. The exponential window (equation 22) and the correction of
   the damping it adds, zeta_corrected = zeta - 1 / (tau omega_n)
   (equation 27), with its caution for very light damping.
2. Fladung, W. A. (1997). "Windows Used for Impact Testing."
   *Proceedings of IMAC XV*. The force window, and the exponential
   window applied to the force and the response alike.
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from typing import Any

import numpy as np

from ..units import UNKNOWN
from .data import Coherence, Frf, MultipleCoherence, TimeHistory, channel_quantities

#: what begins the comment of every record turned by reciprocity
TRANSPOSED = 'transposed by reciprocity'

#: the motions reciprocity turns, per a force, by their dimension's name
MOTIONS = ('acceleration', 'velocity', 'length')

#: what begins the note a published fit's mode carries for an exponential
#: window's damping
WINDOW_NOTE = 'exponential window'

_DIRECTION = re.compile(r'[XYZ][+-]?$', re.IGNORECASE)


def _quantity_refusal(obj: Any, rows: Sequence[int]) -> str:
    for i in rows:
        dim = obj.known_dim(i) if isinstance(obj, Frf) else obj.ordinate_dim[i]
        if isinstance(obj, Frf):
            if dim == UNKNOWN:
                return (f'the record of {obj.response_dof[i]} against {obj.reference_dof[i]} '
                        'does not say what it measures, and reciprocity holds only for a '
                        'motion per force')
            response, reference = channel_quantities(dim)
            if response not in MOTIONS or reference != 'force':
                words = 'displacement' if response == 'length' else response
                return (f'the record of {obj.response_dof[i]} against {obj.reference_dof[i]} '
                        f'is {words} per {reference}, and reciprocity turns only a '
                        'translational motion per force into the same quantity')
    return ''


def fixed_reference_refusal(obj: Any, records: Sequence[int] | None = None) -> str:
    """Why `fixed_reference` cannot turn these records round, or '' when it
    can.

    Parameters
    ----------
    obj : Frf or Coherence
        The measured set.
    records : sequence of int, optional
        The records to turn; every record when omitted.

    Returns
    -------
    str
        The reason in words, or ''.
    """
    if isinstance(obj, MultipleCoherence):
        return ('a multiple coherence belongs to a response and every reference together, '
                'so it has no reference to turn round; turn the ordinary coherence')
    if not isinstance(obj, (Frf, Coherence)) or obj.reference_dof is None:
        return 'only an FRF or an ordinary coherence has a reference to turn round'
    rows = list(range(obj.num_records)) if records is None else [int(i) for i in records]
    if not rows:
        return 'there are no records to turn round'
    bad = [i for i in rows if not 0 <= i < obj.num_records]
    if bad:
        return f'there is no record {bad[0]}: the set has records 0 to {obj.num_records - 1}'
    for i in rows:
        for dof in (obj.response_dof[i], obj.reference_dof[i]):
            if not _DIRECTION.search(str(dof)):
                return (f'{dof or "a DOF with no node"} names no direction, and a record turned '
                        'by reciprocity measures along the force\'s direction')
    if isinstance(obj, Frf):
        why = _quantity_refusal(obj, rows)
        if why:
            return why
    seen: dict[tuple[str, str], int] = {}
    for i in rows:
        pair = (str(obj.reference_dof[i]), str(obj.response_dof[i]))
        if pair in seen:
            return (f'records {seen[pair]} and {i} would both be {pair[0]} against {pair[1]} '
                    'once turned: a pair measured both ways is a check of reciprocity, not '
                    'two records of one set')
        seen[pair] = i
    if any(obj.comment[i].startswith(TRANSPOSED) for i in rows):
        return 'these records were turned by reciprocity already'
    return ''


def fixed_reference(obj: Any, records: Sequence[int] | None = None) -> Any:
    """A roving-excitation set turned round by reciprocity, so its fixed
    DOFs are the references and every response is against every reference.

    Each record of the motion at j for a force at k becomes the record of
    the motion at k for a force at j, with the same values, quantity and
    units; its comment begins `TRANSPOSED` and names what it was measured
    as. The module's docstring says when this is valid; anything else is
    refused (`fixed_reference_refusal`).

    Parameters
    ----------
    obj : Frf or Coherence
        The measured set, as the test measured it.
    records : sequence of int, optional
        The records to turn; every record when omitted.

    Returns
    -------
    Frf or Coherence
        A new object of the same class; `obj` is not changed.

    Raises
    ------
    ValueError
        With `fixed_reference_refusal`'s reason.
    """
    why = fixed_reference_refusal(obj, records)
    if why:
        raise ValueError(why[0].upper() + why[1:])
    rows = list(range(obj.num_records)) if records is None else [int(i) for i in records]

    def said(i: int) -> str:
        measured = (f'{TRANSPOSED}: measured as {obj.response_dof[i]} against '
                    f'{obj.reference_dof[i]}')
        return f'{measured}; {obj.comment[i]}' if obj.comment[i] else measured

    kwargs: dict[str, Any] = {
        'abscissa': obj.abscissa, 'ordinate': obj.ordinate[rows],
        'response_dof': [obj.reference_dof[i] for i in rows],
        'reference_dof': [obj.response_dof[i] for i in rows],
        'comment': [said(i) for i in rows],
        'block': None if obj.block is None else [obj.block[i] for i in rows]}
    if isinstance(obj, Frf):
        kwargs.update(ordinate_dim=[obj.ordinate_dim[i] for i in rows],
                      ordinate_unit=[obj.ordinate_unit[i] for i in rows],
                      reference_unit=[obj.reference_unit[i] for i in rows],
                      dimension_hint=[obj.dimension_hint[i] for i in rows])
    out = type(obj)(**kwargs)
    out.source = obj.source
    for name in ('averaging', 'statistical_dof'):
        if getattr(obj, name, None) is not None:
            setattr(out, name, getattr(obj, name))
    return out


def is_transposed(obj: Any) -> bool:
    """Whether every record of `obj` was turned by reciprocity
    (`fixed_reference`).

    Parameters
    ----------
    obj : DataArray
        Any data object.

    Returns
    -------
    bool
    """
    comments = list(getattr(obj, 'comment', None) or [])
    return bool(comments) and all(str(c).startswith(TRANSPOSED) for c in comments)


def roving_excitation(frf: Frf) -> bool:
    """Whether a set was measured with the excitation roving: more
    reference DOFs than response DOFs, the shape of a roving hammer test.

    Parameters
    ----------
    frf : Frf
        The measured set.

    Returns
    -------
    bool
    """
    if frf.reference_dof is None:
        return False
    return len(set(frf.reference_dof)) > len(set(frf.response_dof))


# ---- the exponential window's damping -------------------------------------

def window_damping(frequency: Any, tau: float) -> np.ndarray:
    """The damping ratio an exponential window of time constant `tau`
    seconds adds to a mode at `frequency` hertz, to first order:
    1 / (2 pi f_n tau).

    Parameters
    ----------
    frequency : float or array-like
        Natural frequencies, Hz, above zero.
    tau : float
        The window's time constant, s.

    Returns
    -------
    numpy.ndarray
        Fractions of critical.
    """
    return 1.0 / (2.0 * np.pi * np.asarray(frequency, dtype=np.float64) * float(tau))


def less_window_damping(frequency: Any, damping: Any, tau: float
                        ) -> tuple[np.ndarray, np.ndarray]:
    """The natural frequency and damping ratio of a mode before an
    exponential window of time constant `tau` seconds was applied, from
    those fitted to the windowed FRF.

    The window moves each pole left by exactly 1 / tau and leaves its
    damped frequency where it was, so the decay rate is the fitted
    zeta' w_n' less 1 / tau and the natural frequency is that rate and the
    damped frequency together. To first order the damping is the fitted
    less `window_damping`. A mode whose fitted decay is no more than the
    window's own reads NaN: the window accounts for all of it, so its own
    damping is not known.

    Parameters
    ----------
    frequency : float or array-like
        Fitted natural frequencies, Hz.
    damping : float or array-like
        Fitted damping ratios, fractions of critical.
    tau : float
        The window's time constant, s, above zero.

    Returns
    -------
    (numpy.ndarray, numpy.ndarray)
        Natural frequencies (Hz) and damping ratios.

    Raises
    ------
    ValueError
        For a time constant not above zero.
    """
    tau = float(tau)
    if not tau > 0.0:
        raise ValueError('an exponential window\'s time constant is above zero')
    f = np.asarray(frequency, dtype=np.float64)
    zeta = np.asarray(damping, dtype=np.float64)
    omega = 2.0 * np.pi * f
    decay = zeta * omega - 1.0 / tau
    damped = omega * np.sqrt(np.clip(1.0 - zeta ** 2, 0.0, None))
    natural = np.hypot(decay, damped)
    with np.errstate(divide='ignore', invalid='ignore'):
        ratio = np.where(decay > 0.0, decay / natural, np.nan)
    return natural / (2.0 * np.pi), ratio


def tau_words(tau: float) -> str:
    """How a record's comment says the exponential window it was estimated
    with, which `frf_tau` reads back: ``'exponential window on every
    channel, tau 0.5 s'``.

    Parameters
    ----------
    tau : float
        The time constant, s.

    Returns
    -------
    str
    """
    return f'{TAU_WORDS} {float(tau):.12g} s'


#: what begins `tau_words`
TAU_WORDS = 'exponential window on every channel, tau'

_TAU = re.compile(re.escape(TAU_WORDS) + r' ([0-9.eE+-]+) s')


def frf_tau(frf: Any) -> float | None:
    """The exponential window's time constant, in seconds, an FRF was
    estimated with, or None when it was not: from its records' comments
    (`tau_words`), which survive a save, when every record names the same
    time constant.

    Parameters
    ----------
    frf : DataArray
        An FRF, or any object with comments.

    Returns
    -------
    float or None
    """
    comments = list(getattr(frf, 'comment', None) or [])
    found = {float(m.group(1)) if (m := _TAU.search(str(c))) else None for c in comments}
    if len(found) != 1:
        return None
    tau, = found
    return tau if tau is not None and np.isfinite(tau) and tau > 0.0 else None


def window_note(frequency: float, damping: float, tau: float) -> str:
    """The note a fitted mode carries for an exponential window: its time
    constant, the damping it added, and the mode less it.

    Parameters
    ----------
    frequency : float
        The fitted natural frequency, Hz.
    damping : float
        The fitted damping ratio.
    tau : float
        The window's time constant, s.

    Returns
    -------
    str
    """
    f, zeta = (float(v) for v in less_window_damping(frequency, damping, tau))
    added = float(window_damping(frequency, tau))
    head = (f'{WINDOW_NOTE} tau {tau:.6g} s adds about {added * 100:.4g} % to the fitted '
            'damping')
    if not np.isfinite(zeta):
        return f'{head}; it accounts for all of it, so the damping less it is not known'
    return f'{head}; less it: {zeta * 100:.6g} % at {f:.8g} Hz'


_NOTE_VALUES = re.compile(r'less it: ([0-9.eE+-]+) % at ([0-9.eE+-]+) Hz')


def window_corrected(shapes: Any) -> list[tuple[float, float] | None]:
    """Each mode's (frequency in Hz, damping ratio) less the exponential
    window, read from the note `note_window_damping` wrote, or None for a
    mode with no such note.

    Parameters
    ----------
    shapes : ShapeSet
        A published fit.

    Returns
    -------
    list of (float, float) or None
    """
    out: list[tuple[float, float] | None] = []
    for comment in shapes.comment:
        found = _NOTE_VALUES.search(str(comment)) if WINDOW_NOTE in str(comment) else None
        out.append(None if found is None
                   else (float(found.group(2)), float(found.group(1)) / 100.0))
    return out


def note_window_damping(shapes: Any, frf: Any) -> Any:
    """Note on each mode of a published fit the damping the FRF's
    exponential window added, and the mode less it (`window_note`), in
    the mode's comment; the fitted frequency and damping are left as the
    fit made them, so the set still synthesizes the FRF it was fitted to
    and Edit Fit restores the same fit. A comment's other text is kept;
    a note from an earlier publish is replaced. A set fitted to an FRF
    with no exponential window is returned unchanged.

    Parameters
    ----------
    shapes : ShapeSet
        The fit's shape set, changed in place.
    frf : Frf
        The FRF it was fitted to.

    Returns
    -------
    ShapeSet
        `shapes`.
    """
    tau = frf_tau(frf)
    if tau is None:
        return shapes
    for m in range(shapes.num_shapes):
        kept = [part for part in str(shapes.comment[m]).split(' | ')
                if part and not part.startswith(WINDOW_NOTE)]
        note = window_note(float(shapes.frequency[m]), float(shapes.damping[m]), tau)
        shapes.comment[m] = ' | '.join([*kept, note])
    return shapes


# ---- estimating again ------------------------------------------------------

def reprocess(history: TimeHistory, *, method: str = 'H1',
              force: float | None = None, exponential: float | None = None,
              block: int | None = None, captures: Sequence[int] | None = None,
              shaker_window: str | None = None) -> tuple[Frf, Any]:
    """A measurement's FRFs and coherence estimated again from its frames.

    `history` holds the measurement's frames, a record per channel per
    frame (a converted Modal Test run's measurement frames), timed from
    the trigger: the samples at negative times are the pre-trigger. Each
    frame is cut to its first `block` samples, windowed (the force window
    on the references from the trigger, the exponential window on every
    channel, or the shaker window on every channel; `core.impact_windows`),
    and is then one average of the core's own estimate
    (`TimeHistory.compute_frfs` and `TimeHistory.compute_multiple_coherence`,
    on the channels' roles). With one reference the coherence is the
    ordinary coherence (the multiple coherence of one reference is that)
    as a `Coherence` of each response against it; with several, a
    `MultipleCoherence`.

    Parameters
    ----------
    history : TimeHistory
        The frames.
    method : str, default 'H1'
        'H1', 'H2' or 'Hv'.
    force : float, optional
        The force window's width as a fraction of the block after the
        trigger, on the references (`impact_windows.force_width`); none
        when omitted.
    exponential : float, optional
        The exponential window's end value at the block's last sample,
        on every channel; none when omitted.
    block : int, optional
        Samples of each frame to use, from its first; the whole frame
        when omitted.
    captures : sequence of int, optional
        Which frames, by capture number from 0 (`capture_indices`); every
        frame when omitted.
    shaker_window : str, optional
        A window for every channel instead of the transient ones ('hann'
        for a random source); refused beside `force` or `exponential`.

    Returns
    -------
    (Frf, Coherence or MultipleCoherence)

    Raises
    ------
    ValueError
        For a block longer than the frames or under two samples, or that
        ends before the trigger; no frames picked, a capture the history
        does not hold; a shaker window beside a transient one; a force
        window that is not one (`impact_windows.force_window`); a
        reference that holds one value through a frame once windowed (a
        dead channel, or a window that removed the pulse, named so); or
        anything the estimators refuse (too few frames for the
        references, a frame that is not finite).
    """
    from .averaging import Averaging, window_shape
    from .impact_windows import (
        exponential_tau,
        exponential_window,
        force_fall,
        force_width,
        force_window,
    )

    length = len(history.abscissa)
    block = length if block is None else int(block)
    if not 2 <= block <= length:
        raise ValueError(f'the block is from 2 samples to the frames\' {length}; '
                         f'{block} is not')
    if shaker_window is not None and (force is not None or exponential is not None):
        raise ValueError('a shaker window is the whole frame\'s; it is not used with the '
                         'force or exponential window')
    pre = int(np.count_nonzero(np.asarray(history.abscissa[:block]) < 0.0))
    if force is not None and pre >= block:
        raise ValueError(f'the block of {block} samples ends before the trigger at sample '
                         f'{pre}, so there is no pulse to hold')
    held = history.capture_indices()
    count = max(held) + 1 if held else 0
    chosen = sorted(set(range(count) if captures is None else (int(c) for c in captures)))
    if not chosen:
        raise ValueError('no frames are picked, so there is nothing to estimate from')
    absent = [c for c in chosen if not 0 <= c < count]
    if absent:
        raise ValueError(f'the frames are captures 0 to {count - 1}; there is no capture '
                         f'{absent[0]}')
    roles = history.channel_roles()
    keep = [i for i, c in enumerate(held) if c in chosen]
    rows = np.array(history.ordinate[keep, :block], dtype=np.float64)
    every = np.ones(block)
    tau = None
    if exponential is not None:
        tau = exponential_tau(exponential, block)
        every = every * exponential_window(block, tau)
    if shaker_window is not None:
        every = every * window_shape(shaker_window, block)
    width = force_width(force, block, pre) if force is not None else None
    on_force = force_window(block, pre, width) if width is not None else np.ones(block)
    for k, i in enumerate(keep):
        identity = (history.response_dof[i], history.ordinate_dim[i])
        reference = roles.get(identity) == 'reference'
        before = rows[k].copy()
        rows[k] *= every * (on_force if reference else 1.0)
        if reference and np.isfinite(rows[k]).all() and not np.any(rows[k] != rows[k, 0]):
            frame = history.block[i] if history.block is not None else f'capture {held[i] + 1}'
            if np.any(before != before[0]):
                raise ValueError(
                    f'the reference {identity[0]} holds one value through {frame} once '
                    f'windowed: the force window holds one to sample {pre + width} and the '
                    'force there is nothing; widen it or check the trigger')
            raise ValueError(f'the reference {identity[0]} holds one value through {frame}: '
                             'a dead reference gives no FRF; leave that frame out')
    frames = TimeHistory(history.abscissa[:block], rows,
                         response_dof=[history.response_dof[i] for i in keep],
                         ordinate_dim=[history.ordinate_dim[i] for i in keep],
                         ordinate_unit=[history.ordinate_unit[i] for i in keep],
                         block=([history.block[i] for i in keep]
                                if history.block is not None else None))
    frames.roles = dict(history.roles or {})
    each = Averaging.for_records(block)
    frf = frames.compute_frfs(averaging=each, method=method)
    multiple = frames.compute_multiple_coherence(averaging=each)
    references = sorted({frf.reference_dof[i] for i in range(frf.num_records)},
                        key=frf.reference_dof.index)
    if len(references) == 1:
        coherence: Any = Coherence(multiple.abscissa, multiple.ordinate,
                                   response_dof=multiple.response_dof,
                                   reference_dof=references * multiple.num_records,
                                   comment=f'ordinary coherence, {len(chosen)} averages')
    else:
        coherence = multiple
    said = [f'{len(chosen)} frames' + (f' (captures {", ".join(str(c + 1) for c in chosen)})'
                                         if captures is not None else '')]
    if width is not None:
        said.append(f'force window on the references, {width} samples after the trigger '
                    f'at sample {pre} and a fall of {force_fall(width)}')
    if tau is not None:
        rate = 1.0 / float(np.median(np.diff(history.abscissa[:block])))
        said.append(f'{tau_words(tau / rate)}, ending at {exponential:g}')
    if shaker_window is not None:
        said.append(f'{shaker_window} window on every channel')
    frf.comment = [f'{c}; {"; ".join(said)}' for c in frf.comment]
    frf.averaging = Averaging(frame_length=block, frames=len(chosen),
                              window=shaker_window or 'boxcar')
    for made in (frf, coherence):
        made.source = history.source
    return frf, coherence
