"""Drawing decisions from Visual Dynamics' `plot.replication_segments`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from typing import Any

import numpy as np

#: the table's columns, in order
COLUMNS = ('Segment', 'Kind', 'Span [s]', 'Share', 'RMS error', 'Check',
           'Reading', 'Tolerance', 'Verdict')


def _verdict(passed: bool | None) -> str:
    return {True: 'passes', False: 'fails', None: 'not judged'}[passed]


def _number(value: float, places: int = 2) -> str:
    """A number for the table, with the minus sign; a dash for NaN."""
    if not np.isfinite(value):
        return '\u2014'
    return f'{value:.{places}f}'.replace('-', '\u2212')


def _reading(check: Any) -> str:
    """A check's reading as the table gives it: its range, or its one
    value, with its unit."""
    from ...core.replication_segments import _signed

    if not check.judged:
        return 'nothing to judge'
    if check.judged == 1:
        return _signed(check.low, check.unit)
    text = (f'{_signed(check.low, check.unit)} to '
            f'{_signed(check.high, check.unit)}')
    return text + f'; {check.outside} of {check.judged} outside'


def verdict_rows(result: Any) -> list[tuple[str, ...]]:
    """The table's rows, one per check, the whole record first, each a
    tuple of `COLUMNS` strings. A segment's name, kind, span, share and
    error are given on its first row only."""
    from ...core.replication_segments import band_text

    rows = [('whole record', '\u2014',
             f'0 to {result.samples / result.sample_rate:g}',
             '100.00 %', f'{_number(result.rms_error)} %', 'rms error',
             f'{_number(result.rms_error)} %',
             'none: the controller\'s figure', '\u2014')]
    for segment in result.segments:
        seg = segment.segment
        for k, check in enumerate(segment.checks):
            head = ((segment.label, seg.kind,
                     f'{seg.start:g} to {seg.end:g}',
                     f'{_number(segment.share)} %',
                     f'{_number(segment.rms_error)} %') if k == 0
                    else ('', '', '', '', ''))
            rows.append((*head, check.reading, _reading(check),
                         band_text(check.tolerance, check.unit),
                         _verdict(check.passed)))
    return rows


def describe_record(result: Any) -> str:
    """The plot's title: the channel, how many segments fail, and the
    whole-record figure beside them."""
    where = f'{result.channel}: ' if result.channel else ''
    return where + result.describe()
