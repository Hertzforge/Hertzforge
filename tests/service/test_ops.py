"""Each operation's result against the schema's views, and its numbers
against hertzforge called directly."""

from __future__ import annotations

import dataclasses

import numpy as np
import pytest

from hertzforge.service import frames
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS

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


# ---- random.waterfall -----------------------------------------------------------

@pytest.mark.parametrize('of', ('psd', 'octave', 'time', 'coherence'))
def test_waterfall_validates_and_its_offsets_cut_its_points(random_run, validate, of):
    (view,) = run('random.waterfall', random_run, of=of)
    wire_views([view], validate)
    offsets = view['offsets']
    assert offsets[0] == 0 and offsets[-1] == view['x_values'].size == view['z_values'].size
    assert len(view['records']) == offsets.size - 1
    assert np.all(np.diff(offsets.astype(int)) > 0)
    assert view['x_values'].dtype == np.float32 and offsets.dtype == np.uint32


def test_a_psd_waterfall_keeps_every_tread_and_its_values(random_run):
    """A PSD arrives as its step outline, two points per line and never
    thinned, in SI and not logged: the shell takes the log."""
    (view,) = run('random.waterfall', random_run, of='psd')
    psds = random_run.project['Time History PSDs']
    lines = psds.abscissa.size
    assert np.all(np.diff(view['offsets'].astype(int)) == 2 * lines)
    assert view['z'] == {'label': 'PSD', 'unit': '(m/s²)²/Hz', 'scale': 'log'}
    first = psds.record_label(0)
    assert view['records'][0] == first
    z = view['z_values'][:2 * lines]
    np.testing.assert_allclose(z[::2], psds.ordinate[0].real.astype(np.float32), rtol=1e-6)


def test_a_time_waterfall_is_decimated_to_the_point_budget(random_run):
    from hertzforge.drawing.viz.waterfall import POINT_BUDGET

    (view,) = run('random.waterfall', random_run, of='time')
    history = random_run.project['Time History']
    assert history.abscissa.size > POINT_BUDGET
    assert np.all(np.diff(view['offsets'].astype(int)) <= POINT_BUDGET)
    assert view['z']['scale'] == 'linear' and view['x']['unit'] == 's'
    # every record's own extremes survive the thinning
    k = 0
    i0, i1 = int(view['offsets'][k]), int(view['offsets'][k + 1])
    assert np.isclose(view['z_values'][i0:i1].max(), history.ordinate[k].max(), rtol=1e-6)


def test_a_coherence_waterfall_pins_its_level_range(random_run):
    (view,) = run('random.waterfall', random_run, of='coherence')
    assert view['z_range'] == [0.0, 1.05]
    assert view['z']['unit'] is None


def test_records_of_another_quantity_are_left_out_with_a_note(random_run):
    (view,) = run('random.waterfall', random_run, of='psd')
    assert len(view['records']) == 8
    assert view['note'] == '4 records of another quantity are not drawn on this stage'


def test_a_page_past_the_end_lands_on_the_last(random_run, monkeypatch):
    from hertzforge.drawing.viz import waterfall as wf

    lines = random_run.project['Time History PSDs'].abscissa.size
    monkeypatch.setattr(wf, 'SCENE_BUDGET', 3 * 2 * lines)
    (view,) = run('random.waterfall', random_run, of='psd', page=99)
    assert view['page'] == {'number': 3, 'count': 3, 'records': 8}
    assert len(view['records']) == 2


# ---- modal_run.waterfall --------------------------------------------------------

@pytest.mark.parametrize('of', ('frf', 'coherence', 'time'))
def test_modal_run_waterfall_validates(modal_run, validate, of):
    (view,) = run('modal_run.waterfall', modal_run, of=of)
    wire_views([view], validate)
    assert len(view['records']) == view['offsets'].size - 1


