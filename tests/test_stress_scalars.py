"""The stress scalars and modal stress (`core.stress_scalars`) against
`schema/golden/stress_scalars.json`, the numbers the 3-D view's own formulas are held to as well:
tensors whose principal stresses are known by construction, and a two-mode sum whose von Mises is
formed from the summed tensor."""

from __future__ import annotations

import json
import pathlib

import numpy as np
import pytest

from hertzforge.core.stress_scalars import (
    SCALARS,
    clip_peak,
    modal_stress,
    mode_peaks,
    principal_stresses,
    stress_scalar,
    von_mises,
)

GOLDEN = json.loads((pathlib.Path(__file__).resolve().parents[1] / 'schema' / 'golden'
                     / 'stress_scalars.json').read_text(encoding='utf-8'))


@pytest.mark.parametrize('case', GOLDEN['cases'], ids=lambda c: c['name'])
def test_every_scalar_of_a_tensor_with_known_principal_stresses(case):
    for scalar in SCALARS:
        assert stress_scalar(case['tensor'], scalar) == pytest.approx(case['expected'][scalar], abs=1e-9), scalar


def test_the_golden_names_every_scalar():
    assert set(GOLDEN['cases'][0]['expected']) == set(SCALARS)


def test_one_elements_stress_at_one_instant_is_the_shapes_times_the_modal_coordinates():
    g = GOLDEN['modal_sum']
    shapes = np.reshape(g['shapes'], (g['modes'], g['elements'], 6))
    values = np.reshape(g['values'], (g['modes'], g['samples']))
    at_k = modal_stress(shapes, values[:, g['k']])
    # element 1 at sample k by hand: q0 S0 + q1 S1
    by_hand = values[0, g['k']] * shapes[0, 1] + values[1, g['k']] * shapes[1, 1]
    np.testing.assert_allclose(at_k[1], by_hand, atol=1e-12)
    assert von_mises(at_k[1]) == pytest.approx(g['expected'][1]['von_mises'], abs=1e-9)
    # over the clip, sample k is the same tensor
    np.testing.assert_allclose(modal_stress(shapes, values)[g['k']], at_k, atol=1e-12)


def test_von_mises_is_formed_from_the_summed_tensor_not_summed_per_mode():
    g = GOLDEN['modal_sum']
    shapes = np.reshape(g['shapes'], (g['modes'], g['elements'], 6))
    q = np.reshape(g['values'], (g['modes'], g['samples']))[:, g['k']]
    per_mode = float(np.sum(np.abs(q) * von_mises(shapes[:, 0])))
    summed = float(von_mises(modal_stress(shapes, q)[0]))
    assert summed == pytest.approx(g['expected'][0]['von_mises'], abs=1e-9)
    assert abs(per_mode - summed) > 1.0


def test_principal_stresses_come_largest_first():
    general = next(c for c in GOLDEN['cases'] if c['name'] == 'general')
    np.testing.assert_allclose(principal_stresses(general['tensor']), [120.0, 10.0, -35.0], atol=1e-9)


def test_the_clip_peak_is_the_largest_von_mises_over_every_sample_and_element():
    g = GOLDEN['modal_sum']
    shapes = np.reshape(g['shapes'], (g['modes'], g['elements'], 6))
    values = np.reshape(g['values'], (g['modes'], g['samples']))
    every = von_mises(modal_stress(shapes, values))
    assert clip_peak(shapes, values, 'von_mises') == pytest.approx(float(every.max()))
    # a signed scalar's peak is its largest magnitude, wherever its sign
    xx = modal_stress(shapes, values)[..., 0]
    assert clip_peak(shapes, values, 'xx') == pytest.approx(float(np.abs(xx).max()))


def test_the_clip_peak_reads_a_long_clip_a_chunk_at_a_time(monkeypatch):
    from hertzforge.core import stress_scalars

    rng = np.random.default_rng(3)
    shapes = rng.normal(size=(4, 5, 6))
    values = rng.normal(size=(4, 70))
    whole = clip_peak(shapes, values, 'max_principal')
    monkeypatch.setattr(stress_scalars, 'CHUNK', 8)
    assert clip_peak(shapes, values, 'max_principal') == pytest.approx(whole)


def test_the_clip_peak_passes_over_a_nan_sample_and_is_zero_for_a_dead_clip():
    g = GOLDEN['modal_sum']
    shapes = np.reshape(g['shapes'], (g['modes'], g['elements'], 6))
    values = np.reshape(g['values'], (g['modes'], g['samples'])).copy()
    clean = clip_peak(shapes, values[:, [1]], 'von_mises')
    values[0, 0] = np.nan
    assert clip_peak(shapes, values[:, :2], 'von_mises') == pytest.approx(clean)
    assert clip_peak(shapes, np.zeros((2, 4)), 'von_mises') == 0.0


def test_a_real_modes_peak_is_its_shapes_and_a_complex_one_turns_through_its_cycle():
    g = GOLDEN['modal_sum']
    shapes = np.reshape(g['shapes'], (g['modes'], g['elements'], 6))
    np.testing.assert_allclose(mode_peaks(shapes, 'von_mises'), von_mises(shapes).max(axis=1))
    # a mode whose imaginary part is its real part turned a quarter: its peak is sqrt(2) times
    turned = mode_peaks(shapes, 'von_mises', shapes_imag=shapes)
    np.testing.assert_allclose(turned, np.sqrt(2) * von_mises(shapes).max(axis=1), rtol=1e-3)


def test_an_unknown_scalar_and_a_short_tensor_are_refused_by_name():
    with pytest.raises(ValueError, match='not a stress scalar'):
        stress_scalar([0.0] * 6, 'octahedral')
    with pytest.raises(ValueError, match='six components'):
        von_mises([1.0, 2.0, 3.0])
    with pytest.raises(ValueError, match='2 stress shapes for 3 modal coordinates'):
        modal_stress(np.zeros((2, 1, 6)), np.zeros(3))
