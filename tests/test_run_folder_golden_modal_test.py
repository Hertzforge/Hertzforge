"""The bridge from a Modal Test run to the analysis side, on folders the
control service wrote: the golden Modal Test run folders
(`schema/golden/runs/modal_test/impact_roving/` and
`schema/golden/runs/modal_test/shaker_burst_random/`, written by the
control service's own test `modal_test_golden_run` on the simulator)
convert with `hertzforge.io.run_convert` into projects that
`Project.open` opens, with the channel table that was set, the setup,
the FRFs at the DOFs each measurement named (a roving hammer's turned
round for the fit too), the coherence, the reference autospectra, each
measurement's hits as time data, and the run record with the frame
lines and the outcome; both sets of FRFs go to the modal fit through
the service; a copy cut off part way converts with what it holds; and
`docs/run-folder.md` lists exactly what a Modal Test run saves.

Each folder's streams are cut to their first 1024 frames
(`cut_from_frames`), so only the hits inside them have time data, and
the conversion says so: that is the one thing a converted golden folder
lacks."""

from __future__ import annotations

import json
import math
import os
import re
import shutil

import numpy as np
import pytest

from hertzforge import Project
from hertzforge.core.channel_table import ChannelTable
from hertzforge.core.data import Coherence, Frf, MultipleCoherence, Psd, TimeHistory
from hertzforge.core.modal_fit import fit_refusal
from hertzforge.core.modal_test import (
    TRANSPOSED,
    frf_tau,
    is_transposed,
    window_corrected,
)
from hertzforge.core.run_record import RunRecord
from hertzforge.io import modal_run, run_convert, run_folder
from hertzforge.service import fitting

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RUNS = os.path.join(ROOT, 'schema', 'golden', 'runs', 'modal_test')
IMPACT, SHAKER = 'impact_roving', 'shaker_burst_random'
G, LBF = 9.80665, 4.4482216152605005


def _json(name, *parts):
    with open(os.path.join(RUNS, name, *parts), encoding='utf-8') as f:
        return json.load(f)


def _lines(name, result):
    with open(os.path.join(RUNS, name, 'results', f'{result}.jsonl'), encoding='utf-8') as f:
        return [json.loads(line) for line in f if line.strip()]


def _dof(entry):
    return f'{entry["node"]}{entry["direction"]}'


def _cut_line(name, word):
    stream = _json(name, 'streams', 'inputs', 'stream.json')
    spans = run_folder.read(os.path.join(RUNS, name)).results['modal_frames'].arrays['span']
    past = sum(1 for at, length in spans if at + length > stream['frames'])
    return (f"the stream 'inputs' holds its first {stream['frames']} of "
            f"{stream['cut_from_frames']} frames, so {past} {word}s past them have no frames")


@pytest.fixture(scope='module')
def converted(tmp_path_factory):
    out = {}
    for name in (IMPACT, SHAKER):
        folder = str(tmp_path_factory.mktemp('runs') / name)
        shutil.copytree(os.path.join(RUNS, name), folder)
        done = run_convert.convert(folder)
        out[name] = (folder, done, Project.open(done.path))
    return out


def test_both_folders_convert_with_only_the_cut_streams_lacking(converted):
    _f, done, project = converted[IMPACT]
    assert done.lacked == [_cut_line(IMPACT, 'hit')]
    assert list(project) == done.objects == [
        'Channel Table', 'Measurement 1 frames (point 1)', 'modal_frf',
        'modal_frf fixed reference', 'modal_coherence', 'modal_coherence fixed reference',
        'modal_reference_spectra', 'Run Record']
    assert project.name == 'Golden Roving Hammer' and project.project_type == 'Modal Test'
    _f, done, project = converted[SHAKER]
    # every shaker frame lies past the cut, so the shaker run has no measurement frames
    assert done.lacked == [_cut_line(SHAKER, 'frame')]
    assert list(project) == done.objects == [
        'Channel Table', 'modal_frf', 'modal_coherence', 'modal_reference_spectra',
        'Run Record']
    assert project.name == 'Golden Two Shakers'


