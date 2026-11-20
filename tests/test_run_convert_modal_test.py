"""A Modal Test run opens in the analysis side (`io.modal_run`): its FRFs, coherence and
reference autospectra become analysis objects at the DOFs each measurement named, a roving
hammer's FRFs are turned round by reciprocity as well, each measurement's accepted frames
become a time history, and the frame lines and outcome stay in the run record. The folders
are synthetic (`run_folders.modal_test_run`), written to the wire shapes `modal_run.WIRE`
assumes."""

from __future__ import annotations

import os

import numpy as np
import pytest
import run_folders as rf

from hertzforge import Project
from hertzforge.core.data import Coherence, Frf, MultipleCoherence, Psd, TimeHistory
from hertzforge.core.modal_test import TRANSPOSED, frf_tau, is_transposed
from hertzforge.core.run_record import RunRecord
from hertzforge.core.test_setup import TestSetup
from hertzforge.io import modal_run, run_convert

LINES = np.arange(rf.MODAL_LINES) * rf.MODAL_STEP


def _open(tmp_path, layout='roving_hammer', name='run', **kwargs):
    folder = rf.modal_test_run(str(tmp_path / name), layout, **kwargs)
    done = run_convert.convert(folder)
    return done, Project.open(done.path)


def _record(project) -> RunRecord:
    (record,) = [project[n] for n in project if isinstance(project[n], RunRecord)]
    return record


def test_modal_test_fixed_hammer_frfs_are_at_each_dof_in_si(tmp_path):
    done, project = _open(tmp_path, 'fixed_hammer')
    frf = project['modal_frf']
    assert isinstance(frf, Frf)
    assert frf.response_dof == [f'{k}Z+' for k in range(1, 7)]
    assert frf.reference_dof == ['1Z+'] * 6
    assert frf.ordinate_dim == ['acceleration/force'] * 6
    np.testing.assert_allclose(frf.abscissa, LINES)
    for k in range(6):
        np.testing.assert_allclose(frf.ordinate[k], rf.modal_accelerance(LINES, k, 0),
                                   rtol=1e-12, atol=1e-12)
    assert frf.comment[0].startswith('H1 from 1 reference, 3 averages; measurement 1;')
    assert 'force window 5 % of the block (204.8 samples)' in frf.comment[0]
    assert isinstance(project['modal_coherence'], Coherence)
    assert project['modal_coherence'].reference_dof == ['1Z+'] * 6
    spectra = project['modal_reference_spectra']
    assert isinstance(spectra, Psd) and spectra.response_dof == ['1Z+']
    assert frf.source == 'SIM'
    assert 'modal_frf fixed reference' not in project
    assert done.lacked == []


def test_modal_test_frames_are_each_measurements_accepted_hits(tmp_path):
    _done, project = _open(tmp_path, 'fixed_hammer', hits=3)
    history = project['Measurement 1 frames']
    assert isinstance(history, TimeHistory)
    # seven channels, three accepted hits each: the trial and the double hit are not copied
    assert history.ordinate.shape == (21, rf.MODAL_BLOCK)
    assert history.block[:3] == ['hit 1', 'hit 2', 'hit 3']
    # timed from the trigger: the pre-trigger samples at negative times
    assert history.abscissa[64] == 0.0 and history.abscissa[0] < 0.0
    assert history.channel_roles()[('1Z+', 'force')] == 'reference'
    assert history.channel_roles()[('6Z+', 'acceleration')] == 'response'
    hammer = [i for i, d in enumerate(history.response_dof) if d == '1Z+'
              and history.ordinate_dim[i] == 'force']
    assert history.ordinate[hammer[0], 64:69].max() > 0
    record = _record(project)
    lines = record.results['modal_frames']['records']
    assert [line['reason'] for line in lines] == ['trial hit', '', 'double hit', '', '']


def test_modal_test_roving_hammer_is_turned_round_and_the_later_set_wins(tmp_path):
    _done, project = _open(tmp_path, again=[2])
    frf = project['modal_frf']
    # as measured: one response, the hammer at every point, point 3 once
    assert frf.response_dof == ['1Z+'] * 6
    assert frf.reference_dof == [f'{k}Z+' for k in (1, 2, 4, 5, 6, 3)]
    # the later measurement of point 3 (written doubled) is the one held
    np.testing.assert_allclose(frf.ordinate[5], 2 * rf.modal_accelerance(LINES, 0, 2),
                               rtol=1e-12, atol=1e-12)
    assert 'measurement 7, point 3' in frf.comment[5]
    turned = project['modal_frf fixed reference']
    assert is_transposed(turned)
    assert turned.response_dof == [f'{k}Z+' for k in (1, 2, 4, 5, 6, 3)]
    assert turned.reference_dof == ['1Z+'] * 6
    assert turned.comment[1].startswith(f'{TRANSPOSED}: measured as 1Z+ against 2Z+; H1')
    # reciprocity: the turned record of 4Z+ for a force at 1Z+ is the structure's own
    np.testing.assert_allclose(turned.ordinate[2], rf.modal_accelerance(LINES, 3, 0),
                               rtol=1e-12, atol=1e-12)
    coherence = project['modal_coherence fixed reference']
    assert isinstance(coherence, Coherence) and coherence.reference_dof == ['1Z+'] * 6
    # the replaced measurement's frames are kept, and named so
    assert 'Measurement 3 frames (point 3) (replaced)' in project
    history = project['Measurement 7 frames (point 3)']
    assert set(history.response_dof) == {'1Z+', '3Z+'}


