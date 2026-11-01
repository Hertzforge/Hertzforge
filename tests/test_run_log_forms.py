"""The Run Log's forms: the test header in full, carried fields marked
until confirmed, the readings of each kind of test (a sweep against the
continuous rating, a shot's four checks and its pulses against the
budget, a survey's result), the notch against the plan, the findings,
and the predictions read beside what is observed.

Every log is written under `tmp_path`, with a fixed login and machine
name, on a clock the test sets. Each form is driven the way the keeper
drives it: boxes typed, Enter (or `write`) pressed, and the entry read
back from the file.
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

import numpy as np
import pytest

from hertzforge.core.data import Frf, Specification
from hertzforge.core.run_log import RunLog

WHO = ('tester', 'bench')

ZONE = timezone(timedelta(hours=-6))

START = datetime(2026, 10, 30, 8, 0, tzinfo=ZONE)

G = 9.80665

RANDOM = {'test_kind': 'Random Vibration', 'title': 'Random, Z',
          'steps_db': '-12, -6, -3, 0', 'sigma': '3',
          'rated_current_peak': '800', 'rail_voltage': '250'}


class Clock:
    """A clock that reads what the test sets, moved by hand."""

    def __init__(self, now=START):
        self.now = now

    def __call__(self):
        return self.now

    def move(self, **step):
        self.now += timedelta(**step)


@pytest.fixture
def clock():
    return Clock()


def _entries(path, kind=None):
    entries = RunLog.read(path).view()
    return [e for e in entries if kind is None or e.kind == kind]


def _limit_prediction():
    """A limit channel of transmissibility 4 over 240 to 260 Hz against a
    flat profile of 0.18 g^2/Hz, under a flat reference of 0.04 g^2/Hz:
    the control comes down 5.51 dB there."""
    from hertzforge.core.limit_prediction import predict_limits

    f = np.arange(0.0, 2001.0, 1.0)
    t = np.ones(len(f), dtype=complex)
    t[(f >= 240) & (f <= 260)] = 4.0
    frf = Frf(f, t[None], response_dof=['2Z+'], reference_dof=['1Z+'],
              ordinate_dim='acceleration/acceleration')
    values = np.array([[0.04, 0.04], [0.18, 0.18]]) * G ** 2
    spec = Specification(
        np.array([20.0, 2000.0]), values, response_dof=['1Z+', '2Z+'],
        ordinate_dim='acceleration**2/frequency',
        abort_lower=np.array([values[0] * 0.5, np.full(2, np.nan)]),
        abort_upper=np.array([values[0] * 2.0, np.full(2, np.nan)]))
    return frf, spec, predict_limits(frf, spec)


def _drive_prediction():
    """397 lines of 5 Hz at 0.01 V^2/Hz: 4.455 V rms; at a peak factor of
    three, 13.37 V against a 10 V limit, 134 %."""
    from hertzforge.core.drive_prediction import predict_drives

    f = np.arange(0.0, 2002.5, 5.0)
    frf = Frf(f, np.full(len(f), 2.0 * G, dtype=complex)[None],
              response_dof='1Z+', reference_dof='1Z+',
              ordinate_dim='acceleration/voltage')
    spec = Specification(np.array([20.0, 2000.0]),
                         np.array([[0.04, 0.04]]) * G ** 2,
                         response_dof=['1Z+'],
                         ordinate_dim='acceleration**2/frequency')
    return predict_drives(frf, spec, drive_limit=10.0, peak_factor=3.0)