@pytest.mark.parametrize('name, references', [(IMPACT, [1]), (SHAKER, [1, 4])])
def test_the_channel_table_is_the_table_that_was_set(converted, name, references):
    _f, _done, project = converted[name]
    table = project['Channel Table']
    rows = _json(name, 'table.json')['rows']
    assert isinstance(table, ChannelTable)
    assert list(table['channel']) == [r['index'] for r in rows]
    assert list(table['unit']) == [r['units'] for r in rows]
    assert list(table['comment']) == [r['description'] for r in rows]
    inputs = [r for r in rows if r['role'] != 'drive']
    assert table.dof_strings()[:len(inputs)] == [_dof(r) for r in inputs]
    # the force gauges the setup names as its references are the table's references
    assert [r['index'] for r in _json(name, 'setup.json')['check']['references']] == references
    roles = dict(zip(table['channel'], table.roles(), strict=True))
    assert [i for i in references if roles[i] == 'reference'] == references
    assert all(roles[r['index']] == 'response' for r in inputs
               if r['analysis_role'] == 'response')
    assert all(roles[r['index']] == r['analysis_role'] for r in inputs)


def test_the_impact_frfs_are_each_measurements_at_its_dofs_in_si(converted):
    _f, _done, project = converted[IMPACT]
    frf = project['modal_frf']
    lines = _lines(IMPACT, 'modal_frf')
    raw = run_folder.read(os.path.join(RUNS, IMPACT)).results['modal_frf'].arrays['frf']
    assert isinstance(frf, Frf) and frf.source == 'SIM'
    want_resp = [_dof(r) for line in lines for r in line['rows']['responses']]
    want_ref = [_dof(line['rows']['references'][0]) for line in lines
                for _r in line['rows']['responses']]
    assert frf.response_dof == want_resp == ['4Z+', '7Z+'] * 3
    assert frf.reference_dof == want_ref == ['1Z+', '1Z+', '2Z+', '2Z+', '3Z+', '3Z+']
    assert frf.ordinate_dim == ['acceleration/force'] * 6
    # [response, reference, line], g per lbf, in SI
    np.testing.assert_allclose(frf.ordinate, raw[:, :, 0].reshape(6, -1) * G / LBF,
                               rtol=1e-12, atol=0)
    side = _json(IMPACT, 'results', 'modal_frf.json')
    axis = side['abscissa']
    np.testing.assert_allclose(frf.abscissa, axis['start'] + axis['step'] * np.arange(
        _json(IMPACT, 'setup.json')['check']['band']['lines']))
    windows = side['windows']
    assert windows == _json(IMPACT, 'setup.json')['resolved']['windows']
    force = windows['force']
    decay = lines[0]['decay']
    decayed = next(_dof(r) for r in lines[0]['rows']['responses'] if r['index'] == decay['row'])
    assert (decay['warning'], decay['row'], decayed) == (False, 3, '7Z+')
    assert frf.comment[0] == (
        f'H1 from 1 reference, 2 averages; measurement 1, point 1; decayed to '
        f"{decay['ratio'] * 100:.3g} % by the block's end at row 3 (7Z+); uniform analysis "
        'window; '
        f'force window on the references, {force["width_frames"]} samples after the trigger '
        f'at sample {force["pre_frames"]} and a fall of {force["fall_frames"]}; exponential '
        f'window on every channel, tau {windows["exponential"]["tau_s"]:.12g} s (adds a decay '
        f'rate of {windows["exponential"]["inv_tau_per_s"]:g} 1/s), ending at 10 %')
    assert frf_tau(frf) == pytest.approx(windows['exponential']['tau_s'], rel=1e-11)
    assert frf.averaging.frame_length == 512 and frf.averaging.frames == 2
    assert frf.averaging.window == 'boxcar'


