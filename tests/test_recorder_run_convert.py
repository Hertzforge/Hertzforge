"""The Recorder run converter: a run folder in the control service's layout
becomes a `.hforge` run file that reads back bit for bit; station frames
follow the gaps; a run that did not finish converts what it wrote, a frame
cut short included; the file's size, not a sidecar, sets a recording's
length; a folder that is not a run is refused. The golden recording in
`schema/golden/` is converted when it is there, and with
`HERTZFORGE_RECORDER_RUN_DIR` naming a folder the control service wrote,
that folder is converted and checked as well."""

from __future__ import annotations

import glob
import json
import os

import numpy as np
import pytest

from hertzforge.io import recorder_run_convert as rrc

FS = 12800.0
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _source(station_frames: np.ndarray) -> np.ndarray:
    """The simulator's stated source on four rows at station frames `n`: the
    markers, 250 Hz at 0.2, 433 Hz at 0.1 and 1000 Hz at 0.15."""
    t = station_frames / FS
    u = np.mod(t, 2.0) - 0.5
    marker = np.where((u >= 0) & (u < 0.010), 0.5 * np.sin(np.pi * u / 0.010), 0.0)
    return np.stack([marker,
                     0.2 * np.sin(2 * np.pi * 250 * t),
                     0.1 * np.sin(2 * np.pi * 433 * t),
                     0.15 * np.sin(2 * np.pi * 1000 * t)], axis=1).astype(np.float32)


def _recording(folder, name, first, pre, frames, *, gaps=(), complete=True, trigger='level',
               extra_bytes=b'', stated_frames=None, fault=None):
    """A recording folder written the way the control service writes one, from
    station frame `first`, with `gaps` [(at_frame, dropped)] left out of the file."""
    os.makedirs(folder)
    n = np.arange(first, first + frames + sum(d for _, d in gaps), dtype=np.int64)
    keep = np.ones(n.size, dtype=bool)
    for at, dropped in gaps:
        keep[at:at + dropped] = False
    data = _source(n[keep])
    with open(os.path.join(folder, 'telemetry.f32'), 'wb') as f:
        f.write(data.astype('<f4').tobytes())
        f.write(extra_bytes)
    side = {'complete': complete, 'name': name, 'trigger': trigger, 'requested_fs_hz': FS,
            'fs_hz': FS, 'group_delay_frames': 38.44, 'rows': [1, 2, 3, 4],
            'units': ['g', 'g', 'g', 'g'], 'first_station_frame': first,
            'trigger_station_frame': first + pre, 'pre_frames': pre, 'pre_frames_asked': 6400,
            'frames': frames if stated_frames is None else stated_frames,
            'gaps': [{'at_frame': a, 'dropped_frames': d} for a, d in gaps],
            'marks': [], 'fault': fault, 'a_field_a_reader_ignores': 1}
    with open(os.path.join(folder, 'recorder.json'), 'w', encoding='utf-8') as f:
        json.dump(side, f)
    with open(os.path.join(folder, 'capture.json'), 'w', encoding='utf-8') as f:
        json.dump({'config': {'n_channels': 4, 'sample_rate_hz': FS}, 'frames_written': frames,
                   'chunks_dropped': len(gaps), 'flushes': 1, 'complete': complete,
                   'free_space_floor_bytes': 1 << 30, 'fault': fault}, f)
    return data


def _run(folder, *, finished=True):
    os.makedirs(folder)
    with open(os.path.join(folder, 'run.json'), 'w', encoding='utf-8') as f:
        json.dump({'config': {'title': 'Fixture A, run 4'}, 'rows': [1, 2, 3, 4]}, f)
    with open(os.path.join(folder, 'log.jsonl'), 'w', encoding='utf-8') as f:
        f.write(json.dumps({'kind': 'state', 'text': 'records in here'}) + '\n')
        f.write(json.dumps({'kind': 'recording', 'text': 'rec_0001 began'}) + '\n')
    with open(os.path.join(folder, 'marks.jsonl'), 'w', encoding='utf-8') as f:
        f.write(json.dumps({'kind': 'limit', 'row': 1, 'axis': 'accel', 'value': 0.5,
                            'limit': 0.4, 'station_frame': 6410, 'recording': 'rec_0001'})
                + '\n')
    if finished:
        with open(os.path.join(folder, 'end.json'), 'w', encoding='utf-8') as f:
            json.dump({'outcome': 'operator_stop', 'recordings': 2}, f)
    rec = os.path.join(folder, 'recordings')
    a = _recording(os.path.join(rec, 'rec_0001'), 'rec_0001', 0, 6400, 19200)
    b = _recording(os.path.join(rec, 'rec_0002'), 'rec_0002', 25600, 6400, 19200)
    return a, b


