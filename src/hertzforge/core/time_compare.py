"""Two sets of time responses compared station by station: a test's
measured responses against a model's predicted ones.

Each response record of one set is paired with the record of the other
at the same signed DOF and the same quantity (`TimeHistory.channel_key`).
Force and other reference channels, and monitors, are left out: a
channel is paired only when its role is 'response'
(`TimeHistory.channel_roles`). A DOF with no partner, or with more than
two records, is refused with the DOF named rather than paired by
guessing; two sets whose node numbers differ are paired by hand.

The pairs are put on one clock, the coarsest of the sources' clocks,
inside the time all of them cover, and the other traces are
interpolated linearly onto it. Nothing else is done to them: no lag is
removed, no sign or gain is fitted, and no filter is applied. A source
sampled much faster than the shared clock is interpolated without an
anti-alias filter, so content above the coarser clock's Nyquist
frequency must be filtered out first.

For a pair, the first trace a (the test) and the second b (the model),
on the shared clock inside the scoring window:

- TRAC, the time response assurance criterion, (a·b)² / ((a·a)(b·b)),
  from 0 to 1, and 1 when the two have the same shape. Squared and
  normalized, it is blind to a reversed sign and to any constant gain.
- the signed correlation, a·b / √((a·a)(b·b)), negative for a reversal;
- the RMS ratio, ‖b‖ / ‖a‖, the difference in amplitude;
- sTRAC, (a·b) / (b·b), the least-squares factor on b that fits a.
  Its ideal value is 1; -2 says the model is half the test's size and
  the wrong way round.

A trace that is zero, constant or not finite inside the window has no
score, and its row says why.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np

from .data import UNKNOWN, TimeComparison, TimeHistory


def response_records(data: TimeHistory) -> list[int]:
    """The records whose channel's role is 'response', in order: a
    force, a reference or a monitor is not compared."""
    roles = data.channel_roles()
    return [i for i in range(data.num_records)
            if roles.get((data.response_dof[i], data.ordinate_dim[i])) == 'response']


def _label(key: tuple[str, str, str | None]) -> str:
    dof, dimension, hint = key
    quantity = hint if dimension == UNKNOWN and hint else dimension
    return f'{dof or "(no DOF)"} {quantity.replace("_", " ")}'


def match_records(histories: Mapping[str, TimeHistory],
                  pairs: Sequence[Sequence[Any]] | None = None,
                  records: Mapping[str, Sequence[int]] | None = None
                  ) -> list[list[Any]]:
    """The pairs to compare, as [first name, record, second name, record].

    Parameters
    ----------
    histories : mapping of str to TimeHistory
        The sets compared, by name, the first set first: its records are
        the first of each pair (the test, by convention).
    pairs : sequence of [str, int, str, int], optional
        Pairs made by hand, which may join different DOFs but never
        different quantities. Without them the records are paired by
        `TimeHistory.channel_key`.
    records : mapping of str to sequence of int, optional
        The records of each history to pair automatically, by name, such
        as the ones picked; all of its records when not given. Only the
        response records among them (`response_records`) are paired. A
        history not named here, beside one that is, gives only the
        partners of the records named.

    Returns
    -------
    list of [str, int, str, int]
        One entry a pair, in the first set's record order.

    Raises
    ------
    TypeError
        If a source is not a time history.
    ValueError
        If a DOF has no partner or more than two records; if a pair
        names a record that does not exist, uses a record twice, joins
        two quantities, or holds a record whose units are not declared.
    """
    names = list(histories)
    if not names:
        raise ValueError('pick the time histories to compare')
    for name in names:
        if not isinstance(histories[name], TimeHistory):
            raise TypeError(f'{name} is not a time history')
    if pairs is None:
        chosen: dict[str, list[int]] = {}
        for name in names:
            data = histories[name]
            responses = response_records(data)
            picked = (responses if records is None or name not in records
                      else [int(i) for i in records[name]])
            for i in picked:
                if not 0 <= i < data.num_records:
                    raise ValueError(f'{name} has no record {i + 1}')
            chosen[name] = [i for i in picked if i in responses]
        # records picked under some sets and not the others: the others give
        # only the partners of what was picked
        wanted = ({histories[name].channel_key(i) for name in records if name in chosen
                   for i in chosen[name]} if records else None)
        grouped: dict[tuple[str, str, str | None], list[tuple[str, int]]] = {}
        for name in names:
            data = histories[name]
            for i in chosen[name]:
                key = data.channel_key(i)
                if wanted is None or name in records or key in wanted:
                    grouped.setdefault(key, []).append((name, i))
        if not grouped:
            raise ValueError('no response records to compare: forces, references and '
                             'monitors are left out')
        pairs = []
        for key, members in grouped.items():
            if len(members) != 2:
                where = ', '.join(sorted({name for name, _i in members}))
                raise ValueError(
                    f'{_label(key)}: {len(members)} record{"s" * (len(members) != 1)} '
                    f'({where}), and a comparison needs two; pick both sets, or pair '
                    f'the records by hand')
            pairs.append([*members[0], *members[1]])
    if not pairs:
        raise ValueError('no pairs to compare')
    resolved: list[list[Any]] = []
    used: set[tuple[str, int]] = set()
    for pair in pairs:
        first, i, second, j = pair
        i, j = int(i), int(j)
        for name, k in ((first, i), (second, j)):
            if name not in histories:
                raise ValueError(f'a pair names {name}, which is not among the sets compared')
            if not 0 <= k < histories[name].num_records:
                raise ValueError(f'{name} has no record {k + 1}')
            if (name, k) in used:
                raise ValueError(f'{name} record {k + 1} is in two pairs')
            used.add((name, k))
        a, b = histories[first], histories[second]
        for name, data, k in ((first, a, i), (second, b, j)):
            if data.ordinate_dim[k] == UNKNOWN:
                raise ValueError(f'{name} record {k + 1} ({data.response_dof[k]}): '
                                 f'declare its units before comparing')
        if a.ordinate_dim[i] != b.ordinate_dim[j]:
            raise ValueError(
                f'{first} record {i + 1} is {a.ordinate_dim[i].replace("_", " ")} and '
                f'{second} record {j + 1} is {b.ordinate_dim[j].replace("_", " ")}: '
                f'a pair is one quantity')
        resolved.append([str(first), i, str(second), j])
    return resolved


def compare(histories: Mapping[str, TimeHistory], *,
            pairs: Sequence[Sequence[Any]] | None = None,
            records: Mapping[str, Sequence[int]] | None = None,
            start: float | None = None, stop: float | None = None) -> TimeComparison:
    """The pairs on one clock, ready to be scored.

    The clock is the coarsest of the clocks of the records paired (by
    median step), kept where it lies inside the time every one of them
    covers; every other trace is interpolated linearly onto it, never
    extrapolated. The values are the sources' own, in SI.

    Parameters
    ----------
    histories, pairs, records
        As `match_records` takes them.
    start, stop : float, optional
        The scoring window, in seconds; the whole shared clock when not
        given.

    Returns
    -------
    TimeComparison
        Two records a pair, the first set's first, on the shared clock.

    Raises
    ------
    ValueError
        As `match_records` raises; and if a record's clock is not
        finite and increasing, the records share no stretch of time,
        or the window does not fit (`scores`).
    """
    pairs = match_records(histories, pairs, records)
    traces = [(name, histories[name], k) for first, i, second, j in pairs
              for name, k in ((first, i), (second, j))]
    for name, data, _k in {id(d): (n, d, k) for n, d, k in traces}.values():
        x = data.abscissa
        if len(x) < 2 or not np.isfinite(x).all() or not (np.diff(x) > 0).all():
            raise ValueError(f'{name}: its time samples are not finite and increasing')
    low = max(float(data.abscissa[0]) for _n, data, _k in traces)
    high = min(float(data.abscissa[-1]) for _n, data, _k in traces)
    coarsest = max((data for _n, data, _k in traces),
                   key=lambda d: float(np.median(np.diff(d.abscissa))))
    clock = coarsest.abscissa[(coarsest.abscissa >= low) & (coarsest.abscissa <= high)]
    if len(clock) < 2:
        raise ValueError('the time histories compared share no stretch of time')
    rows = [np.interp(clock, data.abscissa, data.ordinate[k]) for _n, data, k in traces]
    out = TimeComparison(
        clock.copy(), np.asarray(rows),
        response_dof=[data.response_dof[k] for _n, data, k in traces],
        ordinate_dim=[data.ordinate_dim[k] for _n, data, k in traces],
        ordinate_unit=[data.ordinate_unit[k] for _n, data, k in traces],
        block=[name for name, _d, _k in traces],
        comment=[data.comment[k] for _n, data, k in traces],
        pairs=pairs, sources=list(histories),
        start=float(clock[0]) if start is None else float(start),
        stop=float(clock[-1]) if stop is None else float(stop))
    sources = {data.source for _n, data, _k in traces} - {None}
    if 'SIM' in sources:
        out.source = 'SIM'
    elif sources:
        out.source = sources.pop() if len(sources) == 1 else None
    scores(out)  # refuses a window that does not fit before anything keeps it
    return out


def window(data: TimeComparison, start: float | None = None,
           stop: float | None = None) -> np.ndarray:
    """Which samples of the shared clock lie in the scoring window, as a
    mask: the comparison's own window, or `start` and `stop`."""
    start = data.start if start is None else float(start)
    stop = data.stop if stop is None else float(stop)
    if not np.isfinite([start, stop]).all() or stop <= start:
        raise ValueError('the scoring window ends after it starts')
    clock = data.abscissa
    if start < clock[0] or stop > clock[-1]:
        raise ValueError(f'the scoring window, {start:g} s to {stop:g} s, must lie inside '
                         f'the shared clock, {clock[0]:g} s to {clock[-1]:g} s')
    keep = (clock >= start) & (clock <= stop)
    if keep.sum() < 2:
        raise ValueError(f'the scoring window, {start:g} s to {stop:g} s, holds fewer than '
                         f'two samples')
    return keep