def test_the_impact_frfs_take_the_fixed_reference_form(converted):
    _f, _done, project = converted[IMPACT]
    frf, turned = project['modal_frf'], project['modal_frf fixed reference']
    assert is_transposed(turned) and not is_transposed(frf)
    assert turned.response_dof == frf.reference_dof
    assert turned.reference_dof == frf.response_dof
    np.testing.assert_array_equal(turned.ordinate, frf.ordinate)
    assert turned.comment[0].startswith(f'{TRANSPOSED}: measured as 4Z+ against 1Z+; H1')
    coherence = project['modal_coherence fixed reference']
    assert isinstance(coherence, Coherence) and is_transposed(coherence)
    assert coherence.reference_dof == ['4Z+', '7Z+'] * 3


def test_the_coherence_is_the_kind_its_array_names(converted):
    _f, _done, project = converted[IMPACT]
    coherence = project['modal_coherence']
    raw = run_folder.read(os.path.join(RUNS, IMPACT)).results['modal_coherence']
    assert isinstance(coherence, Coherence) and list(raw.arrays) == ['coherence']
    assert coherence.response_dof == ['4Z+', '7Z+'] * 3
    assert coherence.reference_dof == ['1Z+', '1Z+', '2Z+', '2Z+', '3Z+', '3Z+']
    np.testing.assert_allclose(coherence.ordinate,
                               np.clip(raw.arrays['coherence'].reshape(6, -1), 0, 1))
    _f, _done, project = converted[SHAKER]
    coherence = project['modal_coherence']
    raw = run_folder.read(os.path.join(RUNS, SHAKER)).results['modal_coherence']
    assert isinstance(coherence, MultipleCoherence)
    assert list(raw.arrays) == ['multiple_coherence']
    assert coherence.response_dof == ['4Z+', '7Z+']
    np.testing.assert_allclose(coherence.ordinate,
                               np.clip(raw.arrays['multiple_coherence'][0], 0, 1))
    assert coherence.comment[0] == 'multiple coherence; measurement 1, point 1'


def test_the_shaker_frfs_are_matrix_h1_from_both_references(converted):
    _f, _done, project = converted[SHAKER]
    frf = project['modal_frf']
    raw = run_folder.read(os.path.join(RUNS, SHAKER)).results['modal_frf'].arrays['frf']
    assert frf.response_dof == ['4Z+', '4Z+', '7Z+', '7Z+']
    assert frf.reference_dof == ['1Z+', '8Z+', '1Z+', '8Z+']
    np.testing.assert_allclose(frf.ordinate, raw[0].reshape(4, -1) * G / LBF, rtol=1e-12)
    decay = _lines(SHAKER, 'modal_frf')[0]['decay']
    assert (decay['warning'], decay['row']) == (False, 2)
    assert frf.comment[0] == ('matrix H1 from 2 references, 4 averages; measurement 1, '
                              f"point 1; decayed to {decay['ratio'] * 100:.3g} % by the block's "
                              'end at row 2 (4Z+); uniform analysis window')
    assert frf_tau(frf) is None
    assert frf.averaging.frames == 4 and frf.averaging.window == 'boxcar'
    assert frf.averaging.overlap == 0.0
    assert 'modal_frf fixed reference' not in project


@pytest.mark.parametrize('name, references, measurements', [
    (IMPACT, ['1Z+', '2Z+', '3Z+'], 3), (SHAKER, ['1Z+', '8Z+'], 1)])
def test_the_reference_spectra_are_each_measurements_in_si(converted, name, references,
                                                           measurements):
    _f, _done, project = converted[name]
    spectra = project['modal_reference_spectra']
    raw = run_folder.read(os.path.join(RUNS, name)).results['modal_reference_spectra']
    assert isinstance(spectra, Psd) and spectra.response_dof == references
    assert spectra.ordinate_dim == ['force**2/frequency'] * len(references)
    np.testing.assert_allclose(spectra.ordinate,
                               raw.arrays['autospectra'].reshape(len(references), -1) * LBF ** 2,
                               rtol=1e-12)
    assert len(set(spectra.block)) == measurements
    record = project['Run Record']
    if name == SHAKER:
        np.testing.assert_array_equal(record.arrays['modal_reference_spectra']['condition_number'],
                                      raw.arrays['condition_number'])


