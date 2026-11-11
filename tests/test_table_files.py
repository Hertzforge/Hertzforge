"""Channel and breakpoint tables as files: every form written and read back,
headings read by their aliases, what is not a column reported, and a file of
another kind refused in words."""

from __future__ import annotations

import json
from pathlib import Path

import h5py
import numpy as np
import pytest

import hertzforge as hz
from hertzforge.core.channel_table import ChannelTable
from hertzforge.io import tables as t

GOLDEN = Path(__file__).resolve().parents[1] / 'schema' / 'golden'
FORMS = ('.hforge', '.csv', '.tsv', '.txt', '.xlsx')

#: the channel table the shell saves for five channels: every setting of a
#: control channel, a reference, an off monitor, a drive and a second
#: control group, with some of the analysis columns it carries
SAVED = json.loads((GOLDEN / 'request_tables_save.json').read_text(encoding='utf-8'))['params']


def channels():
    return t.Table('channels', list(SAVED['columns']), [list(r) for r in SAVED['rows']])


def profile(table_kind='random_profile'):
    columns = ['freq_hz', 'psd_g2_hz', 'slope_db_oct', 'alarm_below_db', 'alarm_above_db']
    rows = [[20.0, None, 3.0, 1.5, 1.5], [80.0, 0.04, None, None, None],
            [2000.0, None, -6.0, 3.0, 3.0]]
    return t.Table('breakpoints', columns, rows, table_kind=table_kind)


def by_column(table):
    return [dict(zip(table.columns, row)) for row in table.rows]


def converted(name):
    return (f'{name} had no controller settings, so each channel was given them at their '
            'defaults: on, unbound and without abort limits.')


def measurement_table():
    return ChannelTable({
        'channel': [1, 2], 'node': ['101', '7'], 'direction': ['Z+', 'X-'],
        'unit': ['g', 'lbf'], 'sensitivity': ['10.2', '2.25'], 'daq_range': ['5', '0.5'],
        'sensor_range': ['5', '10'], 'serial_number': ['0123', ''], 'make': ['Acme', ''],
        'model': ['T1', ''], 'triax_dof': ['Z', ''], 'expiration': ['2027-03-01', ''],
        'role': ['response', 'reference'], 'control': ['True', 'False'],
        'comment': ['Table top', 'Fixture']})


def written_columns(path):
    """The columns a file holds, as its own headings name them."""
    from openpyxl import load_workbook

    from hertzforge.core.channel_table import canonical_name

    suffix = Path(path).suffix
    if suffix == '.hforge':
        with h5py.File(path) as f:
            return [str(c) for c in f['channel_table'].attrs['column_order']]
    if suffix == '.xlsx':
        book = load_workbook(path, read_only=True)
        header = next(book.active.iter_rows(values_only=True))
        book.close()
        return [canonical_name(h) for h in header]
    lines = [line for line in Path(path).read_text(encoding='utf-8').splitlines()
             if not line.startswith('#')]
    return [canonical_name(h) for h in lines[0].split(',' if suffix == '.csv' else '\t')]


@pytest.mark.parametrize('suffix', FORMS)
def test_every_setting_of_a_channel_reads_back_as_saved(tmp_path, suffix):
    path = t.write(tmp_path / f'table{suffix}', channels())
    read = t.read(path, 'channels')
    assert read.warnings == [] and read.table_kind is None
    # every column of the one table comes back, in its order, the controller's last
    assert read.columns == list(ChannelTable.COLUMNS)
    assert written_columns(path) == list(ChannelTable.COLUMNS)
    for sent, got in zip(SAVED['rows'], by_column(read)):
        assert {c: got[c] for c in SAVED['columns']} == dict(zip(SAVED['columns'], sent))
    # what the table works out for itself from a channel's unit
    assert [r['channel_type'] for r in by_column(read)] == [
        'acceleration', 'force', 'velocity', 'voltage', 'length']


