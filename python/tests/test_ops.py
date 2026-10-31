"""Each operation's result against the schema's views, and its numbers
against visualdynamics called directly."""

from __future__ import annotations

import dataclasses

import numpy as np
import pytest

from sds_analysis import frames
from sds_analysis.protocol import ProtocolError
from sds_analysis.server import OPERATIONS

OPS = {op.id: op for op in OPERATIONS}


def run(op_id, data, **params):
    op = OPS[op_id]
    return op.run(data, op.resolve(params, [c.value for c in data.channels]))


def wire_views(views, validate):
    """The views as they travel, references in place, checked."""
    wire, arrays = frames.to_wire({'views': views})
    validate(wire['views'], 'views')
    return wire, arrays


def by_label(reading):
    return {row['label'].split(' (')[0]: row for row in reading['rows']}


# ---- random.psd_vs_spec ---------------------------------------------------------

def test_psd_vs_spec_validates(random_run, validate):
    views = run('random.psd_vs_spec', random_run)
    wire_views(views, validate)
    assert [view['view'] for view in views] == ['spectrum', 'reading']
    assert views[0]['link'] == 'frequency'


def test_psd_vs_spec_reads_what_compliance_compare_reads(random_run):
    from visualdynamics.core import compliance

    channel = random_run.channels[0]
    expected = compliance.compare(random_run.specification, random_run.psds,
                                  channel.spec_record, channel.record,
                                  scale_db=random_run.scale_db)
    rows = by_label(run('random.psd_vs_spec', random_run,
                        channel=channel.value)[1])
    assert rows['Measured rms']['value'] == pytest.approx(
        expected['measured_rms'], rel=1e-3)
    assert rows['Specification rms']['value'] == pytest.approx(
        expected['specification_rms'], rel=1e-3)
    assert rows['RMS error']['value'] == pytest.approx(
        expected['difference_db'], abs=0.005)
    assert rows['Lines within abort']['value'] == pytest.approx(
        100 - expected['abort_percent'], abs=0.05)
    assert rows['Lines within warning']['value'] == pytest.approx(
        100 - expected['warning_percent'], abs=0.05)
    assert rows['Lines judged']['value'] == expected['lines']
    # the plate's first control channel is 4.5 dB high and 19 % outside
    # abort: both readings fail, so the channel does
    assert rows['RMS error']['status'] == 'fail'
    assert rows['Lines within abort']['status'] == 'fail'
    assert rows['Channel']['value'] == 'fail'
    verdict = compliance.verdict(compliance.compare_all(
        random_run.specification, random_run.psds, scale_db=0))
    assert rows['Run']['value'] == ('pass' if verdict['passed'] else 'fail')
    assert rows['Limits from']['value'] == (
        'specification: warning -3.0/+3.0 dB, abort -6.0/+6.0 dB')


def test_psd_vs_spec_series_carry_the_spec_and_its_limits(random_run):
    spectrum = run('random.psd_vs_spec', random_run)[0]
    roles = [(s['name'], s['role']) for s in spectrum['series']]
    assert roles[0][1] == 'measured'
    assert ('Specification', 'reference') in roles
    assert [r for _n, r in roles].count('tolerance') == 2
    assert [r for _n, r in roles].count('abort') == 2
    spec = next(s for s in spectrum['series'] if s['role'] == 'reference')
    # trimmed to the written band, nothing at or below 0 Hz on a log axis
    assert spec['x'][0] == pytest.approx(50.0)
    assert spec['x'][-1] == pytest.approx(1500.0)
    measured = spectrum['series'][0]
    assert (measured['x'] > 0).all()
    assert spectrum['y']['unit'] == '(m/s²)²/Hz'


def test_spectra_are_drawn_as_vd_draws_them(random_run):
    """The PSD is stepped across bins of the line spacing, with VD's edges,
    and the specification takes VD's own shape for it."""
    from visualdynamics.plot import bin_edges, drawing_shape

    spectrum = run('random.psd_vs_spec', random_run)[0]
    measured = spectrum['series'][0]
    assert measured['shape'] == 'steps'
    x, edges = measured['x'], measured['edges']
    assert edges.size == x.size + 1
    np.testing.assert_allclose(edges, bin_edges(x))
    df = float(np.median(np.diff(x)))
    np.testing.assert_allclose(np.diff(edges), df, rtol=1e-9)
    assert (edges[:-1] < x).all() and (x < edges[1:]).all()
    spec = next(s for s in spectrum['series'] if s['role'] == 'reference')
    assert spec.get('shape', 'line') == {'bin': 'steps', 'log_log': 'law'}.get(
        random_run.specification.interpolation,
        drawing_shape(random_run.specification))


def test_error_is_stepped_one_value_per_line(random_run):
    spectrum = run('random.error_db', random_run)[0]
    for s in spectrum['series']:
        assert s['shape'] == 'steps'
        assert s['edges'].size == s['y'].size + 1


def test_worst_line_is_the_largest_cell_error(random_run):
    rows = by_label(run('random.psd_vs_spec', random_run)[1])
    error = run('random.error_db', random_run)[0]['series'][0]
    k = int(np.nanargmax(np.abs(error['y'])))
    assert rows['Worst line deviation']['value'] == pytest.approx(
        error['y'][k], abs=0.005)
    assert rows['Worst line at']['value'] == pytest.approx(error['x'][k])