def test_recorder_run_folder_converts_and_reads_back_exactly(tmp_path):
    folder = tmp_path / 'recorder'
    a, b = _run(str(folder))
    run = rrc.read(rrc.convert(str(folder), str(tmp_path / 'run.hforge')))
    assert run['attrs']['test'] == 'recorder'
    assert run['attrs']['run_folder'] == 'recorder'
    assert bool(run['attrs']['complete'])
    assert json.loads(run['settings'][0])['config']['title'] == 'Fixture A, run 4'
    assert json.loads(run['end'][0])['outcome'] == 'operator_stop'
    assert [json.loads(x)['kind'] for x in run['log']] == ['state', 'recording']
    assert json.loads(run['marks'][0])['station_frame'] == 6410
    assert rrc.recording_names(run) == ['rec_0001', 'rec_0002']
    for name, want in (('rec_0001', a), ('rec_0002', b)):
        g = run['recordings'][name]
        assert g['data'].dtype == np.float32
        assert g['data'].shape == (19200, 4)
        np.testing.assert_array_equal(g['data'], want)
        at = g['attrs']
        assert at['frames'] == 19200 and at['frames_stated'] == 19200
        assert at['fs_hz'] == FS and at['requested_fs_hz'] == FS
        assert at['group_delay_frames'] == pytest.approx(38.44)
        assert list(at['rows']) == [1, 2, 3, 4] and list(at['units']) == ['g'] * 4
        assert at['pre_frames'] == 6400 and bool(at['gap_free']) and bool(at['complete'])
        assert at['fault'] == '' and at['partial_frame_bytes'] == 0
        assert json.loads(g['recorder_json'][0])['name'] == name
    rec2 = rrc.recording_of(run, 'rec_0002')
    assert rrc.station_frames(rec2)[0] == 25600
    t = rrc.seconds_from_trigger(rec2)
    assert t[6400] == 0.0 and t[0] == pytest.approx(-0.5)
    # The stated source at its station frames: the marker begins at 6400 + 25600 k.
    col = run['recordings']['rec_0002']['data'][:, 0]
    assert col[6400] == 0.0 and col[6400 + 64] == pytest.approx(0.5, abs=1e-6)


def test_recorder_gaps_place_each_frame_at_its_station_frame(tmp_path):
    folder = tmp_path / 'rec_0001'
    want = _recording(str(folder), 'rec_0001', 1000, 0, 8192, gaps=[(4096, 2048)],
                      trigger='start')
    run = rrc.read(rrc.convert(str(folder), str(tmp_path / 'gap.hforge')))
    rec = rrc.recording_of(run, 'rec_0001')
    assert not bool(rec['gap_free'])
    np.testing.assert_array_equal(rec['gaps'], [[4096, 2048]])
    sf = rrc.station_frames(rec)
    assert sf[4095] == 1000 + 4095 and sf[4096] == 1000 + 4096 + 2048
    np.testing.assert_array_equal(rec['data'], want)
    # The file after the gap equals the source at the station frames the gap leaves.
    np.testing.assert_array_equal(rec['data'][4096:], _source(sf[4096:]))


def test_recorder_run_that_did_not_finish_converts_what_it_wrote(tmp_path):
    folder = tmp_path / 'recorder'
    _run(str(folder), finished=False)
    cut = folder / 'recordings' / 'rec_0003'
    want = _recording(str(cut), 'rec_0003', 51200, 6400, 10000, complete=False,
                      extra_bytes=b'\x00' * 6, stated_frames=8192)
    with open(folder / 'marks.jsonl', 'a', encoding='utf-8') as f:
        f.write('{"kind": "limit", "row": 1, "va')
    run = rrc.read(rrc.convert(str(folder), str(tmp_path / 'cut.hforge')))
    assert not bool(run['attrs']['complete'])
    assert 'end' not in run
    assert len(run['marks']) == 1
    g = run['recordings']['rec_0003']
    assert not bool(g['attrs']['complete'])
    assert g['attrs']['partial_frame_bytes'] == 6
    # The file's size sets the length: 10 000 whole frames, though the sidecar says 8192.
    assert g['attrs']['frames'] == 10000 and g['attrs']['frames_stated'] == 8192
    np.testing.assert_array_equal(g['data'], want)
    assert bool(run['recordings']['rec_0001']['attrs']['complete'])


def test_recorder_run_with_no_end_is_not_complete(tmp_path):
    folder = tmp_path / 'recorder'
    _run(str(folder), finished=False)
    run = rrc.read(rrc.convert(str(folder), str(tmp_path / 'open.hforge')))
    assert all(bool(run['recordings'][n]['attrs']['complete'])
               for n in rrc.recording_names(run))
    assert not bool(run['attrs']['complete'])


