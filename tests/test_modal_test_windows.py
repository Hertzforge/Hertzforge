"""The impact test's force and exponential windows (`core.impact_windows`), against the
golden vectors every implementation of them reads: `tests/data/modal_test_windows.json`.

The file is written by this module run as a script (``python
tests/test_modal_test_windows.py``), never by a test run, and is checked here to 1e-12. Each
case gives every number its samples follow from, in samples or in seconds and hertz, and
nothing derived. The exponential window is also checked against scipy's own exponential
window, an implementation written apart from this one."""

from __future__ import annotations

import json
import math
import os
import sys

import numpy as np
import pytest

from hertzforge.core.averaging import WINDOWS, Averaging, normalize_window
from hertzforge.core.impact_windows import (
    FORCE_FALL,
    exponential_tau,
    exponential_window,
    force_fall,
    force_width,
    force_window,
    nearest_sample,
)

GOLDEN = os.path.join(os.path.dirname(__file__), 'data', 'modal_test_windows.json')
TOLERANCE = 1e-12

DEFINITIONS = {
    'force': ('w[n] = 1 for n < pre + width; (1 + cos(pi (n - pre - width) / fall)) / 2 for '
              'pre + width <= n < pre + width + fall; 0 for n >= pre + width + fall. n counts '
              'from 0 at the frame\'s first sample; pre is the trigger\'s sample in the frame '
              '(the pre-trigger length); width is the pulse allowance after the trigger; all '
              'in samples. A width given as a fraction of the block after the trigger is '
              'floor(fraction * (block - pre) + 0.5) samples; the fall, when not given, is '
              'max(1, floor(0.1 * width + 0.5)). pre + width + fall beyond block is refused.'),
    'exponential': ('w[n] = exp(-n / (tau_s * rate_hz)), n counting from 0 at the frame\'s '
                    'first sample; applied to the force and every response alike.'),
    'damping': ('A mode windowed by exp(-t / tau_s) has its decay rate raised by 1 / tau_s, '
                'so its damping ratio reads high by about 1 / (tau_s omega_n).'),
}

#: each case: every number its samples follow from
CASES = [
    {'window': 'force', 'block': 64, 'pre': 0, 'width': 8, 'fall': 4},
    {'window': 'force', 'block': 64, 'pre': 16, 'width': 8, 'fall': 4},
    {'window': 'force', 'block': 64, 'pre': 10, 'width': 6, 'fall': 1},
    {'window': 'force', 'block': 32, 'pre': 4, 'width': 20, 'fall': 8},
    {'window': 'force', 'block': 256, 'pre': 32, 'width': 22, 'fall': 2},
    {'window': 'exponential', 'block': 64, 'rate_hz': 64.0, 'tau_s': 0.25},
    {'window': 'exponential', 'block': 256, 'rate_hz': 1024.0, 'tau_s': 0.05},
    {'window': 'exponential', 'block': 8, 'rate_hz': 8.0, 'tau_s': math.inf},
]


def samples(case: dict) -> np.ndarray:
    """A case's samples as the analysis core makes them."""
    if case['window'] == 'force':
        return force_window(case['block'], case['pre'], case['width'], case['fall'])
    return exponential_window(case['block'], case['tau_s'] * case['rate_hz'])


def _plain(value):
    return 'inf' if value == math.inf else value


def make() -> dict:
    """The golden file's contents."""
    return {'about': 'The force and exponential windows of an impact test, sample by sample. '
                     'Every implementation reproduces "samples" to "tolerance" from the other '
                     'members of its case. "inf" is an infinite time constant (no decay).',
            'tolerance': TOLERANCE, 'definitions': DEFINITIONS,
            'cases': [{**{k: _plain(v) for k, v in case.items()},
                       'samples': [float(x) for x in samples(case)]} for case in CASES]}


def _golden() -> dict:
    with open(GOLDEN, encoding='utf-8') as f:
        return json.load(f)


def test_modal_test_windows_match_the_golden_vectors():
    golden = _golden()
    assert golden['tolerance'] == TOLERANCE and golden['definitions'] == DEFINITIONS
    stated = [{k: (math.inf if v == 'inf' else v) for k, v in c.items() if k != 'samples'}
              for c in golden['cases']]
    assert stated == CASES
    for case, given in zip(golden['cases'], stated, strict=True):
        np.testing.assert_allclose(samples(given), case['samples'], rtol=0, atol=TOLERANCE,
                                   err_msg=str(given))


