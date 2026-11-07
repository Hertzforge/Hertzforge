"""One rule for a point a specification does not write: in an autospectrum a
zero or NaN breakpoint is a gap, and every reader reads the gap; in a cross
term a zero is a value, and every reader reads the value."""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core import compliance
from hertzforge.core.averaging import Averaging
from hertzforge.core.data import Psd, Specification
from hertzforge.core.signals import random_history
from hertzforge.service import projects
from hertzforge.service.ops_compare import OPERATIONS

D = 'acceleration**2/frequency'
OPS = {op.id: op for op in OPERATIONS}


def _gapped(levels=(1.0, 1.0, 0.0, 1.0), at=(20.0, 100.0, 500.0, 1000.0)):
    return Specification(np.array(at), np.array([levels]), response_dof=['1Z+'],
                         ordinate_dim=[D])


@pytest.mark.parametrize('hole', [0.0, np.nan])
def test_every_reading_of_a_gap_agrees(hole):
    spec = _gapped((1.0, 1.0, hole, 1.0))
    measured = Psd(np.arange(0.0, 1025.0), np.ones((1, 1025)), response_dof=['1Z+'],
                   ordinate_dim=[D])
    assert compliance.coverage(spec, 0) == [(20.0, 100.0)]
    assert spec.area(0) == pytest.approx(80.0)
    assert compliance.specification_rms(spec, 0) == pytest.approx(np.sqrt(80.0))
    assert compliance.compare(spec, measured, scale_db=0)['specification_rms'] \
        == pytest.approx(np.sqrt(80.0))
    assert spec.to_octave(3).area(0) == pytest.approx(80.0)
    assert spec.extent(0) == spec.extent() == (20.0, 100.0)
    # a point with a gap either side is on no segment, so nothing there either
    assert np.isnan(compliance.record_at(spec, 0, np.array([1000.0]))[0])


def test_the_service_reads_the_gap_as_the_comparison_does():
    project = hertzforge.Project()
    project.add('Spec', _gapped())
    op = OPS['project.spec_rms']
    values = op.resolve({'specification': 'Spec'},
                        objects={name: projects.kind_of(obj) for name, obj in project.items()})
    (reading,) = op.run(project, values)
    assert reading['rows'][0]['value'] == pytest.approx(float(np.sqrt(80.0)), rel=1e-3)


def test_a_segment_on_each_side_of_a_gap_both_count():
    spec = _gapped((1.0, 1.0, 0.0, 1.0, 1.0), (20.0, 100.0, 500.0, 1000.0, 2000.0))
    assert compliance.coverage(spec, 0) == [(20.0, 100.0), (1000.0, 2000.0)]
    assert spec.area(0) == pytest.approx(1080.0)
    assert spec.area(0, 200.0, 800.0) == 0.0
    assert spec.area(0, 50.0, 1500.0) == pytest.approx(50.0 + 500.0)
    assert spec.areas(0, [20.0, 200.0], [100.0, 800.0]) == pytest.approx([80.0, 0.0])
    assert spec.extent(0) == (20.0, 2000.0)
    banded = spec.to_octave(3)
    assert banded.area(0) == pytest.approx(1080.0)


def test_nothing_on_a_segment_is_not_a_number_area():
    spec = _gapped((1.0, 0.0, 1.0), (20.0, 100.0, 1000.0))
    assert np.isnan(spec.area(0))
    assert spec.extent(0) is None


def test_a_generated_signal_holds_no_power_in_a_gap():
    history = random_history(_gapped(), duration=20.0, sample_rate=4096.0, seed=1)
    assert np.std(history.ordinate[0]) == pytest.approx(np.sqrt(80.0), rel=0.05)


def _with_zero_cross_term():
    at = np.array([20.0, 100.0, 1000.0])
    cross = np.array([0.5, 0.0, 0.5], dtype=complex)
    return Specification(at, np.array([[1.0, 1.0, 1.0], cross, cross, [1.0, 1.0, 1.0]],
                                      dtype=complex),
                         response_dof=['1Z+', '1Z+', '2Z+', '2Z+'],
                         reference_dof=['1Z+', '2Z+', '1Z+', '2Z+'], ordinate_dim=[D] * 4)


def test_a_generated_signal_holds_a_zero_written_in_a_cross_term():
    spec = _with_zero_cross_term()
    rate = 4096.0
    history = random_history(spec, duration=100.0, sample_rate=rate, frame_length=4096, seed=1)
    n = history.ordinate.shape[1]
    cpsds = history.compute_cpsds(Averaging(frame_length=4096, overlap=0.5,
                                            window='hann').filled(n, rate))
    lines = np.array([200.0, 500.0])
    asked = np.abs(compliance.record_at(spec, 1, lines))
    assert asked == pytest.approx([0.0556, 0.2222], abs=1e-3)
    # a line alone is noisy to about a tenth; twenty beside it are not
    held = [abs(np.mean(cpsds.ordinate[1, np.abs(cpsds.abscissa - line) < 10.0]))
            for line in lines]
    assert held == pytest.approx(asked, abs=0.04)


def test_octave_bands_read_a_zero_in_a_cross_term_as_a_value():
    spec = _with_zero_cross_term()
    banded = spec.to_octave(3)
    assert banded.area(0) == pytest.approx(980.0)
    values = np.abs(banded.ordinate[1])
    assert np.isfinite(values).all() and values.max() < 0.5
    at_zero = int(np.argmin(np.abs(banded.abscissa - 100.0)))
    assert values[at_zero] < 0.15, 'the breakpoint at zero pulls its band down, not across'