@pytest.mark.parametrize('suffix', FORMS)
def test_a_table_without_controller_settings_is_written_as_it_is(tmp_path, suffix):
    table = measurement_table()
    columns, rows = t.channel_rows(table)
    assert columns == list(ChannelTable.SCHEMA)
    path = t.write(tmp_path / f'table{suffix}', t.Table('channels', columns, rows))
    # no empty controller columns in the file
    assert written_columns(path) == list(ChannelTable.SCHEMA)
    # opened for a test, it carries them, and the measurement's columns are as they were
    read = t.read(path, 'channels')
    assert read.columns == list(ChannelTable.COLUMNS)
    assert [row[:len(columns)] for row in read.rows] == rows
    assert read.warnings == [converted(f'table{suffix}')]
    # a serial number of digits keeps its leading zero
    assert by_column(read)[0]['serial_number'] == '0123'


@pytest.mark.parametrize('suffix', FORMS)
def test_the_analysis_columns_survive_the_control_settings(tmp_path, suffix):
    table = measurement_table()
    table.add_controller_settings()
    table.set_cell('physical', 0, 'cDAQ1Mod1/ai3')
    table.set_cell('abort_accel_g', 0, '40')
    columns, rows = t.channel_rows(table)
    path = t.write(tmp_path / f'table{suffix}', t.Table('channels', columns, rows))
    read = t.read(path, 'channels')
    assert (read.columns, read.rows, read.warnings) == (columns, rows, [])


def test_a_table_saved_before_the_controller_settings_reads_with_them_at_their_defaults(
        tmp_path):
    path = tmp_path / 'older.hforge'
    with h5py.File(path, 'w') as f:
        f.attrs['hertzforge_schema'] = 1
        group = f.create_group('channel_table')
        group.attrs['column_order'] = ['channel', 'node', 'direction', 'unit', 'control']
        group.create_dataset('channel', data=np.array([1, 2], dtype=np.int64))
        for name, values in (('node', ['1', '2']), ('direction', ['Z+', 'Z+']),
                             ('unit', ['g', 'g']), ('control', ['True', 'False'])):
            group.create_dataset(name, data=values, dtype=h5py.string_dtype('utf-8'))
    read = t.read(path, 'channels')
    first = by_column(read)[0]
    # a channel that never said is on, as every channel of a recorded run was
    assert (first['enabled'], first['drive'], first['iepe'], first['physical'],
            first['polarity'], first['range_max'], first['abort_accel_g']) == (
        True, False, False, '', '', None, None)
    assert [r['control'] for r in by_column(read)] == [True, False]
    assert read.warnings == [converted('older.hforge')]


def test_the_hforge_form_is_the_channel_tables_own_object(tmp_path):
    path = t.write(tmp_path / 'table.hforge', channels())
    with h5py.File(path) as f:
        assert list(f) == ['channel_table']
        group = f['channel_table']
        assert [str(c) for c in group.attrs['column_order']] == list(ChannelTable.COLUMNS)
        assert group['physical'][0].decode() == 'cDAQ1Mod1/ai0'
        assert group['enabled'][2].decode() == 'False'
    # and a project takes it as any channel table
    project = hz.Project('Survey')
    (added,) = project.import_file(path)
    assert isinstance(project[added], ChannelTable)
    assert list(project[added]['physical'][:2]) == ['cDAQ1Mod1/ai0', 'cDAQ1Mod1/ai1']


def test_headings_are_read_by_the_channel_tables_own_names_and_aliases(tmp_path):
    path = tmp_path / 'by hand.csv'
    path.write_text(
        '// made by hand\n'
        'Channel Number,Node Number,Dir,Units,Sensitivity (mV/g),On,Description,'
        'IEPE,IEPE Bias (V),Abort Accel (g),Physical,Range Max (Unit),Location\n'
        '1,101,Z+,g,100,True,Table,True,24,12,cDAQ1Mod1/ai0,50,bench\n'
        '\n'
        '2,7,X-,g,98.5,no,Fixture,,,,,,\n', encoding='utf-8')
    read = t.read(path, 'channels')
    rows = by_column(read)
    assert {k: rows[0][k] for k in ('channel', 'node', 'direction', 'unit', 'sensitivity',
                                    'enabled', 'comment', 'iepe', 'iepe_bias',
                                    'abort_accel_g', 'physical', 'range_max')} == {
        'channel': 1, 'node': 101, 'direction': 'Z+', 'unit': 'g', 'sensitivity': 100.0,
        'enabled': True, 'comment': 'Table', 'iepe': True, 'iepe_bias': 24.0,
        'abort_accel_g': 12.0, 'physical': 'cDAQ1Mod1/ai0', 'range_max': 50.0}
    assert (rows[1]['enabled'], rows[1]['iepe'], rows[1]['iepe_bias']) == (False, False, None)
    assert read.warnings == [_left_out('location')]


