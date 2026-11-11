"""Mixed Mode's parts tables as files: a tones table and a narrowbands table
written in every form and read back with every column (text, choices and
numbers, each kind of schedule, a tone's amplitude profile), headings read by
their aliases, and a file of another kind refused."""
from __future__ import annotations

import pytest

from hertzforge.io import tables as t

FORMS = ('.hforge', '.csv', '.tsv', '.xlsx')

TONE_COLUMNS = ['id', 'name', 'enabled', 'schedule', 'freq_hz', 'dwell_s', 'dwell_cycles',
                'lo_hz', 'hi_hz', 'rate_law', 'rate', 'start_hz', 'direction', 'end_sweeps',
                'end_s', 'follows', 'ratio', 'alarm_below_db', 'alarm_above_db',
                'abort_below_db', 'abort_above_db', 'on_s', 'off_s', 'amplitude_g', 'profile']


def tones():
    rows = [
        ['t1', 'Blade pass', 'on', 'fixed', 120.0, 60.0, None, None, None, None, None, None,
         None, None, None, None, None, 1.5, 1.5, 3.0, 3.0, 0.0, None, 2.0, None],
        ['t2', 'Rotor', 'on', 'sweep', None, None, None, 300.0, 600.0, 'log', 1.0, 300.0, 'up',
         4.0, None, None, None, 1.5, 1.5, 3.0, 3.0, 0.0, None, None,
         '300 accel_g 1; 600 accel_g 1.5'],
        ['t3', 'Rotor, 2nd', 'off', 'follow', None, None, None, None, None, None, None, None,
         None, None, None, 't2', 2.0, 1.0, 2.0, 3.0, 4.0, 10.0, 250.0, 0.5, None],
    ]
    return t.Table('breakpoints', TONE_COLUMNS, rows, table_kind='mixed_tones')


def bands():
    columns = ['id', 'name', 'enabled', 'schedule', 'freq_hz', 'lo_hz', 'hi_hz', 'rate_law',
               'rate', 'direction', 'alarm_below_db', 'alarm_above_db', 'abort_below_db',
               'abort_above_db', 'on_s', 'off_s', 'level_g2_hz', 'width_hz', 'width_share']
    rows = [
        ['b1', 'Gear mesh', 'on', 'fixed', 250.0, None, None, None, None, None, None, None,
         None, None, 0.0, None, 0.2, 20.0, None],
        ['b2', 'Rotor band', 'on', 'sweep', None, 1300.0, 1800.0, 'linear', 2.0, 'down', 3.0,
         3.0, 6.0, 6.0, 10.0, 250.0, 0.1, None, 0.05],
    ]
    return t.Table('breakpoints', columns, rows, table_kind='mixed_bands')


@pytest.mark.parametrize('suffix', FORMS)
@pytest.mark.parametrize('make', (tones, bands), ids=('mixed_tones', 'mixed_bands'))
def test_a_parts_table_reads_back_with_every_column(tmp_path, suffix, make):
    table = make()
    path = t.write(tmp_path / f'parts{suffix}', table)
    read = t.read(path, 'breakpoints', table.table_kind)
    assert (read.columns, read.rows, read.warnings) == (table.columns, table.rows, [])
    assert t.read(path, 'breakpoints').table_kind == table.table_kind


def test_a_parts_table_is_read_by_its_words(tmp_path):
    path = tmp_path / 'tones.csv'
    path.write_text('Part,Label,Starts on,Kind,Frequency (Hz),Amp\n'
                    't9,Fan,yes,stands still,95,0.4\n', encoding='utf-8')
    read = t.read(path, 'breakpoints', 'mixed_tones')
    assert read.columns == ['id', 'name', 'enabled', 'schedule', 'freq_hz', 'amplitude_g']
    assert read.rows == [['t9', 'Fan', 'on', 'fixed', 95.0, 0.4]]


@pytest.mark.parametrize('suffix', ('.hforge', '.csv'))
def test_a_tones_file_is_not_a_narrowbands_table(tmp_path, suffix):
    path = t.write(tmp_path / f'tones{suffix}', tones())
    with pytest.raises(t.TableFileError, match='tones table'):
        t.read(path, 'breakpoints', 'mixed_bands')


def test_a_part_with_a_number_for_a_name_is_refused_before_writing(tmp_path):
    table = tones()
    table.rows[0][1] = 7.0
    with pytest.raises(t.TableContentError, match='Name: 7.0 is not text'):
        t.write(tmp_path / 'tones.csv', table)
