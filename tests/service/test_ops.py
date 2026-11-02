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


def test_the_largest_quantity_is_drawn_and_the_others_are_offered(random_run, validate):
    """The PSDs hold eight accelerations and four forces: the accelerations
    are drawn and both are listed for the view to choose between."""
    (view,) = run('random.waterfall', random_run, of='psd')
    wire_views([view], validate)
    assert len(view['records']) == 8 and 'note' not in view
    assert view['quantities'] == [
        {'value': 'acceleration**2/frequency|', 'label': 'Acceleration', 'records': 8},
        {'value': 'force**2/frequency|', 'label': 'Force', 'records': 4}]
    assert view['quantity'] == 'acceleration**2/frequency|'


def test_the_quantity_chosen_is_the_one_drawn(random_run):
    (view,) = run('random.waterfall', random_run, of='psd', quantity='force**2/frequency|')
    assert len(view['records']) == 4 and view['z']['unit'] == 'N²/Hz'
    assert view['quantity'] == 'force**2/frequency|'
    psds = random_run.project['Time History PSDs']
    forces = [i for i in range(psds.num_records) if psds.ordinate_dim[i] == 'force**2/frequency']
    assert view['records'] == [psds.record_label(i) for i in forces]


def test_a_quantity_the_object_lacks_draws_the_largest(random_run):
    """A choice kept from another object is not an error about this one."""
    (view,) = run('random.waterfall', random_run, of='psd', quantity='voltage|')
    assert view['quantity'] == 'acceleration**2/frequency|'


def test_an_object_of_one_quantity_offers_no_choice(modal_run):
    (view,) = run('modal_run.waterfall', modal_run)
    assert 'quantities' not in view and 'quantity' not in view


def test_the_quantity_param_is_set_in_the_view_not_the_panel():
    waterfalls = [op.to_json() for op in OPERATIONS if op.outputs == ('waterfall',)]
    assert len(waterfalls) == 2
    for op in waterfalls:
        (param,) = [p for p in op['params'] if p['name'] == 'quantity']
        assert param['in_view'] is True and param['default'] == ''


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
    with pytest.raises(ProtocolError, match=r"the bounds 5000 to 6000 Hz hold none of FRF's "
                       r'lines, which span 0 to 2048 Hz'):
        run('modal_run.modes', modal_run, low_hz=5000.0, high_hz=6000.0)


def test_identified_modes_are_real(modal_run):
    """The fitter fits real normal modes, so no imaginary part travels."""
    (view,) = run('modal_run.modes', modal_run)
    assert not any('shape_imag' in m for m in view['modes'])


# ---- modal_run.ods --------------------------------------------------------------

def test_an_operating_shape_is_the_frfs_at_one_line(modal_run, validate):
    """At the line nearest the asked frequency, each node's response is its FRF
    to that reference, scaled so the largest is 1 and rotated by one common
    phase: the ratios between nodes are the FRFs' own."""
    (view,) = run('modal_run.ods', modal_run, frequency_hz=441.0, reference=2)
    wire_views([view], validate)
    (mode,) = view['modes']
    frf = modal_run.project['FRF']
    lines = np.asarray(frf.abscissa)
    line = int(np.argmin(np.abs(lines - 441.0)))
    assert mode['frequency_hz'] == lines[line] and mode['damping'] is None
    assert mode['label'] == 'Operating shape'
    assert view['title'].endswith(f'Operating Shape at {lines[line]:g} Hz, Reference 1310Z+')
    shape = mode['shape'] + 1j * mode['shape_imag']
    assert np.abs(shape[:, :2]).max() < 1e-9 * np.abs(shape[:, 2]).max()
    from hertzforge.deform import dof_directions
    geometry = modal_run.project['Geometry']
    picked = [k for k, dof in enumerate(frf.reference_dof) if dof == '1310Z+']
    rows, _, used = dof_directions(geometry, [frf.response_dof[k] for k in picked])
    measured = np.asarray(frf.ordinate)[picked][used, line]
    xyz = np.asarray(geometry.node_xyz)
    ratios = []
    for row in np.unique(rows):
        if np.count_nonzero(rows == row) > 1:
            continue   # a node measured twice draws the sum of its records
        (drawn,) = np.flatnonzero(np.all(np.isclose(view['nodes'], xyz[row]), axis=1))
        ratios.append(shape[drawn, 2] / measured[rows == row][0])
    assert len(ratios) >= 8
    np.testing.assert_allclose(ratios, ratios[0], rtol=1e-4)
    assert np.abs(shape[:, 2]).max() == pytest.approx(1.0, abs=1e-6)


def test_an_operating_shape_off_resonance_is_complex(modal_run):
    """Between modes the FRFs are not in phase, so the shape has an imaginary
    part that no common rotation removes."""
    (view,) = run('modal_run.ods', modal_run, frequency_hz=550.0)
    assert np.abs(view['modes'][0]['shape_imag']).max() > 1e-3


def test_an_operating_shape_outside_the_frfs_is_refused(modal_run):
    with pytest.raises(ProtocolError, match='5000 Hz is outside the FRFs, which span 0 to 2048 Hz'):
        run('modal_run.ods', modal_run, frequency_hz=5000.0)


def test_a_reference_the_survey_does_not_have_is_refused(modal_run):
    with pytest.raises(ProtocolError, match=r'has 2 references, 101Z\+, 1310Z\+; there is no '
                       'reference 3'):
        run('modal_run.ods', modal_run, reference=3)


# ---- modal_run.mac --------------------------------------------------------------