def test_a_cell_that_cannot_be_read_takes_its_default_and_says_so(tmp_path):
    path = tmp_path / 'typos.tsv'
    path.write_text('Channel\tNode\tDirection\tUnit\tAbort Accel (g)\tPolarity\n'
                    '1\t1\tZ+\tg\ttwelve\tbackwards\n', encoding='utf-8')
    read = t.read(path, 'channels')
    assert (by_column(read)[0]['abort_accel_g'], by_column(read)[0]['polarity']) == (None, '')
    assert read.warnings == [
        "Channel 1, Abort Accel (g): 'twelve' could not be read; it was left blank.",
        "Channel 1, Polarity: 'backwards' could not be read; it was left blank."]


def test_a_channel_number_that_cannot_be_read_is_refused(tmp_path):
    path = tmp_path / 'numbers.csv'
    path.write_text('Channel,Node,Direction,Unit\none,1,Z+,g\n', encoding='utf-8')
    with pytest.raises(t.TableFileError, match="'one' in the Channel column is not a channel"):
        t.read(path, 'channels')


def test_a_workbook_another_program_wrote_reads_through_the_excel_reader(tmp_path):
    from openpyxl import Workbook

    book = Workbook()
    sheet = book.active
    sheet.append(['Test channels'])
    sheet.append(['Ch', 'Point', 'Axis', 'EU', 'Sens', 'On'])
    sheet.append([1, 3, 'Y+', 'g', 9.8, 'yes'])
    book.save(tmp_path / 'lab.xlsx')
    first = by_column(t.read(tmp_path / 'lab.xlsx', 'channels'))[0]
    assert (first['channel'], first['node'], first['direction'], first['sensitivity'],
            first['enabled']) == (1, 3, 'Y+', 9.8, True)


def test_semicolons_a_byte_order_mark_and_a_windows_code_page_are_read(tmp_path):
    path = tmp_path / 'excel.csv'
    path.write_bytes('﻿Channel;Node;Direction;Unit;Comment\n1;1;Z+;g;Mass ×2\n'.encode())
    assert by_column(t.read(path, 'channels'))[0]['comment'] == 'Mass ×2'
    path.write_bytes('Channel,Node,Direction,Unit,Comment\n1,1,Z+,g,Kraft \xb5\n'
                     .encode('latin-1'))
    assert by_column(t.read(path, 'channels'))[0]['comment'] == 'Kraft µ'


@pytest.mark.parametrize('suffix', FORMS)
def test_a_breakpoint_table_reads_back_with_its_kind_and_units(tmp_path, suffix):
    table = profile()
    table.x_unit, table.y_unit = 'Hz', 'g²/Hz'
    path = t.write(tmp_path / f'profile{suffix}', table)
    read = t.read(path, 'breakpoints', 'random_profile')
    assert (read.columns, read.rows, read.warnings) == (table.columns, table.rows, [])
    assert (read.table_kind, read.x_unit, read.y_unit) == ('random_profile', 'Hz', 'g²/Hz')
    # opened without saying which: the file says
    assert t.read(path, 'breakpoints').table_kind == 'random_profile'


def test_a_path_without_a_suffix_is_written_as_hforge(tmp_path):
    assert t.write(tmp_path / 'table', channels()) == str(tmp_path / 'table.hforge')


def test_the_breakpoint_layout_is_one_group_one_dataset_per_column(tmp_path):
    path = t.write(tmp_path / 'profile.hforge', profile())
    with h5py.File(path) as f:
        assert int(f.attrs['hertzforge_schema']) == 1
        assert list(f) == ['breakpoint_table']
        group = f['breakpoint_table']
        assert [str(c) for c in group.attrs['column_order']] == profile().columns
        assert (group.attrs['table_kind'], group.attrs['x_unit'],
                group.attrs['y_unit']) == ('random_profile', 'Hz', 'g²/Hz')
        assert group['freq_hz'].dtype == np.float64 and np.isnan(group['psd_g2_hz'][0])