def test_the_impact_hit_inside_the_stream_is_time_data_with_its_checks(converted):
    folder, _done, project = converted[IMPACT]
    history = project['Measurement 1 frames (point 1)']
    setup = _json(IMPACT, 'setup.json')
    pre = setup['check']['impact']['trigger']['pretrigger_frames']
    lines = _lines(IMPACT, 'modal_frames')
    span = run_folder.read(os.path.join(RUNS, IMPACT)).results['modal_frames'].arrays['span']
    at, length = (int(v) for v in span[0])
    assert isinstance(history, TimeHistory) and history.source == 'SIM'
    # the hammer at point 1 (node 1) and the two fixed accelerometers
    assert history.response_dof == ['1Z+', '4Z+', '7Z+']
    assert history.ordinate_dim == ['force', 'acceleration', 'acceleration']
    assert history.block == ['hit 1'] * 3
    rate = setup['check']['fs_hz']
    np.testing.assert_allclose(history.abscissa, (np.arange(length) - pre) / rate)
    raw = np.fromfile(os.path.join(folder, 'streams', 'inputs', 'telemetry.f32'),
                      dtype='<f4').reshape(-1, 3)
    np.testing.assert_allclose(history.ordinate, raw[at:at + length].T.astype(np.float64)
                               * np.array([[LBF], [G], [G]]), rtol=1e-6)
    # the force peaks at the trigger the frame was taken around, at the peak the run checked
    hammer = history.ordinate[0] / LBF
    assert abs(int(np.argmax(hammer)) - lines[0]['report']['peak_frame']) <= 1
    assert hammer.max() == pytest.approx(lines[0]['peak'], rel=1e-6)
    decay = lines[0]['decay']
    assert setup['check']['impact']['trigger']['polarity'] == 'positive'
    assert history.comment[0] == (
        f'measurement 1, point 1, hit 1; positive hammer; peak {lines[0]["peak"]:.6g} lbf, '
        f"decayed to {decay['ratio'] * 100:.3g} % by the block's end at row {decay['row']} "
        '(7Z+)')
    assert history.channel_roles()[('1Z+', 'force')] == 'reference'


@pytest.mark.parametrize('name', [IMPACT, SHAKER])
def test_the_run_record_holds_the_setup_the_frame_lines_and_the_outcome(converted, name):
    _f, _done, project = converted[name]
    record = project['Run Record']
    assert isinstance(record, RunRecord) and record.test == 'modal_test'
    assert record.setup == _json(name, 'setup.json')
    assert record.config == _json(name, 'setup.json')['config']
    end = _json(name, 'end.json')
    assert record.end == end and record.complete and record.outcome == 'ended'
    frames = record.results['modal_frames']
    assert frames['records'] == _lines(name, 'modal_frames')
    outcome = record.results['modal_outcome']
    assert outcome['totals'] == end['totals']
    for key, value in end['totals'].items():
        assert record.arrays['modal_outcome'][key].tolist() == [float(value)]
    accepted = sum(1 for line in frames['records'] if line['status'] == 'accepted')
    assert accepted == end['totals']['accepted']
    for result in ('modal_frf', 'modal_coherence', 'modal_reference_spectra'):
        assert record.results[result]['records'] == _lines(name, result)
    assert record.missing == [_cut_line(name, 'hit' if name == IMPACT else 'frame')]
    # every frame and measurement line carries the decay check, none over the warning
    lines = [*frames['records'], *record.results['modal_frf']['records']]
    assert all(set(line['decay']) == {'row', 'ratio', 'warning'} for line in lines)
    assert not any(line['decay']['warning'] for line in lines)
    assert modal_run.findings(record) == []


