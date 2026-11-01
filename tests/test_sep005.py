"""SEP 005 in and out — interoperating with the sdypy ecosystem.

SEP 005 is sdypy's unified-timeseries standard: a plain dict per
series, a list for several. It is an in-memory standard rather than a
file format, so the surface is `hertzforge.from_sep005` and
`TimeHistory.to_sep005` — and the exported dicts are proved against the
sdypy project's *own* validator (`sdypy-sep005`, a test-only
dependency; the package itself imports nothing of sdypy's).
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge


def _series(channels=2, fs=256.0, samples=512):
    t = np.arange(samples) / fs
    return {'data': np.vstack([np.sin(2 * np.pi * 10.0 * t)] * channels),
            'fs': fs, 'name': 'shaker run',
            'channel_name': [f'{100 + i}Z+' for i in range(channels)],
            'unit_str': ['m/s²'] * channels, 'quantity': 'a'}


def test_a_timeseries_dict_becomes_a_time_history():
    history = hertzforge.from_sep005(_series())
    assert isinstance(history, hertzforge.TimeHistory)
    assert history.num_records == 2
    assert list(history.response_dof) == ['100Z+', '101Z+']
    assert history.abscissa[1] == pytest.approx(1.0 / 256.0)
    assert history.ordinate_dim == ['acceleration', 'acceleration'], (
        'a stated unit is a declaration, not a guess — m/s² defines')


def test_units_convert_to_si_and_g_is_gravity():
    """unit_str entries scale exactly as define_units would — including
    the house rule that a bare g is standard gravity, never grams."""
    series = _series()
    series['unit_str'] = ['m/s²', 'g']
    history = hertzforge.from_sep005(series)
    assert history.ordinate[1].max() == pytest.approx(
        9.80665 * history.ordinate[0].max())


def test_an_unparseable_unit_stays_a_claim():
    """Nothing scales by a guess: a unit the parser cannot read leaves
    the values raw with the claim kept beside them."""
    series = _series()
    series['unit_str'] = ['m/s²', 'furlongs of grace']
    history = hertzforge.from_sep005(series)
    assert history.ordinate_unit[1] is None
    assert history.ordinate_dim[1] == 'unknown'
    assert history.dimension_hint[1] == 'acceleration', (
        "the 'a' quantity letter still says what it measures")
    assert history.ordinate[1].max() == pytest.approx(1.0), 'raw values'


def test_a_quantity_letter_becomes_a_hint_without_a_unit():
    series = _series()
    series['unit_str'] = None
    series['quantity'] = 'd'
    history = hertzforge.from_sep005(series)
    assert history.ordinate_dim == ['unknown', 'unknown']
    assert history.dimension_hint == ['length', 'length'], (
        "the package's word for a displacement — a hint in words the "
        'units machinery cannot read would narrow nothing')


def test_a_time_vector_is_taken_as_given():
    series = _series()
    del series['fs']
    series['time'] = np.arange(512) / 256.0
    history = hertzforge.from_sep005(series)
    assert history.abscissa[-1] == pytest.approx(511 / 256.0)


def test_the_refusals_name_what_is_missing():
    with pytest.raises(ValueError, match='data'):
        hertzforge.from_sep005({'fs': 256.0, 'name': 'x'})
    with pytest.raises(ValueError, match='neither'):
        hertzforge.from_sep005({'data': np.ones(8), 'name': 'x'})
    bad = _series()
    del bad['fs']
    bad['time'] = np.arange(7)
    with pytest.raises(ValueError, match='must match'):
        hertzforge.from_sep005(bad)
    bad = _series()
    bad['channel_name'] = ['only one']
    with pytest.raises(ValueError, match='must match'):
        hertzforge.from_sep005(bad)


def test_a_list_returns_named_objects_numbering_clashes():
    a, b, c = _series(), _series(), _series()
    c['name'] = 'other run'
    out = hertzforge.from_sep005([a, b, c])
    assert list(out) == ['shaker run', 'shaker run (2)', 'other run']
    assert all(isinstance(v, hertzforge.TimeHistory)
               for v in out.values())


def test_export_speaks_si_where_units_are_defined():
    history = hertzforge.from_sep005(_series())
    out, = history.to_sep005('shaker run')
    assert out['name'] == 'shaker run'
    assert out['fs'] == pytest.approx(256.0)
    assert out['channel_name'] == ['100Z+', '101Z+']
    assert out['unit_str'] == 'm/s**2', (
        'values are held SI, so the unit says SI — one string, which '
        "is what sdypy's validator holds a series to")
    assert out['quantity'] == 'a'


def test_mixed_units_split_into_the_standards_list_form():
    """An accelerometer beside a force gauge cannot be one compliant
    series — the validator holds unit_str to a single string — so the
    export splits by unit, named to stay distinguishable."""
    t = np.arange(64) / 64.0
    history = hertzforge.TimeHistory(
        t, np.ones((3, 64)), response_dof=['1Z+', '2Z+', '1Z+'])
    history.define_units(['m/s**2', 'm/s**2', 'N'])
    accel, force = history.to_sep005('drive point')
    assert accel['name'] == 'drive point [m/s**2]'
    assert accel['channel_name'] == ['1Z+', '2Z+']
    assert force['name'] == 'drive point [N]'
    assert force['quantity'] == 'f'


def test_export_claims_nothing_for_undefined_units():
    t = np.arange(64) / 64.0
    history = hertzforge.TimeHistory(t, np.ones((1, 64)),
                                         response_dof=['1X+'])
    out, = history.to_sep005('raw')
    assert out['unit_str'] == '', (
        'an empty unit is allowed; an invented one would claim a scale')
    assert 'quantity' not in out


def test_an_uneven_record_sends_its_time_vector():
    t = np.array([0.0, 0.1, 0.3, 0.7])
    history = hertzforge.TimeHistory(t, np.ones((1, 4)),
                                         response_dof=['1X+'])
    out, = history.to_sep005('uneven')
    assert 'fs' not in out
    assert np.array_equal(out['time'], t)


def test_the_round_trip_is_exact():
    history = hertzforge.from_sep005(_series())
    again = hertzforge.from_sep005(
        history.to_sep005('shaker run'))['shaker run']
    assert np.allclose(again.ordinate, history.ordinate)
    assert list(again.response_dof) == list(history.response_dof)
    assert again.ordinate_dim == history.ordinate_dim
    assert again.abscissa == pytest.approx(history.abscissa)


def test_the_export_passes_sdypys_own_validator():
    """The point of the whole module: what we hand a sdypy user must
    pass the sdypy project's compliance checker, not our reading of the
    spec. Both shapes — SI-defined and raw-with-empty-units — and an
    uneven record with a time vector."""
    sep005 = pytest.importorskip('sdypy_sep005.sep005')

    defined = hertzforge.from_sep005(_series())
    t = np.arange(64) / 64.0
    raw = hertzforge.TimeHistory(t, np.ones((2, 64)),
                                     response_dof=['1X+', '2X+'])
    uneven = hertzforge.TimeHistory(
        np.array([0.0, 0.1, 0.3, 0.7]), np.ones((1, 4)),
        response_dof=['1X+'])
    mixed = hertzforge.TimeHistory(
        t, np.ones((2, 64)), response_dof=['1Z+', '1Z+'])
    mixed.define_units(['m/s**2', 'N'])
    # their assert takes the standard's list-of-timeseries form
    sep005.assert_sep005([*defined.to_sep005('defined run'),
                          *raw.to_sep005('raw run'),
                          *uneven.to_sep005('uneven run'),
                          *mixed.to_sep005('drive point')])
