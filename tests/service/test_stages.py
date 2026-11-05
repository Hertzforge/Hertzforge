"""Visual Dynamics' 3-D stages: two densities paired by channel, a
specification with its bands and a measurement's exceedances, and a
scalogram as a surface."""

from __future__ import annotations

import numpy as np
import pytest

from hertzforge.project import Project
from hertzforge.service import display_units
from hertzforge.service.ops_sine_shock import OPERATIONS as SHOCK_OPERATIONS
from hertzforge.service.ops_stages import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views
from .test_compare import imported
from .test_compare import run as run_compare
from .test_sine_shock import history

OPS = {op.id: op for op in OPERATIONS + SHOCK_OPERATIONS}


@pytest.fixture
def plate():
    found = imported('random.nc4')
    found.compute_psds('Time History')
    return found


@pytest.fixture
def two(plate):
    psd = plate['Time History PSDs']
    plate.add('Noise', type(psd)(psd.abscissa, psd.ordinate * 0.01,
                                 response_dof=psd.response_dof,
                                 ordinate_dim=psd.ordinate_dim))
    return plate


def run(project, op_id, **given):
    from hertzforge.service import projects

    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


def test_the_paired_stage_puts_both_densities_at_each_channel(two, validate):
    (view,) = run(two, 'project.paired_stage', first='Noise', second='Time History PSDs')
    wire_views([view], validate)
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    channels = waterfall_arrays(two['Noise'])['labels']
    assert view['stations'] == channels and len(channels) < two['Noise'].num_records, \
        'one quantity on a stage'
    assert view['depths'] == [k for k in range(len(channels)) for _ in (0, 1)]
    assert view['roles'] == ['reference', 'measured'] * len(channels)
    assert view['records'][:2] == [f'Noise: {channels[0]}',
                                   f'Time History PSDs: {channels[0]}'], 'the louder second'
    quiet, loud = (view['z_values'][view['offsets'][r]:view['offsets'][r + 1]]
                   for r in (0, 1))
    assert np.nanmedian(loud / quiet) == pytest.approx(100.0, rel=1e-3)


def test_signal_to_noise_on_the_stage_is_the_flat_plots_ratio(two, validate):
    (view,) = run(two, 'project.paired_stage', first='Noise', second='Time History PSDs',
                  mode='ratio')
    wire_views([view], validate)
    flat, _reading = run_compare(two, 'project.signal_to_noise', first='Noise',
                                 second='Time History PSDs')
    assert view['z'] == {'label': 'Signal to noise', 'unit': 'dB', 'scale': 'linear'}
    assert view['roles'] == ['measured'] * len(view['stations'])
    first = view['z_values'][view['offsets'][0]:view['offsets'][1]]
    assert np.nanmedian(first) == pytest.approx(np.nanmedian(flat['series'][0]['y']))
    with pytest.raises(ProtocolError, match='against itself'):
        run(two, 'project.paired_stage', first='Noise', second='Noise')


def test_a_specification_alone_stands_with_its_bands(plate, validate):
    (view,) = run(plate, 'project.spec_stage')
    wire_views([view], validate)
    spec = plate['Specification']
    assert view['stations'] == [str(d) for d in spec.response_dof]
    assert set(view['roles']) == {'measured', 'tolerance', 'abort'}, 'alone, colored by level'
    kinds = {(z['kind'], z['side']) for z in view['zones']}
    assert kinds == {('warning', 'upper'), ('warning', 'lower'), ('abort', 'upper'),
                     ('abort', 'lower')}
    warning = next(z for z in view['zones'] if z['kind'] == 'warning')
    assert 'outer' in warning, 'between warning and abort'
    abort = next(z for z in view['zones'] if z['kind'] == 'abort')
    assert 'outer' not in abort, 'past abort, to the top or floor'
    assert 'exceed' not in view


def test_a_measurement_on_the_stage_shades_the_lines_it_went_past(plate, validate):
    from hertzforge.core.compliance import outside

    (view,) = run(plate, 'project.spec_stage', spectrum='Time History PSDs')
    wire_views([view], validate)
    assert view['title'] == 'Time History PSDs against Specification'
    assert view['roles'].count('measured') == len(view['stations'])
    assert 'reference' in view['roles'], 'the requirement in gray beside a measurement'
    spec, psd = plate['Specification'], plate['Time History PSDs']
    x = np.asarray(psd.abscissa, float)
    over = outside(x, np.real(psd.ordinate[0]), np.asarray(spec.abscissa, float),
                   np.real(spec.limits['abort_upper'][0]), True)
    hits = [e for e in view['exceed'] if e['station'] == 0 and e['side'] == 'upper']
    assert sum(len(e['x0']) for e in hits) == int(over.sum())
    assert all(np.all(e['x1'] > e['x0']) for e in view['exceed'])


def test_the_stage_marks_follow_the_display_units(plate):
    (view,) = run(plate, 'project.spec_stage', spectrum='Time History PSDs')
    zone = view['zones'][0]
    inner = np.array(zone['inner'], dtype=float)
    hit = view['exceed'][0]
    level = np.array(hit['z'], dtype=float)
    display_units.convert([view], 'in-slinch-lbf-s (g)')
    assert view['z']['unit'] == 'g²/Hz'
    g2 = 9.80665 ** 2
    assert np.nanmax(np.abs(zone['inner'] * g2 / inner - 1.0)) < 1e-4
    assert np.nanmax(np.abs(hit['z'] * g2 / level - 1.0)) < 1e-4


def test_a_scalogram_stands_as_a_surface_inside_its_cone(validate):
    rate = 1024.0
    t = np.arange(8192) / rate
    project = Project()
    project.add('Tone', history(t, 3.0 * np.sin(2.0 * np.pi * 64.0 * t), ['1Z+']))
    surface, picture, _ridge, _reading = run(project, 'project.wavelet')
    wire_views([surface], validate)
    assert surface['surface'] is True
    rows = len(picture['rows']['ticks'])
    assert surface['stations'] == picture['rows']['ticks'][::-1], 'low frequencies in front'
    assert len(surface['offsets']) == rows + 1
    counts = np.diff(surface['offsets'])
    assert np.all(counts == counts[0]), 'every row on the same clock'
    start, end = surface['walls']
    assert np.all(np.diff(start) <= 0) and np.all(np.diff(end) >= 0), 'narrowing as f rises'
    assert np.nanmax(surface['z_values']) == pytest.approx(3.0, rel=0.02)