def test_recorder_a_recordings_fault_is_kept(tmp_path):
    folder = tmp_path / 'rec_0001'
    _recording(str(folder), 'rec_0001', 0, 0, 2048, trigger='start',
               fault='the free space fell under the floor')
    run = rrc.read(rrc.convert(str(folder), str(tmp_path / 'fault.hforge')))
    assert run['recordings']['rec_0001']['attrs']['fault'] == \
        'the free space fell under the floor'


def test_recorder_folder_that_is_not_a_run_is_refused_with_what_to_do(tmp_path):
    with pytest.raises(rrc.RunFolderError, match='choose the run folder'):
        rrc.convert(str(tmp_path), str(tmp_path / 'x.hforge'))
    assert not os.path.exists(tmp_path / 'x.hforge')
    assert not os.path.exists(tmp_path / 'x.hforge.partial')


def test_recorder_sidecar_with_no_rows_is_refused_and_leaves_no_file(tmp_path):
    folder = tmp_path / 'rec_0001'
    _recording(str(folder), 'rec_0001', 0, 0, 256, trigger='start')
    side = json.loads((folder / 'recorder.json').read_text())
    side['rows'] = []
    (folder / 'recorder.json').write_text(json.dumps(side))
    with pytest.raises(rrc.RunFolderError, match='names no recorded rows'):
        rrc.convert(str(folder), str(tmp_path / 'y.hforge'))
    assert not os.path.exists(tmp_path / 'y.hforge')
    assert not os.path.exists(tmp_path / 'y.hforge.partial')


def test_recorder_file_from_a_newer_schema_or_another_run_is_refused(tmp_path):
    import h5py
    p = tmp_path / 'new.hforge'
    with h5py.File(p, 'w') as f:
        f.attrs['hertzforge_schema'] = rrc.HFORGE_SCHEMA + 1
        f.create_group('run').attrs['test'] = 'recorder'
    with pytest.raises(rrc.RunFolderError, match='newer Hertzforge'):
        rrc.read(str(p))
    q = tmp_path / 'mimo.hforge'
    with h5py.File(q, 'w') as f:
        f.attrs['hertzforge_schema'] = rrc.HFORGE_SCHEMA
        f.create_group('run').attrs['test'] = 'mimo_random'
    with pytest.raises(rrc.RunFolderError, match='no Recorder run'):
        rrc.read(str(q))


def _golden_recordings() -> list[str]:
    return sorted(os.path.dirname(p) for p in
                  glob.glob(os.path.join(ROOT, 'schema', 'golden', '**', 'recorder.json'),
                            recursive=True)
                  if os.path.exists(os.path.join(os.path.dirname(p), 'telemetry.f32')))


@pytest.mark.skipif(not _golden_recordings(),
                    reason='the golden recording is not in schema/golden yet')
def test_recorder_golden_recording_converts(tmp_path):
    for folder in _golden_recordings():
        with open(os.path.join(folder, 'recorder.json'), encoding='utf-8') as f:
            side = json.load(f)
        rows = side['rows']
        raw = np.fromfile(os.path.join(folder, 'telemetry.f32'), dtype='<f4')
        run = rrc.read(rrc.convert(folder, str(tmp_path / 'golden.hforge')))
        (name,) = rrc.recording_names(run)
        g = run['recordings'][name]
        assert g['data'].shape == (raw.size // len(rows), len(rows))
        np.testing.assert_array_equal(g['data'].ravel(), raw[:g['data'].size])
        assert g['attrs']['frames'] == side['frames']
        assert g['attrs']['fs_hz'] == side['fs_hz']
        assert list(g['attrs']['rows']) == rows
        assert bool(g['attrs']['complete']) == bool(side['complete'])


@pytest.mark.skipif('HERTZFORGE_RECORDER_RUN_DIR' not in os.environ,
                    reason='set HERTZFORGE_RECORDER_RUN_DIR to a run folder the control '
                           'service wrote')
def test_recorder_run_folder_the_control_service_wrote_converts(tmp_path):
    folder = os.environ['HERTZFORGE_RECORDER_RUN_DIR']
    run = rrc.read(rrc.convert(folder, str(tmp_path / 'service.hforge')))
    assert rrc.recording_names(run)
    for name in rrc.recording_names(run):
        g = run['recordings'][name]
        rec = rrc.recording_of(run, name)
        size = os.path.getsize(os.path.join(folder, 'recordings', name, 'telemetry.f32'))
        assert g['data'].shape[0] == size // (4 * len(rec['rows']))
        if bool(rec['gap_free']) and rec['frames'] > rec['pre_frames']:
            assert rrc.station_frames(rec)[rec['pre_frames']] == rec['trigger_station_frame']