def test_a_symmetric_band_column_fills_both_sides(tmp_path):
    path = tmp_path / 'profile.csv'
    path.write_text('Frequency (Hz),PSD (g^2/Hz),Alarm (dB),Abort-,Abort+,Note\n'
                    '20,0.01,1.5,3,6,flat\n2000,0.01,,,,\n', encoding='utf-8')
    read = t.read(path, 'breakpoints', 'random_profile')
    assert read.columns == ['freq_hz', 'psd_g2_hz', 'alarm_below_db', 'alarm_above_db',
                            'abort_below_db', 'abort_above_db']
    assert read.rows == [[20.0, 0.01, 1.5, 1.5, 3.0, 6.0],
                         [2000.0, 0.01, None, None, None, None]]
    # the units in the headings, as the file has no comment line saying them
    assert (read.x_unit, read.y_unit) == ('Hz', 'g^2/Hz')
    assert read.warnings == [("Left out the column 'Note': it is not a column of a random "
                              'profile.')]


def test_a_breakpoint_file_with_no_heading_row_reads_by_position(tmp_path):
    path = tmp_path / 'target.txt'
    path.write_text('# a target\n100\t10\n2000\t1000\n', encoding='utf-8')
    read = t.read(path, 'breakpoints', 'srs_target')
    assert (read.columns, read.rows) == (['freq_hz', 'accel_g'],
                                         [[100.0, 10.0], [2000.0, 1000.0]])
    assert 'no heading row' in read.warnings[0]


def test_a_sine_profile_reads_its_quantities_by_word(tmp_path):
    path = tmp_path / 'sine.csv'
    path.write_text('freq,quantity,amplitude\n5,displacement,0.5\n20,velocity,2\n'
                    '100,g,1\n', encoding='utf-8')
    read = t.read(path, 'breakpoints', 'sine_profile')
    assert [r[1] for r in read.rows] == ['disp_pp_in', 'velocity_ips', 'accel_g']


@pytest.mark.parametrize('suffix', FORMS)
def test_a_file_of_the_other_kind_is_refused(tmp_path, suffix):
    path = t.write(tmp_path / f'profile{suffix}', profile())
    with pytest.raises(t.TableFileError, match='holds a breakpoint table, not a channel table'):
        t.read(path, 'channels')
    path = t.write(tmp_path / f'table{suffix}', channels())
    with pytest.raises(t.TableFileError, match='holds a channel table, not a'):
        t.read(path, 'breakpoints', 'random_profile')


@pytest.mark.parametrize('suffix', FORMS)
def test_a_breakpoint_table_of_another_kind_is_refused(tmp_path, suffix):
    path = t.write(tmp_path / f'profile{suffix}', profile())
    with pytest.raises(t.TableFileError, match='holds a random profile, not a sine profile'):
        t.read(path, 'breakpoints', 'sine_profile')
    with pytest.raises(t.TableFileError, match='not an SRS target'):
        t.read(path, 'breakpoints', 'srs_target')


#: a Sine limit channel's bound, as the shell saves one
BOUND = json.loads((GOLDEN / 'request_tables_save_bound.json').read_text(encoding='utf-8'))[
    'params']


def bound():
    return t.Table('breakpoints', list(BOUND['columns']), [list(r) for r in BOUND['rows']],
                   table_kind='sine_limit', x_unit='Hz', y_unit='')


def gain():
    return t.Table('breakpoints', ['freq_hz', 'gain'], [[50.0, 0.8], [400.0, 4.0]],
                   table_kind='drive_gain')


@pytest.mark.parametrize('suffix', FORMS)
@pytest.mark.parametrize('make', (bound, gain), ids=('sine_limit', 'drive_gain'))
def test_a_sine_bound_and_a_drive_gain_read_back_as_saved(tmp_path, suffix, make):
    table = make()
    path = t.write(tmp_path / f'table{suffix}', table)
    read = t.read(path, 'breakpoints', table.table_kind)
    expected = [[float(v) if isinstance(v, int) else v for v in row] for row in table.rows]
    assert (read.columns, read.rows, read.warnings) == (table.columns, expected, [])
    units = {'sine_limit': ('Hz', ''), 'drive_gain': ('Hz', 'g/V')}[table.table_kind]
    assert (read.x_unit, read.y_unit) == units
    assert t.read(path, 'breakpoints').table_kind == table.table_kind


