"""File-level tools: Save As in a foreign format, a part of a Rattlesnake run
previewed and imported, a run cut and written back, and delimited text
previewed and imported with its layout corrected."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pytest

from hertzforge.service import ops_files, projects
from hertzforge.service.ops_files import OPERATIONS
from hertzforge.service.protocol import ProtocolError

from .conftest import wire_views

PLATE = Path(__file__).resolve().parents[2] / 'testdata' / 'plate'
RUN = str(PLATE / 'random.nc4')
OPS = {op.id: op for op in OPERATIONS}


@pytest.fixture
def project():
    state, _ = projects.import_file(None, {'path': str(PLATE / 'geometry.unv')})
    return state.project


def run(project, op_id, **given):
    op = OPS[op_id]
    values = op.resolve(given, objects={name: projects.kind_of(obj)
                                        for name, obj in project.items()})
    return op.run(project, values)


def rows(reading):
    return {row['label']: row for row in reading['rows']}


def history():
    from hertzforge.core.data import TimeHistory

    t = np.arange(256) / 256.0
    return TimeHistory(t, [np.sin(2 * np.pi * 8 * t)], response_dof=['1Z+'],
                       ordinate_dim=['acceleration'], ordinate_unit=['m/s^2'])


def test_the_descriptors_validate(validate):
    for op in OPERATIONS:
        validate(op.to_json(), 'operation')


# ---- Save As ---------------------------------------------------------------------

def test_a_geometry_saved_as_unv_reads_back_whole(project, tmp_path, validate):
    import hertzforge as hz

    (reading,) = run(project, 'project.save_as', object='Geometry', format='unv',
                     path=str(tmp_path / 'plate'))
    wire_views([reading], validate)
    written = rows(reading)['Written']['value']
    assert written == str(tmp_path / 'plate.unv')
    assert rows(reading)['Size']['value'] == Path(written).stat().st_size
    assert hz.import_file(written).num_nodes == project['Geometry'].num_nodes


def test_a_format_that_cannot_hold_the_object_is_refused_naming_those_that_can(project,
                                                                               tmp_path):
    with pytest.raises(ProtocolError, match=r'cannot be written as Delimited text.*'
                                            r'Universal file \(\.unv\)'):
        run(project, 'project.save_as', object='Geometry', format='delimited',
            path=str(tmp_path / 'plate.csv'))
    with pytest.raises(ProtocolError, match='needs a path'):
        run(project, 'project.save_as', object='Geometry', format='unv')


def test_an_escdf_file_records_the_creator_given(project, tmp_path):
    from hertzforge.io import escdf

    project.add('Time History', history())
    (reading,) = run(project, 'project.save_as', object='Time History', format='escdf',
                     path=str(tmp_path / 'run'), creator='A. Tester')
    assert rows(reading)['Written']['value'] == str(tmp_path / 'run.h5')
    assert escdf.read(tmp_path / 'run.h5').created_by == 'A. Tester'


def test_data_to_exodus_rides_on_the_geometry_or_is_refused(project, tmp_path):
    from hertzforge.project import Project

    project.add('Time History', history())
    with pytest.raises(ProtocolError, match='has none of the data DOFs'):
        run(project, 'project.save_as', object='Time History', format='exodus',
            path=str(tmp_path / 'run.exo'))
    alone = Project()
    alone.add('Time History', history())
    with pytest.raises(ProtocolError, match='needs a geometry'):
        run(alone, 'project.save_as', object='Time History', format='exodus',
            path=str(tmp_path / 'run.exo'))


def test_an_object_saved_as_hforge_reads_back(project, tmp_path):
    import hertzforge as hz

    project.add('Time History', history())
    (reading,) = run(project, 'project.save_as', object='Time History',
                     path=str(tmp_path / 'run'))
    back = hz.io.load(rows(reading)['Written']['value'])
    assert np.allclose(back.ordinate, project['Time History'].ordinate)


# ---- Rattlesnake runs --------------------------------------------------------------

def test_a_preview_reads_the_runs_envelope_and_what_a_window_costs(project, validate):
    import netCDF4

    view, reading, table = run(project, 'project.rattlesnake_preview', path=RUN, start=5.0,
                               stop=10.0, channels='1, 2')
    wire_views([view, reading, table], validate)
    with netCDF4.Dataset(RUN) as ds:
        raw = np.asarray(ds.variables['time_data'][0], dtype=float)
    # 101Z+ is in m/s², SI already
    assert view['series'][0]['y'].max() == pytest.approx(raw.max())
    assert view['series'][0]['y'].min() == pytest.approx(raw.min())
    assert [(r['x0'], r['x1']) for r in view['regions']] == [(0.0, 5.0), (10.0, raw.size / 4096
                                                                         - 1 / 4096)]
    found = rows(reading)
    assert found['Samples of each channel']['value'] == 5 * 4096 + 1
    assert found['Channels kept']['value'] == '2 of 12'
    assert found['Import size']['value'] == pytest.approx((5 * 4096 + 1) * 2 * 8 / 2 ** 30)
    assert table['rows'][0] == ['1', '101Z+', 'm/s^2']
    with pytest.raises(ProtocolError, match='not both'):
        run(project, 'project.rattlesnake_preview', path=RUN, last=4.0, start=1.0)
    with pytest.raises(ProtocolError, match='not a Rattlesnake run'):
        run(project, 'project.rattlesnake_preview', path=str(PLATE / 'geometry.unv'))


def test_the_last_seconds_and_two_channels_import_as_that_part(project, validate):
    (reading,) = run(project, 'project.import_rattlesnake_part', path=RUN, last=4.0,
                     channels='1 104Z+')
    wire_views([reading], validate)
    data = project['Time History']
    assert list(data.response_dof) == ['101Z+', '104Z+']
    seconds = (114688 - 1) / 4096
    assert data.abscissa[0] == pytest.approx(seconds - 4.0, abs=1 / 4096)
    assert data.abscissa[-1] == pytest.approx(seconds)
    assert rows(reading)['Channels']['value'] == '2 of 12'
    assert 'Channel Table' in project
    with pytest.raises(ProtocolError, match='there is no channel 13'):
        run(project, 'project.import_rattlesnake_part', path=RUN, channels='13')


def test_a_run_cut_and_written_back_holds_the_window(project, tmp_path, validate):
    from hertzforge.io import rattlesnake

    target = tmp_path / 'cut.nc4'
    (reading,) = run(project, 'project.truncate_rattlesnake_file', path=RUN, start=2.0,
                     stop=3.0, target=str(target))
    wire_views([reading], validate)
    assert rows(reading)['Samples kept, time_data']['value'] == 4097
    (stream,) = rattlesnake.stream_summary(target)['streams']
    assert stream['samples'] == 4097
    assert 'save_window' in project.journal[-1]
    assert list(project) == ['Geometry']
    with pytest.raises(ProtocolError, match='whole run'):
        run(project, 'project.truncate_rattlesnake_file', path=RUN, target=str(target))
    with pytest.raises(ProtocolError, match='cannot be written over the run'):
        run(project, 'project.truncate_rattlesnake_file', path=RUN, start=1.0, target=RUN)


def test_a_cut_with_no_copy_named_is_named_for_its_window(project, tmp_path):
    import shutil

    copy = tmp_path / 'random.nc4'
    shutil.copy(RUN, copy)
    (reading,) = run(project, 'project.truncate_rattlesnake_file', path=str(copy), last=1.0)
    start = (114688 - 1) / 4096 - 1.0
    assert rows(reading)['Written']['value'] == str(tmp_path / f'random_{start:g}s-end.nc4')
    assert rows(reading)['Samples kept, time_data']['value'] == 4097


# ---- delimited text -----------------------------------------------------------------

@pytest.fixture
def text(tmp_path):
    path = tmp_path / 'run.txt'
    body = '\n'.join(f'{0.5 * k:.1f};{float(k):.1f}'.replace('.', ',') for k in range(50))
    path.write_text(f'Sample Rate: 100\nAccel (g);Force (N)\n{body}\n')
    return str(path)


def test_a_text_preview_shows_the_layout_found_and_the_head_in_si(project, text, validate):
    reading, columns, first, traces = run(project, 'project.text_preview', path=text)
    wire_views([reading, columns, first, traces], validate)
    found = rows(reading)
    assert found['Delimiter']['value'] == 'semicolon'
    assert found['Decimal point']['value'] == 'comma'
    assert found['Names line']['value'] == 2
    assert found['Sample rate stated']['value'] == 100
    assert found['Read']['value'] == '2 channels at 100 samples per second'
    assert columns['rows'] == [['1', 'Accel', 'g'], ['2', 'Force', 'N']]
    assert first['columns'] == ['Time [s]', 'Accel [m/s²]', 'Force [N]']
    assert first['rows'][1] == ['0.01', f'{0.5 * 9.80665:.6g}', '1']
    assert len(traces['series']) == 2


def test_corrections_reach_the_read_and_a_missing_column_is_refused(project, text):
    reading, _columns, first, _traces = run(project, 'project.text_preview', path=text,
                                            columns='2', names='1X+')
    assert first['columns'] == ['Time [s]', '1X+ [N]']
    assert "columns=[1], names=['1X+']" in rows(reading)['Corrections']['value']
    reading, *_views = run(project, 'project.text_preview', path=text, sample_rate=200.0)
    assert rows(reading)['Read']['value'] == '2 channels at 200 samples per second'
    with pytest.raises(ProtocolError, match='Torque'):
        run(project, 'project.text_preview', path=text, columns='Torque')


def test_a_text_import_adds_one_history_read_with_the_corrections(project, text, validate):
    (reading,) = run(project, 'project.import_text', path=text, names='1X+, 1Z+',
                     units='Accel=g')
    wire_views([reading], validate)
    data = project['Time History']
    assert list(data.response_dof) == ['1X+', '1Z+']
    assert data.sample_rate == pytest.approx(100.0)
    assert data.ordinate[0, 1] == pytest.approx(0.5 * 9.80665)
    with pytest.raises(ProtocolError, match='does not read as delimited text'):
        run(project, 'project.import_text', path=RUN)


def test_export_asks_for_its_own_units_and_not_the_text_imports():
    units = next(p for op in OPERATIONS if op.id == 'project.save_as' for p in op.params
                 if p.name == 'units')
    assert units.default == 'display' and [o.value for o in units.options] == ['display', 'si']


def test_a_foreign_file_is_written_in_the_units_asked_for(tmp_path, validate):
    state, _ = projects.import_file(None, {'path': str(PLATE / 'geometry.unv'),
                                           'units': {'length_unit': 'mm'}})
    project = state.project
    (si,) = run(project, 'project.save_as', object='Geometry', format='unv',
                path=str(tmp_path / 'si'), units='si')
    # the server puts the connection's system in for 'display' (test_display_units)
    written = {'object': 'Geometry', 'format': 'unv', 'creator': ''}
    (inches,) = ops_files.save_as(project, {**written, 'path': str(tmp_path / 'inches'),
                                            'units': 'in-slinch-lbf-s'})
    wire_views([si, inches], validate)
    assert rows(si)['Units']['value'] == 'm-kg-N-s'
    assert rows(inches)['Units']['value'] == 'in-slinch-lbf-s'
    assert (tmp_path / 'si.unv').read_bytes() != (tmp_path / 'inches.unv').read_bytes()
    with pytest.raises(ProtocolError, match="no unit system named 'cubits'"):
        ops_files.save_as(project, {**written, 'path': str(tmp_path / 'odd'), 'units': 'cubits'})