def test_modal_test_force_window_holds_one_from_the_trigger_through_the_pulse():
    w = force_window(64, 16, 8, 4)
    # one over the pre-trigger and the pulse: the trigger is sample 16, the pulse 8 more
    assert np.all(w[:25] == 1.0)
    np.testing.assert_allclose(w[25:28], [0.5 * (1 + math.cos(math.pi * k / 4))
                                          for k in (1, 2, 3)], atol=1e-15)
    assert np.all(w[28:] == 0.0)
    assert np.all(np.diff(w) <= 0.0)
    # a hit at the trigger survives a width far under the pre-trigger
    assert force_window(4096, 64, 4)[64:68].min() == 1.0


def test_modal_test_force_width_is_a_fraction_of_the_block_after_the_trigger():
    # 0.01 of the 4032 samples after a 64-sample pre-trigger is 40.32, so 40
    assert force_width(0.01, 4096, 64) == 40
    assert force_fall(40) == 4 and force_fall(3) == 1 and FORCE_FALL == 0.1
    # a half rounds up, where Python's round would take it to even
    assert nearest_sample(2.5) == 3 and nearest_sample(3.5) == 4 and round(2.5) == 2
    assert force_window(64, 0, 10)[10] == 1.0 and force_window(64, 0, 10)[11] == 0.0


def test_modal_test_exponential_window_is_scipys_exponential_from_the_first_sample():
    from scipy.signal.windows import exponential

    for length, tau in ((64, 16.0), (1024, 200.5), (7, 0.3)):
        np.testing.assert_allclose(exponential_window(length, tau),
                                   exponential(length, center=0, tau=tau, sym=False),
                                   rtol=1e-14, atol=0)


def test_modal_test_exponential_tau_from_an_end_value_ends_there():
    for length, end in ((256, 0.01), (1024, 0.05), (2, 0.5)):
        w = exponential_window(length, exponential_tau(end, length))
        assert w[0] == 1.0 and w[-1] == pytest.approx(end, rel=1e-12)
    assert math.isinf(exponential_tau(1.0, 8))
    assert np.all(exponential_window(8, math.inf) == 1.0)


@pytest.mark.parametrize('call, words', [
    (lambda: force_window(64, 0, 0), 'above zero samples'),
    (lambda: force_window(64, 0, -2), 'above zero samples'),
    (lambda: force_window(64, 0, 2.5), 'whole number'),
    (lambda: force_window(64, 0, float('nan')), 'whole number'),
    (lambda: force_window(64, -1, 4), 'zero or more'),
    (lambda: force_window(64, 60, 4), 'runs past the block'),
    (lambda: force_width(float('nan'), 64, 0), 'above zero'),
    (lambda: force_width(float('inf'), 64, 0), 'above zero'),
    (lambda: force_width(0.0, 64, 0), 'above zero'),
    (lambda: force_width(0.001, 64, 0), 'holds no sample'),
    (lambda: exponential_window(8, 0.0), 'above zero'),
    (lambda: exponential_window(8, float('nan')), 'above zero'),
    (lambda: exponential_tau(0.0, 8), 'between 0 and 1'),
    (lambda: exponential_tau(float('nan'), 8), 'between 0 and 1'),
])
def test_modal_test_windows_refuse_what_names_no_window(call, words):
    with pytest.raises(ValueError, match=words):
        call()


@pytest.mark.parametrize('name', ['force', 'exponential'])
def test_modal_test_transient_windows_are_not_averaging_windows(name):
    """A continuous record's averaged spectra take no impact window: the names are refused
    where every other window is accepted, as they were before the modal test."""
    assert name not in WINDOWS
    with pytest.raises(ValueError, match='unknown window'):
        normalize_window(name)
    with pytest.raises(ValueError, match='unknown window'):
        Averaging(frame_length=256, window=name)


if __name__ == '__main__':  # pragma: no cover - writes the golden file
    path = sys.argv[1] if len(sys.argv) > 1 else GOLDEN
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'w', encoding='utf-8') as f:
        json.dump(make(), f, indent=1)
        f.write('\n')
    print(path)