def test_modal_test_unmeasured_points_are_said_and_a_killed_run_converts(tmp_path):
    done, project = _open(tmp_path, measured=[0, 1, 3], complete=False)
    assert any(line == '3 of the 6 points planned were not measured: point 3, point 5, '
                       'point 6' for line in done.lacked)
    frf = project['modal_frf']
    assert frf.reference_dof == ['1Z+', '2Z+', '4Z+']
    assert 'modal_frf fixed reference' in project
    record = _record(project)
    assert record.end is None
    assert 'modal_outcome' not in record.results


def test_modal_test_run_killed_before_a_measurement_converts_with_what_it_has(tmp_path):
    done, project = _open(tmp_path, measured=[], complete=False)
    assert 'modal_frf' not in project
    assert 'the run finished no measurement, so it has no FRFs' in done.lacked
    assert isinstance(_record(project), RunRecord)


def test_modal_test_two_shakers_carry_multiple_coherence(tmp_path):
    done, project = _open(tmp_path, 'two_shakers')
    frf = project['modal_frf']
    assert frf.num_records == 12
    assert sorted(set(frf.reference_dof)) == ['1Z+', '4Z+']
    np.testing.assert_allclose(frf.ordinate[frf.response_dof.index('5Z+') + 1],
                               rf.modal_accelerance(LINES, 4, 3), rtol=1e-12, atol=1e-12)
    assert frf.comment[0].startswith('H1 from 2 references, 40 averages')
    coherence = project['modal_coherence']
    assert isinstance(coherence, MultipleCoherence)
    assert coherence.response_dof == [f'{k}Z+' for k in range(1, 7)]
    assert coherence.comment[0].startswith('multiple coherence')
    spectra = project['modal_reference_spectra']
    assert spectra.response_dof == ['1Z+', '4Z+']
    record = _record(project)
    assert record.arrays['modal_reference_spectra']['condition'].shape == (1, rf.MODAL_LINES)
    # a shaker averages: the window every channel shares is on the FRFs
    assert frf.averaging.window == 'hann' and frf.averaging.overlap == 0.5
    assert 'modal_frf fixed reference' not in project
    assert done.lacked == []


def test_modal_test_window_and_comments_survive_a_save_and_reopen(tmp_path):
    _done, project = _open(tmp_path, 'fixed_hammer', tau_s=0.5)
    frf = project['modal_frf']
    assert frf.averaging.window == 'exponential'
    assert frf.averaging.frame_length == rf.MODAL_BLOCK and frf.averaging.frames == 3
    assert frf_tau(frf) == pytest.approx(0.5, rel=1e-9)
    assert 'tau 0.5 s (adds a decay rate of 2 1/s)' in frf.comment[0]
    saved = str(tmp_path / 'again.hforge')
    project.save(saved)
    again = Project.open(saved)['modal_frf']
    assert frf_tau(again) == pytest.approx(0.5, rel=1e-9)
    assert again.comment == frf.comment
    assert again.averaging == frf.averaging


def test_modal_test_values_not_finite_are_said_and_kept(tmp_path):
    folder = rf.modal_test_run(str(tmp_path / 'run'), 'fixed_hammer')
    path = os.path.join(folder, 'results', 'modal_frf.bin')
    with open(path, 'r+b') as f:
        f.seek(16 * 10)
        f.write(np.array([np.nan + 0j], dtype='<c16').tobytes())
    done = run_convert.convert(folder)
    assert ('the modal_frf record of 1Z+ against 1Z+ in measurement 1 is not a finite '
            'number at 1 of its 601 lines') in done.lacked
    frf = Project.open(done.path)['modal_frf']
    assert np.isnan(frf.ordinate[0, 10]) and np.isfinite(frf.ordinate[1]).all()


def test_modal_test_a_unit_unlike_the_tables_keeps_the_result_as_arrays(tmp_path):
    folder = rf.modal_test_run(str(tmp_path / 'run'), 'fixed_hammer')
    path = os.path.join(folder, 'results', 'modal_frf.jsonl')
    with open(path, encoding='utf-8') as f:
        text = f.read()
    with open(path, 'w', encoding='utf-8') as f:
        f.write(text.replace('"unit": "N"', '"unit": "lbf"'))
    done = run_convert.convert(folder)
    assert any(line.startswith("the result 'modal_frf' is kept as arrays in the run record: "
                               "it gives channel 7 the unit 'lbf' and the table 'N'")
               for line in done.lacked)
    project = Project.open(done.path)
    assert 'modal_frf' not in project
    assert _record(project).arrays['modal_frf']['values'].shape == (1, 6, 1, rf.MODAL_LINES)


def test_modal_test_the_setup_it_ran_opens_from_the_run(tmp_path):
    _done, project = _open(tmp_path)
    setup = TestSetup.from_run(_record(project))
    assert setup.module == 'modal_test'
    method, params = setup.configure
    assert method == 'modal_test.configure'
    assert params['config']['block'] == rf.MODAL_BLOCK
    assert project.project_type == 'Modal Test'


def test_modal_test_the_same_folder_converts_to_the_same_bytes(tmp_path):
    folder = rf.modal_test_run(str(tmp_path / 'run'), again=[1])
    first = run_convert.convert(folder, str(tmp_path / 'a.hforge')).path
    second = run_convert.convert(folder, str(tmp_path / 'b.hforge')).path
    with open(first, 'rb') as a, open(second, 'rb') as b:
        assert a.read() == b.read()


def test_modal_test_a_report_names_the_test_and_every_wire_name_is_in_one_table():
    from hertzforge.report.run_records import TEST_NAMES

    assert TEST_NAMES['modal_test'] == 'Modal Test'
    assert modal_run.WIRE['test'] == 'modal_test'
    assert run_convert.PROJECT_TYPES['modal_test'] == 'Modal Test'