def test_save_and_reopen_loses_nothing(converted, tmp_path):
    for name in (IMPACT, SHAKER):
        _f, _done, project = converted[name]
        again = Project.open(project.save(tmp_path / f'{name}.hforge'))
        assert list(again) == list(project)
        for obj in project:
            assert again[obj] == project[obj], obj


def _structure(name):
    return _json(name, 'setup.json')['resolved']['structure']


def _shape(node, mode):
    """The simulated structure's shape, as its setup states it."""
    return math.sin(mode * math.pi * (((node - 1) % 10) + 0.5) / 10)


def _fit(project, frf, finds=3):
    fit = fitting.start(project, {'frf': frf})
    assert fit.coherence is not None, "the run's coherence weights the fit"
    for _ in range(finds):
        fitting.act(fit, 'fit.find', {'low': 10.0, 'high': 200.0})
        fitting.act(fit, 'fit.confirm', {})
    return project[fitting.publish(project, fit, {})]


def test_the_impact_fit_recovers_the_structure_less_the_window(converted):
    """The roving hammer's set turned round goes to the fit; the fit reads the windowed
    damping, and each mode less the exponential window is the simulated structure's."""
    _f, _done, project = converted[IMPACT]
    assert fit_refusal(project['modal_frf fixed reference']) == ''
    shapes = _fit(project, 'modal_frf fixed reference')
    modes = _structure(IMPACT)['modes']
    order = np.argsort(shapes.frequency)
    assert shapes.num_shapes == len(modes) == 3
    for m, mode in zip(order, modes, strict=True):
        f, zeta = window_corrected(shapes)[m]
        # within the brief's 0.5 %: measured 0.21 % at 42 Hz, 0.05 % at 118 and 196 Hz
        assert abs(f / mode['f_hz'] - 1) < 5e-3
        # measured within 0.4 %
        assert abs(zeta / mode['damping'] - 1) < 1e-2
    # no drive point (hit at 1 to 3, measured at 4 and 7), so the shapes are unscaled; each
    # side's ratios are the structure's
    assert shapes.unscaled
    column = list(shapes.coordinate)
    for r, m in enumerate(order):
        for nodes in ((1, 2, 3), (4, 7)):
            got = [np.real(shapes.shape_matrix[m, column.index(f'{n}Z+')]) for n in nodes]
            want = [_shape(n, r + 1) for n in nodes]
            np.testing.assert_allclose(np.array(got) / got[-1], np.array(want) / want[-1],
                                       rtol=0, atol=2e-2)


def test_the_shaker_fit_finds_the_structures_three_modes_and_nothing_else(converted):
    """Four burst-random frames of a 1024-sample block over 201 lines: three finds give the
    modes at 42, 118 and 196 Hz, each within a tenth of a percent in frequency and 1 % in
    damping, one find to each mode, and no other mode."""
    _f, _done, project = converted[SHAKER]
    assert fit_refusal(project['modal_frf']) == ''
    shapes = _fit(project, 'modal_frf')
    modes = _structure(SHAKER)['modes']
    found = sorted(zip(shapes.frequency, shapes.damping, strict=True))
    assert len(found) == len(modes) == 3
    for (f, zeta), mode in zip(found, modes, strict=True):
        assert abs(f / mode['f_hz'] - 1) < 1e-3, (f, mode)
        assert abs(zeta / mode['damping'] - 1) < 1e-2, (zeta, mode)