def test_every_kind_of_breakpoint_table_has_its_label():
    assert t.table_kinds() == {
        'random_profile': 'Random profile', 'sine_profile': 'Sine profile',
        'sine_limit': 'Sine limit', 'srs_target': 'SRS target', 'limit': 'Limit',
        'drive_gain': 'Drive gain', 'mixed_tones': 'Tones table',
        'mixed_bands': 'Narrowbands table'}


@pytest.mark.parametrize('suffix', FORMS)
def test_a_sine_bound_is_not_a_sine_profile(tmp_path, suffix):
    path = t.write(tmp_path / f'bound{suffix}', bound())
    with pytest.raises(t.TableFileError, match='holds a sine limit, not a sine profile'):
        t.read(path, 'breakpoints', 'sine_profile')
    path = t.write(tmp_path / f'gain{suffix}', gain())
    with pytest.raises(t.TableFileError, match='holds a drive gain, not a limit'):
        t.read(path, 'breakpoints', 'limit')


def test_a_bound_and_a_gain_are_read_by_their_aliases(tmp_path):
    path = tmp_path / 'bound.csv'
    path.write_text('Frequency (Hz),Quantity,Limit\n10,displacement,0.2\n100,g,4\n',
                    encoding='utf-8')
    read = t.read(path, 'breakpoints', 'sine_limit')
    assert (read.columns, read.rows) == (['freq_hz', 'quantity', 'bound'],
                                         [[10.0, 'disp_pp_in', 0.2], [100.0, 'accel_g', 4.0]])
    path = tmp_path / 'gain.txt'
    path.write_text('Freq\tMagnitude (g/V)\n50\t0.8\n2000\t1.5\n', encoding='utf-8')
    read = t.read(path, 'breakpoints', 'drive_gain')
    assert (read.columns, read.rows, read.y_unit) == (['freq_hz', 'gain'],
                                                      [[50.0, 0.8], [2000.0, 1.5]], 'g/V')


@pytest.mark.parametrize('suffix', FORMS)
def test_units_a_file_does_not_say_are_the_ones_it_is_opened_into(tmp_path, suffix):
    plain = tmp_path / 'plain.csv'
    plain.write_text('freq,psd\n20,0.01\n2000,0.01\n', encoding='utf-8')
    read = t.read(plain, 'breakpoints', 'random_profile', x_unit='Hz', y_unit='lbf²/Hz')
    assert (read.x_unit, read.y_unit) == ('Hz', 'lbf²/Hz')
    assert t.read(plain, 'breakpoints', 'random_profile').y_unit == 'g²/Hz'
    # a file that says its units keeps them, in every form
    table = profile()
    table.x_unit, table.y_unit = 'Hz', 'g²/Hz'
    path = t.write(tmp_path / f'profile{suffix}', table)
    read = t.read(path, 'breakpoints', 'random_profile', x_unit='kHz', y_unit='lbf²/Hz')
    assert (read.x_unit, read.y_unit) == ('Hz', 'g²/Hz')
    # and a unit in a heading is the file saying it
    headed = tmp_path / 'headed.csv'
    headed.write_text('Frequency (kHz),PSD (g^2/Hz)\n0.02,0.01\n', encoding='utf-8')
    read = t.read(headed, 'breakpoints', 'random_profile', x_unit='Hz', y_unit='lbf²/Hz')
    assert (read.x_unit, read.y_unit) == ('kHz', 'g^2/Hz')


def test_a_channel_table_is_opened_without_units(tmp_path):
    path = t.write(tmp_path / 'channels.csv', channels())
    with pytest.raises(t.TableContentError, match='no x_unit or y_unit'):
        t.read(path, 'channels', x_unit='Hz')


def test_a_project_or_another_object_is_not_a_table(tmp_path):
    hz.Project('Survey').save(tmp_path / 'survey.hforge')
    with pytest.raises(t.TableFileError, match='holds a Hertzforge project, not a channel table'):
        t.read(tmp_path / 'survey.hforge', 'channels')
    (tmp_path / 'notes.hforge').write_bytes(b'not hdf5 at all')
    with pytest.raises(t.TableFileError, match='not a readable .hforge file'):
        t.read(tmp_path / 'notes.hforge', 'channels')


def test_a_breakpoint_file_that_does_not_say_its_kind_needs_one_given(tmp_path):
    path = tmp_path / 'points.csv'
    path.write_text('freq,psd\n20,0.01\n', encoding='utf-8')
    with pytest.raises(t.TableFileError, match='does not say which breakpoint table'):
        t.read(path, 'breakpoints')
    assert t.read(path, 'breakpoints', 'random_profile').rows == [[20.0, 0.01]]


