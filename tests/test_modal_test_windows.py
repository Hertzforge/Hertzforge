"""The impact test's force and exponential windows (`core.averaging`), against the
golden vectors every implementation of them reads: `tests/data/modal_test_windows.json`.

The file is written by this module run as a script (``python
tests/test_modal_test_windows.py``), never by a test run, and is checked here to 1e-12. The
exponential window is also checked against scipy's own exponential window, an implementation
written apart from this one."""

from __future__ import annotations

import json
import math
import os
import sys

import numpy as np
import pytest

from hertzforge.core.averaging import (
    FORCE_FALL,
    Averaging,
    exponential_tau,
    exponential_window,
    force_window,
    nearest_sample,
    window_shape,
)

GOLDEN = os.path.join(os.path.dirname(__file__), 'data', 'modal_test_windows.json')
TOLERANCE = 1e-12

DEFINITIONS = {
    'force': ('w[n] = 1 for n < W; (1 + cos(pi (n - W) / F)) / 2 for W <= n < W + F; 0 for '
              'n >= W + F; n from 0 at the frame\'s first sample, W the width and F the fall '
              'in samples. Named by fractions: W = floor(width * length + 0.5), '
              'F = floor(fall_fraction * W + 0.5), fall_fraction 0.1 unless given.'),
    'exponential': ('w[n] = exp(-n / tau), n from 0 at the frame\'s first sample, tau in '
                    'samples = tau_s * sample_rate_hz. Named by its end value e (the value at '
                    'n = length - 1): tau = (length - 1) / -ln(e); e = 1 is no decay.'),
    'damping': ('A mode windowed by exp(-t / tau_s) has its decay rate raised by 1 / tau_s, '
                'so its damping ratio reads high by about 1 / (2 pi f_n tau_s).'),
}

#: each case: what it is named by, and nothing derived
CASES = [
    {'window': 'force', 'length': 64, 'width_samples': 8, 'fall_samples': 4},
    {'window': 'force', 'length': 64, 'width_samples': 10, 'fall_samples': 0},
    {'window': 'force', 'length': 32, 'width_samples': 28, 'fall_samples': 8},
    {'window': 'force', 'length': 16, 'width_samples': 16, 'fall_samples': 0},
    {'window': 'force', 'length': 256, 'width_fraction': 0.1},
    {'window': 'exponential', 'length': 64, 'sample_rate_hz': 64.0, 'tau_s': 0.25},
    {'window': 'exponential', 'length': 256, 'end_value': 0.01},
    {'window': 'exponential', 'length': 8, 'end_value': 1.0},
]


def samples(case: dict) -> np.ndarray:
    """A case's samples as the analysis core makes them."""
    if case['window'] == 'force':
        if 'width_fraction' in case:
            return window_shape('force', case['length'], case['width_fraction'])
        return force_window(case['length'], case['width_samples'], case['fall_samples'])
    if 'end_value' in case:
        return window_shape('exponential', case['length'], case['end_value'])
    return exponential_window(case['length'], case['tau_s'] * case['sample_rate_hz'])


def make() -> dict:
    """The golden file's contents."""
    return {'about': 'The force and exponential windows of an impact test, sample by sample. '
                     'Every implementation reproduces "samples" to "tolerance".',
            'tolerance': TOLERANCE, 'definitions': DEFINITIONS,
            'cases': [{**case, 'samples': [float(x) for x in samples(case)]}
                      for case in CASES]}


def _golden() -> dict:
    with open(GOLDEN, encoding='utf-8') as f:
        return json.load(f)


def test_modal_test_windows_match_the_golden_vectors():
    golden = _golden()
    assert golden['tolerance'] == TOLERANCE
    assert [{k: v for k, v in c.items() if k != 'samples'} for c in golden['cases']] == CASES
    for case in golden['cases']:
        np.testing.assert_allclose(samples(case), case['samples'], rtol=0, atol=TOLERANCE,
                                   err_msg=str({k: v for k, v in case.items()
                                                if k != 'samples'}))