def _cut_off(tmp_path, name, *, drop=(), lose=0):
    """A copy of a golden folder as a run cut off leaves it: no end.json, run.json not
    complete, no outcome; `lose` last lines (and their values) gone from each result in
    `drop`, with half a line written after them."""
    folder = str(tmp_path / name)
    shutil.copytree(os.path.join(RUNS, name), folder)
    os.remove(os.path.join(folder, 'end.json'))
    manifest = _json(name, 'run.json')
    manifest['complete'] = False
    manifest['results'] = [r for r in manifest['results'] if r['name'] != 'modal_outcome']
    with open(os.path.join(folder, 'run.json'), 'w', encoding='utf-8') as f:
        json.dump(manifest, f)
    for ext in ('json', 'bin'):
        os.remove(os.path.join(folder, 'results', f'modal_outcome.{ext}'))
    for result in drop:
        path = os.path.join(folder, 'results', result)
        lines = _lines(name, result)
        description = _json(name, 'results', f'{result}.json')
        size = max(a['offset'] + int(np.prod(a['shape'])) * (16 if a['dtype'] == 'c16' else 8)
                   for a in description['arrays'])
        with open(f'{path}.jsonl', 'w', encoding='utf-8') as f:
            f.writelines(json.dumps(line) + '\n' for line in lines[:len(lines) - lose])
            f.write(json.dumps(lines[-1])[:20])
        with open(f'{path}.bin', 'r+b') as f:
            f.truncate(size * (len(lines) - lose))
    return folder


def test_a_copy_cut_off_after_its_last_point_converts_with_every_measurement(tmp_path):
    done = run_convert.convert(_cut_off(tmp_path, IMPACT))
    project = Project.open(done.path)
    assert done.lacked == ['the manifest says the run is not complete',
                           'the run did not end cleanly: there is no end.json',
                           _cut_line(IMPACT, 'hit')]
    record = project['Run Record']
    assert record.end is None and not record.complete
    assert 'modal_outcome' not in record.results
    assert project['modal_frf'].num_records == 6


def test_a_copy_cut_off_before_its_last_spectra_keeps_the_last_frfs_as_they_are(tmp_path):
    folder = _cut_off(tmp_path, IMPACT, drop=('modal_coherence', 'modal_reference_spectra'),
                      lose=1)
    done = run_convert.convert(folder)
    project = Project.open(done.path)
    assert ('measurement 3 of point 3 has no coherence and no reference spectra; it is kept '
            'as it is') in done.lacked
    assert project['modal_frf'].num_records == 6
    assert project['modal_coherence'].num_records == 4
    assert project['modal_reference_spectra'].response_dof == ['1Z+', '2Z+']
    assert 'modal_frf fixed reference' in project


def test_a_copy_cut_off_before_its_last_point_says_the_point_and_its_hits(tmp_path):
    folder = _cut_off(tmp_path, IMPACT, drop=('modal_frf', 'modal_coherence',
                                              'modal_reference_spectra'), lose=1)
    done = run_convert.convert(folder)
    project = Project.open(done.path)
    assert '1 of the 3 points planned were not measured: point 3' in done.lacked
    assert ('2 accepted hits of point 3 belong to no finished measurement; they stay in the '
            'stream and the frame lines') in done.lacked
    assert project['modal_frf'].reference_dof == ['1Z+', '1Z+', '2Z+', '2Z+']
    assert fit_refusal(project['modal_frf fixed reference']) == ''


def _saved_by_the_guide():
    with open(os.path.join(ROOT, 'docs', 'run-folder.md'), encoding='utf-8') as f:
        text = f.read()
    section = text.split('## What a Modal Test Run Saves', 1)[1].split('\n## ', 1)[0]
    return dict(re.findall(r'^\| `([^`]+)` \| (.+) \|$', section, flags=re.MULTILINE))


def test_the_guide_says_exactly_what_a_modal_test_run_saves():
    named = _saved_by_the_guide()
    assert named, 'the guide lists nothing'
    on_disk = set()
    for name in (IMPACT, SHAKER):
        run = run_folder.read(os.path.join(RUNS, name))
        on_disk |= {f'streams/{n}' for n in run.streams} | {f'results/{n}' for n in run.results}
    on_disk.add('end.json')
    assert set(named) == on_disk