def test_a_breakpoint_file_without_its_levels_is_refused(tmp_path):
    path = tmp_path / 'sine.csv'
    path.write_text('freq,quantity,value\n5,g,1\n', encoding='utf-8')
    with pytest.raises(t.TableFileError, match='no PSD or Slope column'):
        t.read(path, 'breakpoints', 'random_profile')
    path.write_text('psd\n0.01\n', encoding='utf-8')
    with pytest.raises(t.TableFileError, match='no Frequency column'):
        t.read(path, 'breakpoints', 'random_profile')


@pytest.mark.parametrize(('kind', 'text', 'words'), [
    ('channels', '', 'has no channel table in it'),
    ('channels', '# only a comment\n\n', 'has no channel table in it'),
    ('channels', 'Serial,Owner\nA1,Lab\n', 'has no channel table in it'),
    ('channels', 'Channel,Comment\n1,"open quote\n', 'not readable as comma-delimited text'),
    ('breakpoints', '', 'no heading row and no rows'),
    ('breakpoints', 'freq,psd\n20,abc\n', "Row 2, PSD: 'abc' is not a number"),
    ('breakpoints', 'freq,psd,slope\n20,0.01,steep\n', "Row 2, Slope: 'steep' is not a number"),
])
def test_an_empty_or_malformed_file_is_refused_in_words(tmp_path, kind, text, words):
    path = tmp_path / 'bad.csv'
    path.write_text(text, encoding='utf-8')
    with pytest.raises(t.TableFileError, match=words):
        t.read(path, kind, 'random_profile' if kind == 'breakpoints' else None)


def test_a_heading_row_with_no_rows_reads_as_an_empty_table(tmp_path):
    path = tmp_path / 'empty.csv'
    path.write_text('Channel,Comment\n', encoding='utf-8')
    read = t.read(path, 'channels')
    assert (read.rows, read.warnings) == ([], [converted('empty.csv'), 'empty.csv has no rows.'])


def test_a_broken_workbook_a_missing_file_and_another_suffix_are_refused(tmp_path):
    (tmp_path / 'broken.xlsx').write_bytes(b'PK not a workbook')
    for kind, table_kind in (('channels', None), ('breakpoints', 'limit')):
        with pytest.raises(t.TableFileError, match='not a readable Excel workbook'):
            t.read(tmp_path / 'broken.xlsx', kind, table_kind)
    with pytest.raises(FileNotFoundError, match='no file at'):
        t.read(tmp_path / 'absent.csv', 'channels')
    with pytest.raises(t.TableFileError, match=r'a table file is \.hforge, \.csv'):
        t.read(tmp_path / 'table.xls', 'channels')
    with pytest.raises(t.TableFileError, match='a table file is'):
        t.write(tmp_path / 'table.json', channels())


def channel(columns, *rows):
    return t.Table('channels', columns, [list(r) for r in rows])


@pytest.mark.parametrize(('table', 'words'), [
    (channel(['channel', 'serial'], [1, 'A']), "'serial' is not a column of a channel table"),
    (channel(['comment'], ['x']), 'has a Channel column'),
    (channel(['channel'], [1.5]), 'row 1, Channel: 1.5 is not a whole number'),
    (channel(['channel'], [0]), 'row 1, Channel: channel must be positive'),
    (channel(['channel'], [1], [1]), 'channel 1 is in the table twice'),
    (channel(['channel', 'enabled'], [1, 'yes']), "row 1, Enabled: 'yes' is not true or false"),
    (channel(['channel', 'sensitivity'], [1, 0]), 'sensitivity must be greater than 0'),
    (channel(['channel', 'range_max'], [1, float('nan')]), 'not a finite number'),
    (channel(['channel', 'polarity'], [1, 'reversed']), 'polarity is one of positive'),
    (channel(['channel', 'unit'], [1, 'wibbles per fortnight squared']), 'row 1, Unit'),
    (channel(['channel', 'unit'], [1]), 'row 1 has 1 value, not one for each'),
    (channel(['channel', 'channel'], [1, 1]), 'a column is named twice'),
    (t.Table('channels', ['channel'], [[1]], x_unit='Hz'), 'no table_kind or units'),
    (t.Table('breakpoints', ['freq_hz'], [[1.0]], table_kind='shock_pulse'),
     "'shock_pulse' is not a kind of breakpoint table"),
    (t.Table('breakpoints', ['psd_g2_hz'], [[1.0]], table_kind='random_profile'),
     'has a Frequency column'),
    (t.Table('breakpoints', ['freq_hz', 'quantity'], [[1.0, 'g']], table_kind='sine_profile'),
     "row 1, Quantity: 'g' is not one of accel_g"),
    (t.Table('tables', ['index'], []), 'the kind of table is one of'),
])
def test_writing_is_strict(tmp_path, table, words):
    with pytest.raises(t.TableContentError, match=words):
        t.write(tmp_path / 'out.csv', table)
    assert not list(tmp_path.iterdir())