def test_the_mac_against_the_model_pairs_each_mode_with_its_own(modal_run, validate):
    """The survey was simulated from the plate FE model, so each identified
    mode's MAC with the model mode at its frequency is near 1 and the rest
    near 0. The repeated pair at 1142 Hz is any rotation within its plane, so
    the fit's pair and the model's agree less, and are not held to 0.99."""
    (view,) = run('modal_run.mac', modal_run)
    wire_views([view], validate)
    values = view['values']
    assert values.shape == (5, 5) and values.dtype == np.float32
    assert view['rows']['ticks'] == ['439.2 Hz', '647.2 Hz', '823.5 Hz', '1142.3 Hz',
                                     '1142.4 Hz']
    assert view['columns']['label'] == 'FE model' and len(view['columns']['ticks']) == 5
    assert view['value_range'] == [0.0, 1.0]
    assert np.diag(values)[:3].min() > 0.99
    pair = values[3:, 3:]
    assert pair.sum(axis=1).min() > 0.95
    off = values[:3].copy()
    off[np.arange(3), np.arange(3)] = 0.0
    assert off.max() < 0.02


def test_the_mac_against_the_model_is_the_core_cross_mac(modal_run):
    """The service only picks the columns: its values are `cross_mac`'s."""
    from hertzforge.core.shapes import cross_mac
    (view,) = run('modal_run.mac', modal_run)
    shapes = modal_run.fits[(300.0, 1300.0, 5)]
    model = modal_run.analysis_model().shapes
    frequency = np.asarray(model.frequency)
    keep = (frequency > 0) & (frequency >= 200.0) & (frequency <= 1950.0)
    np.testing.assert_allclose(view['values'], cross_mac(shapes, model)[:, keep], rtol=1e-6)


def test_the_auto_mac_has_a_unit_diagonal(modal_run, validate):
    (view,) = run('modal_run.mac', modal_run, against='self')
    wire_views([view], validate)
    assert view['rows'] == view['columns']
    np.testing.assert_allclose(np.diag(view['values']), 1.0, rtol=1e-6)
    np.testing.assert_allclose(view['values'], view['values'].T, rtol=1e-5)


def test_a_survey_with_no_model_is_refused_a_model_mac(modal_run):
    bare = dataclasses.replace(modal_run, model=None, _opened_model=[])
    with pytest.raises(ProtocolError, match='has no FE model to compare against'):
        run('modal_run.mac', bare)
    (view,) = run('modal_run.mac', bare, against='self')
    assert view['view'] == 'matrix'


def test_an_empty_band_is_refused_a_mac(modal_run):
    with pytest.raises(ProtocolError, match='empty'):
        run('modal_run.mac', modal_run, low_hz=900.0, high_hz=800.0)


# ---- modal_run.motion -----------------------------------------------------------

def test_a_capture_plays_back_its_accelerations(modal_run, validate):
    """Capture 3: the eleven accelerometers, one record each, their samples as
    the file has them; the two force gauges are left out and said so."""
    (view,) = run('modal_run.motion', modal_run, capture=3)
    wire_views([view], validate)
    assert view['modes'] == [] and view['title'].endswith('Capture 3 of 20, Acceleration')
    assert view['note'] == '2 records of another quantity are not shown'
    motion = view['motion']
    data = modal_run.project['Time History']
    picked = [i for i in range(data.num_records)
              if data.block[i] == 'avg 3' and data.known_dim(i) == 'acceleration']
    assert motion['values'].shape == (11, 2048) and motion['values'].dtype == np.float32
    np.testing.assert_array_equal(motion['values'], np.asarray(data.ordinate)[picked].astype(np.float32))
    assert motion['quantity'] == 'Acceleration' and motion['value_unit'] == 'm/s²'
    assert motion['step'] == pytest.approx(1 / 4096) and motion['start'] == 0.0
    assert motion['node'].max() < len(view['nodes'])
    np.testing.assert_allclose(np.linalg.norm(motion['direction'], axis=1), 1.0, rtol=1e-6)


def test_each_dof_moves_the_node_at_its_own_position(modal_run):
    """The drawn node a DOF moves is the geometry node its name gives."""
    from hertzforge.deform import dof_directions
    (view,) = run('modal_run.motion', modal_run)
    data = modal_run.project['Time History']
    geometry = modal_run.project['Geometry']
    dofs = [data.response_dof[i] for i in range(data.num_records)
            if data.block[i] == 'avg 1' and data.known_dim(i) == 'acceleration']
    rows, _, _ = dof_directions(geometry, dofs)
    xyz = np.asarray(geometry.node_xyz)[rows]
    np.testing.assert_allclose(view['nodes'][view['motion']['node']], xyz, atol=1e-6)


def test_the_motion_peak_is_the_furthest_a_node_gets(modal_run):
    (view,) = run('modal_run.motion', modal_run)
    motion = view['motion']
    nodes = len(view['nodes'])
    furthest = 0.0
    for k in range(motion['values'].shape[1]):
        offsets = np.zeros((nodes, 3))
        np.add.at(offsets, motion['node'], motion['direction'] * motion['values'][:, k:k + 1])
        furthest = max(furthest, np.linalg.norm(offsets, axis=1).max())
    assert motion['peak'] == pytest.approx(furthest, rel=1e-5)


def test_a_capture_the_survey_does_not_have_is_refused(modal_run):
    with pytest.raises(ProtocolError, match='has 20 captures; there is no capture 21'):
        run('modal_run.motion', modal_run, capture=21)


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