def test_a_force_channel_is_refused_not_judged(random_run):
    force = next(c for c in random_run.channels if c.spec_record is None)
    with pytest.raises(ProtocolError) as caught:
        run('random.psd_vs_spec', random_run, channel=force.value)
    assert caught.value.code == 'refused'
    assert 'not a control channel' in caught.value.message
    assert force.label in caught.value.message


def test_tolerance_param_applies_when_the_spec_has_no_limits(random_run,
                                                             validate):
    bare = dataclasses.replace(
        random_run, specification=random_run.specification.as_specification())
    assert not bare.specification.has_limits
    views = run('random.psd_vs_spec', bare, tolerance_db=1.0)
    wire_views(views, validate)
    roles = [s['role'] for s in views[0]['series']]
    assert roles.count('tolerance') == 2 and 'abort' not in roles
    rows = by_label(views[1])
    assert rows['Limits from']['value'] == 'tolerance param: +/-1 dB'
    wide = by_label(run('random.psd_vs_spec', bare, tolerance_db=12.0)[1])
    # a wider band holds more of the lines
    assert (wide['Lines within tolerance']['value']
            > rows['Lines within tolerance']['value'])


# ---- random.error_db -------------------------------------------------------------

def test_error_db_validates_and_is_linear_in_db(random_run, validate):
    views = run('random.error_db', random_run)
    wire_views(views, validate)
    (view,) = views
    assert view['y'] == {'label': 'Error', 'unit': 'dB', 'scale': 'linear'}
    assert view['link'] == 'frequency'
    limits = {s['name']: s['y'] for s in view['series']
              if s['role'] in ('tolerance', 'abort')}
    # the plate's limits are written at +/-3 and +/-6 dB about the target
    assert np.nanmedian(limits['Warning, upper']) == pytest.approx(3.0, abs=0.01)
    assert np.nanmedian(limits['Abort, lower']) == pytest.approx(-6.0, abs=0.01)


def test_error_db_cells_against_judge(random_run):
    from visualdynamics.core import compliance

    channel = random_run.channels[2]
    judged = compliance.judge(random_run.specification, random_run.psds,
                              channel.spec_record, channel.record, None, True,
                              random_run.scale_db)
    error = run('random.error_db', random_run,
                channel=channel.value)[0]['series'][0]['y']
    good = judged['judged']
    assert np.allclose(error[good],
                       10 * np.log10(judged['held'][good] / judged['asked'][good]))
    assert np.isnan(error[~good]).all()


# ---- modal.modes -------------------------------------------------------------------

@pytest.mark.parametrize('model', ('drone', 'plate_fem'))
def test_modes_validate(model, request, validate):
    data = request.getfixturevalue(model)
    views = run('modal.modes', data)
    wire, arrays = wire_views(views, validate)
    (view,) = views
    assert view['normalization'] == 'mass'
    assert 'Mass-Normalized' in view['title']
    assert len(view['modes']) == 6
    n = view['nodes'].shape[0]
    assert view['nodes'].dtype == np.float32 and view['nodes'].shape == (n, 3)
    assert view['triangles'].dtype == np.uint32
    assert view['triangles'].max() < n
    for mode in view['modes']:
        assert mode['shape'].dtype == np.float32
        assert mode['shape'].shape == (n, 3)
        assert mode['frequency_hz'] > 0 and mode['damping'] is None
    names = {frames.dtype_name(a) for a in arrays.values()}
    assert names <= {'f32', 'u32'}


def test_drone_draws_beams_as_lines(drone):
    (view,) = run('modal.modes', drone, count=1)
    assert view['lines'].shape[1] == 2 and len(view['lines']) > 0
    assert view['lines'].max() < view['nodes'].shape[0]


def test_translations_equal_node_displacements(drone):
    from visualdynamics.deform import node_displacements

    for k in (0, 7, 29):
        expected = node_displacements(drone.geometry, drone.shapes.coordinate,
                                      drone.shapes.shape_matrix[k])
        assert np.allclose(drone.translations(k),
                           expected[drone.node_rows], rtol=0, atol=1e-12)


def test_modes_skip_rigid_unless_asked(drone):
    elastic = run('modal.modes', drone, count=3)[0]['modes']
    assert [m['index'] for m in elastic] == [7, 8, 9]
    rigid = run('modal.modes', drone, count=8, rigid=True)[0]['modes']
    assert [m['index'] for m in rigid] == list(range(1, 9))
    assert [m['frequency_hz'] for m in rigid[:6]] == [0.0] * 6


def test_modes_say_when_fewer_exist_than_asked(plate_fem):
    from types import SimpleNamespace

    shapes = plate_fem.shapes
    fewer = dataclasses.replace(plate_fem, shapes=SimpleNamespace(
        frequency=shapes.frequency[:10], damping=shapes.damping[:10],
        shape_matrix=shapes.shape_matrix[:10]))
    (view,) = run('modal.modes', fewer, count=24)
    assert [m['index'] for m in view['modes']] == [7, 8, 9, 10]
    assert view['note'].startswith('4 of the 24 modes asked for')
    (view,) = run('modal.modes', plate_fem, count=24)
    assert len(view['modes']) == 24 and 'note' not in view


def test_a_model_with_only_rigid_modes_is_refused(plate_fem):
    from types import SimpleNamespace

    shapes = plate_fem.shapes
    rigid = dataclasses.replace(plate_fem, shapes=SimpleNamespace(
        frequency=shapes.frequency[:6], damping=shapes.damping[:6],
        shape_matrix=shapes.shape_matrix[:6]))
    with pytest.raises(ProtocolError, match='no elastic mode among the 6'):
        run('modal.modes', rigid)