def test_a_failed_write_leaves_the_file_as_it_was(tmp_path):
    path = t.write(tmp_path / 'table.csv', channels())
    before = (tmp_path / 'table.csv').read_bytes()
    with pytest.raises(t.TableContentError):
        t.write(path, channel(['channel', 'enabled'], [1, 'on']))
    assert (tmp_path / 'table.csv').read_bytes() == before
    with pytest.raises(t.TableFileError, match='could not be written'):
        t.write(tmp_path / 'no such folder' / 'table.csv', channels())


def test_opening_a_breakpoint_file_as_a_project_says_what_it_is(tmp_path):
    path = t.write(tmp_path / 'profile.hforge', profile())
    words = ('holds a breakpoint table. A table file is opened from the table it belongs to, '
             'not opened or imported as a project.')
    with pytest.raises(ValueError, match=words):
        hz.Project.open(path)
    with pytest.raises(ValueError, match='holds a breakpoint table'):
        hz.Project('Survey').import_file(path)


def _left_out(key):
    return f'Left out the column {key!r}: it is not a column of a channel table.'


def test_converting_adds_the_controller_settings_once_and_changes_nothing_else():
    table = measurement_table()
    before = table.frame.copy()
    assert not table.has_controller_settings
    table.add_controller_settings()
    assert table.has_controller_settings
    assert table.column_names == list(ChannelTable.COLUMNS)
    assert table.frame[before.columns].equals(before)
    # started from what the table says: on, no drive, and each full scale
    # the clip level over the sensitivity (5 V at 10.2 mV/g; 0.5 V at 2.25 mV/lbf)
    assert list(table['enabled']) == ['True', 'True']
    assert list(table['drive']) == ['False', 'False']
    assert list(table['range_max']) == ['490.196', '222.222']
    once = table.frame.copy()
    table.add_controller_settings()
    assert table.frame.equals(once)
    table.remove_controller_settings()
    assert table.frame.equals(before) and not table.has_controller_settings
    table.remove_controller_settings()
    assert table.frame.equals(before)


def test_a_controller_setting_is_edited_only_on_a_converted_table():
    table = measurement_table()
    with pytest.raises(KeyError, match='add the controller settings first'):
        table.set_cell('polarity', 0, 'negative')
    table.add_controller_settings()
    table.set_cell('polarity', 0, 'negative')
    with pytest.raises(ValueError, match='polarity is one of positive'):
        table.set_cell('polarity', 0, 'backwards')
    assert table['polarity'][0] == 'negative'
    assert table.add_channels(1) == [3]
    assert table['enabled'][2] == 'True'


def test_the_analysis_view_shows_the_controller_columns_only_on_a_converted_table():
    from hertzforge.core.channel_table import title_of
    from hertzforge.core.tables import table_of

    table = measurement_table()
    headers, _rows = table_of(table)
    assert headers == [title_of(name) for name in ChannelTable.SCHEMA]
    table.add_controller_settings()
    headers, rows = table_of(table)
    assert headers[len(ChannelTable.SCHEMA):] == [
        'Enabled', 'Drive', 'Physical', 'IEPE', 'IEPE Bias (V)', 'Polarity',
        'Range Max (Unit)', 'Control Group', 'Abort Accel (g)', 'Abort Velocity (in/s)',
        'Abort Displacement (mil)']
    assert rows[0][len(ChannelTable.SCHEMA):] == ['True', 'False', '', 'False', '', '',
                                                  '490.196', '', '', '', '']
