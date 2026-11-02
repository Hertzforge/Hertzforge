"""Drawing decisions from Visual Dynamics' `plot.classical_pulse`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from typing import Any


def readout(check: Any, unit_system: Any = None) -> dict[str, str]:
    """The numbers of a check, short enough for the app's panel, in
    display units: 'peak', 'duration' and 'velocity' as the measured
    value and its distance from nominal, 'rise' and 'fall' (where the
    shape has them) against the limit, and 'outside' as a count. A
    value out of its tolerance ends in 'out', so the panel says so in a
    word.
    """
    from ...units import DEFAULT_SYSTEM

    us = DEFAULT_SYSTEM if unit_system is None else unit_system
    measured = check.measured

    def out(ok: bool) -> str:
        return '' if ok else ', out'

    level = (f'{float(us.from_si(measured.peak, "acceleration")):.4g} '
             f'{us.label_text("acceleration")}')
    speed = (f'{float(us.from_si(check.velocity_change, "velocity")):.4g} '
             f'{us.label_text("velocity")}')
    limit = f'{check.edge_allowance * 1e3:.3g}'
    rows = {'peak': f'{level}, {_percent(check.peak_ratio)}'
                    f'{out(check.peak_ok)}',
            'duration': f'{measured.duration * 1e3:.4g} ms, '
                        f'{_percent(check.duration_ratio)}'
                        f'{out(check.duration_ok)}'}
    if measured.rise is not None:
        rows['rise'] = (f'{measured.rise * 1e3:.3g} of {limit} ms'
                        f'{out(check.rise_ok)}')
    if measured.fall is not None:
        rows['fall'] = (f'{measured.fall * 1e3:.3g} of {limit} ms'
                        f'{out(check.fall_ok)}')
    rows['velocity'] = (f'{speed}, {_percent(check.velocity_ratio)}'
                        f'{out(check.velocity_ok)}')
    rows['outside'] = f'{check.outside_count} of {check.points}'
    return {key: value.replace('-', '\u2212')
            for key, value in rows.items()}


def _percent(ratio: float) -> str:
    return f'{100.0 * (ratio - 1.0):+.1f} %'
