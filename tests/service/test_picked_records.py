"""An action reads the records picked under each object in the tree, every record of an
object picked whole: the verbs that derive an object, the comparisons, the readings and Save
As. An action that takes an object whole says why (`action_table.WHOLE`)."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.core.data import Psd, Specification
from hertzforge.service import action_table, projects
from hertzforge.service.ops_project import DATA_KINDS
from hertzforge.service.protocol import ProtocolError
from hertzforge.service.server import OPERATIONS
from hertzforge.service.show import _run

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
#: the kinds whose items are picked in the tree by index
PICKABLE = {*DATA_KINDS, 'shape_set', 'channel_table', 'sine_sweep_specification',
            'sine_level_set', 'matched_modes'}


@pytest.fixture
def state():
    held, _ = projects.import_file(None, {'path': str(PLATE / 'random.nc4')})
    held.project.compute_psds('Time History')
    return held


def row(state, op, *objects, records=None):
    out = projects.actions(state, {'objects': list(objects), 'records': records or {},
                                   'part': None})
    return next(r for column in out['table']['columns'] for r in column['rows']
                if r['id'] == op)


def rows_of(reading):
    return {r['label']: r for r in reading['rows']}


def test_a_kept_copy_holds_the_records_picked_and_leaves_the_object_whole():
    f = np.linspace(20, 2000, 50)
    spec = Specification(f, np.vstack([np.full(50, 0.01 * (k + 1)) for k in range(3)]),
                         response_dof=['1Z+', '2Z+', '3Z+'], ordinate_unit=['g**2/Hz'] * 3)
    spec.limits['abort_high'] = np.asarray(spec.ordinate) * 4
    kept = spec.kept([2, 0, 2])
    assert kept.response_dof == ['1Z+', '3Z+'] and np.allclose(kept.ordinate[1], 0.03)
    assert kept.limits['abort_high'].shape == (2, 50)
    # the original keeps every record and every limit row
    assert spec.num_records == 3 and spec.limits['abort_high'].shape == (3, 50)
    assert spec.kept(None) is spec
    with pytest.raises(ValueError, match='no record is picked'):
        spec.kept([])
    with pytest.raises(IndexError, match='Specification has 3 records; no record 4'):
        spec.kept([3])


def test_a_derived_object_is_made_from_the_records_picked(state):
    project = state.project
    filled = row(state, 'project.compute_psds', 'Time History',
                 records={'Time History': [0, 2]})
    assert filled['fill']['records'] == '1, 3' and 'records' in filled['omit']
    _run(project, 'project.compute_psds', history='Time History', records='1, 3')
    name = [n for n in project if n.startswith('Time History PSDs')][-1]
    history = project['Time History']
    assert project[name].response_dof == [history.response_dof[0], history.response_dof[2]]
    # a refresh reads the same records
    assert project.provenance[name]['params']['records'] == [0, 2]
    _run(project, 'project.integrate', source='Time History', records='2')
    integrated = project[[n for n in project if 'Integrated' in n or 'Velocity' in n][-1]]
    assert integrated.num_records == 1


def test_a_comparison_reads_the_records_picked_under_each(state):
    project = state.project
    filled = row(state, 'project.spec_rms_error', 'Time History PSDs', 'Specification',
                 records={'Time History PSDs': [0, 1]})
    assert (filled['fill']['spectrum_records'], filled['fill']['specification_records']) == (
        '1-2', '')
    (whole,) = _run(project, 'project.spec_rms_error', spectrum='Time History PSDs',
                    specification='Specification')
    (picked,) = _run(project, 'project.spec_rms_error', spectrum='Time History PSDs',
                     specification='Specification', spectrum_records='1-2')
    assert rows_of(whole)['Channels out']['value'].split(' channels')[0].endswith('of 8')
    assert rows_of(picked)['Channels out']['value'].split(' channels')[0].endswith('of 2')
    # picks under the specification narrow it the same way
    (spec_picked,) = _run(project, 'project.spec_rms_error', spectrum='Time History PSDs',
                          specification='Specification', specification_records='1')
    assert rows_of(spec_picked)['Channels out']['value'].split(' channels')[0].endswith('of 1')


def test_a_reading_of_one_record_takes_it_from_the_object_it_is_read_from(state):
    """Spec Chance reads one record of the measured PSD: picks under the specification do
    not number it."""
    filled = row(state, 'project.spec_chance', 'Time History PSDs', 'Specification',
                 records={'Specification': [5]})
    assert filled['fill'].get('record', 1) in (1, '1')
    filled = row(state, 'project.spec_chance', 'Time History PSDs', 'Specification',
                 records={'Time History PSDs': [3]})
    assert filled['fill']['record'] == '4'


def test_kurtosis_and_signal_to_noise_read_the_records_picked(state):
    project = state.project
    (whole,) = _run(project, 'project.kurtosis', history='Time History')
    (picked,) = _run(project, 'project.kurtosis', history='Time History', history_records='1-3')
    assert rows_of(picked)['Outside the band']['value'].split(' channels')[0].endswith('of 3')
    assert rows_of(whole)['Outside the band']['value'].split(' channels')[0].endswith('of 12')
    psd = project['Time History PSDs']
    project.add('Noise', Psd(psd.abscissa, psd.ordinate * 0.01, response_dof=psd.response_dof,
                             ordinate_dim=psd.ordinate_dim))
    _plot, reading = _run(project, 'project.signal_to_noise', first='Time History PSDs',
                          second='Noise', first_records='1-2')
    assert rows_of(reading)['Channels']['value'] == 2.0


def test_save_as_writes_the_records_picked(state, tmp_path):
    project = state.project
    filled = row(state, 'project.save_as', 'Time History PSDs',
                 records={'Time History PSDs': [1, 4]})
    assert filled['fill']['object_records'] == '2, 5'
    path = tmp_path / 'two.hforge'
    _run(project, 'project.save_as', object='Time History PSDs', format='hforge',
         path=str(path), object_records='2, 5')
    opened, _ = projects.import_file(None, {'path': str(path)})
    (written,) = [obj for _name, obj in opened.project.items()]
    psd = project['Time History PSDs']
    assert written.response_dof == [psd.response_dof[1], psd.response_dof[4]]


def test_calibration_and_conditioning_read_the_rows_and_records_picked(state):
    from hertzforge.core.calibration import check, span_of

    project = state.project
    table = project['Channel Table']
    span = span_of('2026-01-01', None)
    assert [c.row for c in check(table, span, [3, 1]).channels] == [1, 3]
    assert len(check(table, span).channels) == table.num_channels
    filled = row(state, 'project.calibration_check', 'Channel Table',
                 records={'Channel Table': [0]})
    assert filled['fill']['rows'] == '1'


def test_a_derived_object_refuses_a_record_the_source_does_not_have(state):
    with pytest.raises(ProtocolError, match='13'):
        _run(state.project, 'project.compute_psds', history='Time History', records='13')


def _pickable_verbs():
    """The verbs that apply to an object whose items are picked: a time history, a PSD, an
    FRF and a shape set."""
    from hertzforge.core.shapes import ShapeSet

    held, _ = projects.import_file(None, {'path': str(PLATE / 'modal.nc4')})
    project = held.project
    project.compute_psds('Time History')
    project.compute_frfs('Time History')
    project.add('Modes', ShapeSet([10.0], [0.01], ['1Z+'], [[1.0]]))
    return {verb for name in project
            if projects.kind_of(project[name]) in PICKABLE
            for verb in projects.verbs_of(project, name)}


def by_id_of():
    return {op.id: op for op in OPERATIONS}


def test_every_action_reads_what_is_picked_or_says_why_not():
    """The rule, enforced: an action on an object whose items are picked in the tree reads
    the picks under every such object it takes (`PICKED`, a `<param>_records` param, or the
    one object's `records`, `channels` or `rows`), or is listed with the reason it takes the
    object whole (`WHOLE`, `WHOLE_OBJECTS`)."""
    verbs = _pickable_verbs()
    generic = action_table.PICKED_LISTS | action_table.PICKED_ONE | {'picked'}
    unread = []
    for op in action_table._rows_of(OPERATIONS):
        if op.id in action_table.WHOLE:
            continue
        names = {p.name for p in op.params}
        explicit = action_table.PICKED.get(op.id, {})
        whole = action_table.WHOLE_OBJECTS.get(op.id, {})
        pickable = [p.name for p in op.params
                    if (set(p.kinds) & PICKABLE) or (p.verb and p.verb in verbs)]
        missed = [name for name in pickable if name not in explicit.values()
                  and f'{name}_records' not in names and name not in whole]
        if missed and not (len(pickable) == 1 and names & generic):
            unread.append((op.id, missed))
    assert unread == []
    assert not set(action_table.PICKED) & set(action_table.WHOLE)
    for op_id, objects in action_table.WHOLE_OBJECTS.items():
        names = {p.name for p in by_id_of()[op_id].params}
        assert set(objects) <= names, op_id
    by_id = {op.id: op for op in OPERATIONS}
    for op_id, fills in action_table.PICKED.items():
        names = {p.name for p in by_id[op_id].params}
        assert set(fills) <= names and set(fills.values()) <= names, op_id


@pytest.mark.parametrize('verb, source, given', [
    ('compute_spectra', 'Time History', {}),
    ('compute_cpsds', 'Time History', {}),
    ('filter_data', 'Time History', {}),
    ('resample', 'Time History', {'rate': 1024.0}),
    ('integrate', 'Time History', {}),
    ('compute_octave', 'Time History PSDs', {}),
])
def test_each_verb_derives_from_the_records_picked(state, verb, source, given):
    """Two channels picked: the derived object holds only theirs, and its recipe keeps
    them."""
    project = state.project
    data = project[source]
    picked = [data.response_dof[1], data.response_dof[3]]
    before = set(project.names)
    _run(project, f'project.{verb}', source=source, records='2, 4', **given)
    (added,) = set(project.names) - before
    assert set(project[added].response_dof) == set(picked)
    assert project.provenance[added]['params']['records'] == [1, 3]


def test_drive_points_and_conditioning_read_the_records_picked(state):
    project = state.project
    project.compute_cpsds('Time History')
    cpsd = next(name for name in project if name.endswith('CPSDs'))
    (whole,) = _run(project, 'project.drive_points', data=cpsd)
    (picked,) = _run(project, 'project.drive_points', data=cpsd, records='1')
    assert len(whole['series']) > 1 and len(picked['series']) == 1
    data = project[cpsd]
    history = project['Time History']
    channels = [dof for dof, dim in zip(history.response_dof, history.ordinate_dim,
                                        strict=True) if dim == 'acceleration'][:2]
    square = [i for i in range(data.num_records)
              if data.response_dof[i] in channels and data.reference_dof[i] in channels
              and data.ordinate_dim[i] == 'acceleration**2/frequency']
    *_plots, reading = _run(project, 'project.conditioning', data=cpsd,
                            records=', '.join(str(i + 1) for i in square))
    assert rows_of(reading)['Effective rank']['value'].endswith('of 2')


def test_rename_dof_renames_the_channel_picked(state):
    project = state.project
    psd = project['Time History PSDs']
    old = psd.response_dof[2]
    at_old = psd.response_dof.count(old)
    _run(project, 'project.rename_dof', source='Time History PSDs', old='', new='999Z+',
         measured='', source_records='3')
    # the channel picked moves, and another quantity's channel at that DOF stays
    assert psd.response_dof[2] == '999Z+' and psd.response_dof.count(old) == at_old - 1
    with pytest.raises(ProtocolError, match='one DOF is renamed at a time'):
        _run(project, 'project.rename_dof', source='Time History PSDs', old='', new='998Z+',
             measured='', source_records='1-2')


def test_a_signal_is_generated_at_the_dofs_picked(state):
    project = state.project
    spec = project['Specification']
    before = set(project.names)
    _run(project, 'project.generate_signal', source='Specification', kind='random',
         duration=1.0, source_records='1')
    (added,) = set(project.names) - before
    assert set(project[added].response_dof) == {spec.response_dof[0]}


def _two_channel_sweep():
    from hertzforge import Project
    from hertzforge.core.data import TimeHistory
    from hertzforge.core.sine import SineSweepSpecification, SineTone

    rate = 4096.0
    tone = SineTone('Up', 0.5, [100.0, 500.0], [[2.0, 2.0]] * 2, [0], [100.0])
    n = int(6 * rate)
    argument = tone.argument(1.0 / rate)
    start = int(0.5 * rate)
    m = min(len(argument), n - start)
    x = np.zeros((2, n))
    x[0, start:start + m] = 2.0 * np.cos(argument[:m])
    x[1, start:start + m] = 1.0 * np.cos(argument[:m])
    project = Project()
    project.add('Sweep', TimeHistory(np.arange(n) / rate, x, response_dof=['101Z+', '102Z+'],
                                     ordinate_dim=['acceleration'] * 2,
                                     ordinate_unit=['m/s**2'] * 2))
    project.add('Profile', SineSweepSpecification([tone], ['101Z+', '102Z+'],
                                                  ordinate_unit='m/s**2'))
    project.extract_sine('Sweep')
    return project


def test_level_error_and_the_profile_read_the_control_channels_picked():
    project = _two_channel_sweep()
    levels = next(name for name in project if name.startswith('Sine Levels'))
    _plot, whole = _run(project, 'project.level_error', specification='Profile', levels=levels)
    _plot, picked = _run(project, 'project.level_error', specification='Profile',
                         levels=levels, levels_records='2')
    assert {'101Z+', '102Z+'} <= set(rows_of(whole))
    assert '102Z+' in rows_of(picked) and '101Z+' not in rows_of(picked)
    with pytest.raises(ProtocolError, match='a profile is read at one control channel'):
        _run(project, 'project.sine_profile', specification='Profile',
             specification_records='1-2')
