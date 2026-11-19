"""The filter view: the toggle, the panel, the preview, the verbs.

The GUI half of the motion chain (Brandon, 2026-08-24): a taskbar
toggle like the averaging's, a parameter panel beside the plot, and
the filtered trace previewed in green over the raw one — with the
calculator entries that actually make the derived records.
"""

from __future__ import annotations

import numpy as np
import pytest


def test_the_response_is_what_the_data_sees_not_the_filters_own():
    """`filtered` runs the filter forward and backward, so the record
    is multiplied by |H| twice. Plotting |H| would draw a filter
    nothing here applies: −3 dB at the corner where the real pair is
    at −6.02 (Brandon, 2026-08-25)."""
    from scipy.signal import butter, sosfreqz

    from hertzforge.core.filters import Filtering, response

    rate, corner, order = 8192.0, 800.0, 4
    frequencies, magnitude = response(Filtering(high=corner,
                                                order=order), rate)
    assert float(np.interp(corner, frequencies, magnitude)) == \
        pytest.approx(-6.02, abs=0.05)

    sos = butter(order, corner, 'low', fs=rate, output='sos')
    _w, h = sosfreqz(sos, worN=frequencies, fs=rate)
    single = 20 * np.log10(np.abs(h))
    assert float(np.interp(corner, frequencies, single)) == \
        pytest.approx(-3.01, abs=0.05), 'the one-pass filter, for contrast'
    # above the floor the curve is exactly twice the one-pass dB;
    # below it `response` clamps, on purpose
    alive = single > -100.0
    assert np.allclose(magnitude[alive], 2 * single[alive], atol=1e-6), \
        'the drawn curve is exactly the squared magnitude'