def test_an_frf_waterfall_is_its_magnitude_on_a_log_axis_by_default(modal_run):
    (view,) = run('modal_run.waterfall', modal_run, of='frf')
    frf = modal_run.project['FRF']
    assert len(view['records']) == frf.num_records
    assert view['z'] == {'label': 'FRF', 'unit': '(m/s²)/N', 'scale': 'log'}
    i0, i1 = int(view['offsets'][0]), int(view['offsets'][1])
    drawn = view['z_values'][i0:i1]
    assert np.nanmax(drawn) == pytest.approx(float(np.abs(frf.ordinate[0]).max()), rel=1e-6)


def test_an_frf_phase_reads_linear_in_degrees(modal_run):
    (view,) = run('modal_run.waterfall', modal_run, of='frf', component='phase')
    assert view['z'] == {'label': 'FRF phase', 'unit': 'deg', 'scale': 'linear'}
    finite = view['z_values'][np.isfinite(view['z_values'])]
    assert finite.min() >= -180.0 and finite.max() <= 180.0
    (real,) = run('modal_run.waterfall', modal_run, of='frf', component='real')
    assert real['z']['scale'] == 'linear' and real['z']['label'] == 'FRF, real part'


def test_a_component_does_nothing_to_real_data(modal_run):
    (plain,) = run('modal_run.waterfall', modal_run, of='coherence')
    (phased,) = run('modal_run.waterfall', modal_run, of='coherence', component='phase')
    assert np.array_equal(plain['z_values'], phased['z_values'], equal_nan=True)
    assert plain['z'] == phased['z'] and plain['z_range'] == [0.0, 1.05]


def test_the_modal_run_says_its_values_are_simulated(modal_run):
    assert modal_run.summary()['source_of_values'] == 'SIM'


# ---- modal_run.modes ------------------------------------------------------------

def test_identified_modes_validate_and_match_the_plate(modal_run, validate):
    """The plate project's own worked fit (300 to 1300 Hz, five modes): the
    four distinct modes and the repeated pair at 1142 Hz, on the ten survey
    nodes, moving out of plane only."""
    (view,) = run('modal_run.modes', modal_run)
    wire_views([view], validate)
    frequencies = [m['frequency_hz'] for m in view['modes']]
    np.testing.assert_allclose(frequencies, [439.2, 647.2, 823.5, 1142.3, 1142.4],
                               atol=0.15)
    assert all(0.0 < m['damping'] < 0.05 for m in view['modes'])
    assert view['nodes'].shape == (10, 3) and len(view['lines'])
    shape = view['modes'][0]['shape']
    assert np.abs(shape[:, :2]).max() < 1e-9 * np.abs(shape[:, 2]).max()


def test_identified_modes_are_fitted_once_per_band(modal_run, monkeypatch):
    run('modal_run.modes', modal_run)
    calls = []
    monkeypatch.setattr(modal_run.project, 'fit_modes',
                        lambda *a, **k: calls.append(1))
    run('modal_run.modes', modal_run)
    assert calls == []
    assert 'Identified Modes' not in modal_run.project


def test_an_empty_band_is_refused(modal_run):
    with pytest.raises(ProtocolError, match='empty'):
        run('modal_run.modes', modal_run, low_hz=900.0, high_hz=800.0)


def test_a_band_above_the_data_is_refused(modal_run):
    with pytest.raises(ProtocolError, match='no mode found between 5000 and 6000 Hz'):
        run('modal_run.modes', modal_run, low_hz=5000.0, high_hz=6000.0)


# ---- random.psd_vs_spec ---------------------------------------------------------

def test_psd_vs_spec_validates(random_run, validate):
    views = run('random.psd_vs_spec', random_run)
    wire_views(views, validate)
    assert [view['view'] for view in views] == ['spectrum', 'reading']
    assert views[0]['link'] == 'frequency'


def test_psd_vs_spec_reads_what_compliance_compare_reads(random_run):
    from hertzforge.core import compliance

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
    from hertzforge.drawing.plot import bin_edges, drawing_shape

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
    from hertzforge.core import compliance

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
    _wire, arrays = wire_views(views, validate)
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
    from hertzforge.deform import node_displacements

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