def scores(data: TimeComparison, start: float | None = None,
           stop: float | None = None) -> list[dict[str, Any]]:
    """Every pair's scores over the scoring window.

    Parameters
    ----------
    data : TimeComparison
        The comparison.
    start, stop : float, optional
        A window in place of the comparison's own, in seconds.

    Returns
    -------
    list of dict
        One a pair, in order: `pair`, `dof` (the first trace's),
        `quantity`, `trac`, `correlation`, `rms_ratio` and `strac`, each
        None with `reason` saying why when a trace is zero, constant or
        not finite in the window, and `samples`, how many the window
        holds.

    Raises
    ------
    ValueError
        If the window ends before it starts, reaches outside the shared
        clock, or holds fewer than two samples.
    """
    keep = window(data, start, stop)
    rows = []
    for k, pair in enumerate(data.pairs):
        a, b = data.ordinate[2 * k, keep], data.ordinate[2 * k + 1, keep]
        reason = ''
        if not (np.isfinite(a).all() and np.isfinite(b).all()):
            reason = 'a trace is not finite in the window'
        elif not (a.any() and b.any()):
            reason = 'a trace is zero in the window'
        elif np.ptp(a) == 0 or np.ptp(b) == 0:
            reason = 'a trace is constant in the window'
        trac = correlation = ratio = strac = None
        if not reason:
            # scaled to their peaks first, so the squares neither overflow
            # nor underflow whatever the units' size
            sa, sb = float(np.abs(a).max()), float(np.abs(b).max())
            u, v = a / sa, b / sb
            uu, vv, uv = float(u @ u), float(v @ v), float(u @ v)
            correlation = float(np.clip(uv / np.sqrt(uu * vv), -1.0, 1.0))
            trac = correlation ** 2
            ratio = float(sb / sa * np.sqrt(vv / uu))
            strac = float(sa / sb * uv / vv)
        rows.append({'pair': list(pair), 'dof': data.response_dof[2 * k],
                     'quantity': data.ordinate_dim[2 * k],
                     'trac': trac, 'correlation': correlation, 'rms_ratio': ratio,
                     'strac': strac, 'samples': int(keep.sum()), 'reason': reason})
    return rows