def test_modal_test_force_window_holds_one_falls_by_half_cosine_then_zero():
    w = force_window(64, 8, 4)
    assert np.all(w[:9] == 1.0)
    # the fall's own samples: cos at a quarter, a half and three quarters of its length
    np.testing.assert_allclose(w[9:12], [0.5 * (1 + math.cos(math.pi * k / 4))
                                         for k in (1, 2, 3)], atol=1e-15)
    assert w[10] == pytest.approx(0.5)
    assert np.all(w[12:] == 0.0)
    assert np.all(np.diff(w) <= 0.0)
    # a cut with no fall, and a window as wide as the frame
    assert list(force_window(6, 2)) == [1, 1, 0, 0, 0, 0]
    assert np.all(force_window(16, 16) == 1.0)


def test_modal_test_force_window_by_fraction_rounds_half_up():
    # 0.1 of 256 is 25.6 samples: 26 at one, and a tenth of 26 (2.6) is a fall of 3
    w = window_shape('force', 256, 0.1)
    assert np.all(w[:27] == 1.0) and 0.0 < w[28] < 1.0 and np.all(w[29:] == 0.0)
    assert FORCE_FALL == 0.1
    # a half rounds up, where Python's round would take it to even
    assert nearest_sample(2.5) == 3 and nearest_sample(3.5) == 4 and round(2.5) == 2
    assert nearest_sample(-1.0) == 0


def test_modal_test_exponential_window_is_scipys_exponential_from_the_first_sample():
    from scipy.signal.windows import exponential

    for length, tau in ((64, 16.0), (1024, 200.5), (7, 0.3)):
        np.testing.assert_allclose(exponential_window(length, tau),
                                   exponential(length, center=0, tau=tau, sym=False),
                                   rtol=1e-14, atol=0)


def test_modal_test_exponential_window_by_end_value_ends_there():
    for length, end in ((256, 0.01), (1024, 0.05), (2, 0.5)):
        w = window_shape('exponential', length, end)
        assert w[0] == 1.0
        assert w[-1] == pytest.approx(end, rel=1e-12)
        assert exponential_tau(end, length) == pytest.approx((length - 1) / -math.log(end))
    assert np.all(window_shape('exponential', 8, 1.0) == 1.0)
    assert math.isinf(exponential_tau(1.0, 8))


@pytest.mark.parametrize('call, words', [
    (lambda: force_window(0, 1), 'at least one sample'),
    (lambda: force_window(8, -1), 'zero or more'),
    (lambda: force_window(8, 2, -1), 'zero or more'),
    (lambda: exponential_window(8, 0.0), 'above zero'),
    (lambda: exponential_window(8, float('nan')), 'above zero'),
    (lambda: exponential_tau(0.0, 8), 'between 0 and 1'),
    (lambda: exponential_tau(1.5, 8), 'between 0 and 1'),
    (lambda: exponential_tau(float('nan'), 8), 'between 0 and 1'),
])
def test_modal_test_windows_refuse_what_names_no_window(call, words):
    with pytest.raises(ValueError, match=words):
        call()


def test_modal_test_windows_are_averaging_windows_with_their_number():
    averaging = Averaging(frame_length=256, window='exponential', window_parameter=0.01)
    assert averaging.window_text() == 'exponential (end 0.01)'
    np.testing.assert_allclose(averaging.shape(), window_shape('exponential', 256, 0.01))
    force = Averaging(frame_length=256, window='force')
    assert force.window_parameter == 0.1
    with pytest.raises(ValueError, match='outside it'):
        Averaging(frame_length=256, window='exponential', window_parameter=0.0)
    with pytest.raises(ValueError, match='outside it'):
        Averaging(frame_length=256, window='force', window_parameter=1.5)


if __name__ == '__main__':  # pragma: no cover - writes the golden file
    path = sys.argv[1] if len(sys.argv) > 1 else GOLDEN
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(make(), f, indent=1)
        f.write('\n')
    print(path)
