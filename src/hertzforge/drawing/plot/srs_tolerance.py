"""Drawing decisions from Visual Dynamics' `plot.srs_tolerance`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from typing import Any

import numpy as np

from ...core.compliance import srs_band_text
from . import _signed_db


def _db(value: Any) -> str:
    """One band edge in words, '−1.5 dB'; an edge read from a limit
    curve that is not a fixed number of dB from the requirement is
    'the limit'."""
    if np.ndim(value):
        return 'the limit'
    return _signed_db(value)


def band_text(inner: tuple[Any, Any],
              outer: tuple[Any, Any] | None) -> str:
    """The band in words, as the title and the legend give it:
    '−1.5/+3 dB', or '−1.5/+3 dB within −3/+6 dB' with an outer band
    (`compliance.srs_band_text`, which the report captions share)."""
    return srs_band_text(inner, outer)


def describe(result: dict[str, Any]) -> str:
    """One judged spectrum in a line: its polarity, the count and share
    of its points inside the band, and whether it passed, as in
    'maximax: 85/92 inside (92.4 %), passes'. Short, because it is a
    legend entry beside two or three others."""
    name = result.get('kind') or 'measured'
    # the counts are the judgement's own under either rule: 'above_lower'
    # has no ceiling, so inside is at or above the lower edge
    where = ('inside' if result['rule'] == 'share'
             else f'at or above {_db(result["inner"][0])}')
    verdict = {True: 'passes', False: 'fails', None: 'not judged'}[
        result['passed']]
    return (f'{name}: {result["inside_count"]}/{result["points"]} {where} '
            f'({result["inside_percent"]:.1f} %), {verdict}')
