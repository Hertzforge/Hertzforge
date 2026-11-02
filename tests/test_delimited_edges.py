"""The edges of the delimited-text reader, each one a boundary a
one-character change to the reader would move.

Written from a mutation run over `io/delimited.py` (`tools/mutate.py`):
every test here fails on a change the main tests let through, and pins
the boundary by its value rather than by "something was read".
"""

from __future__ import annotations

import numpy as np
import pytest
from test_delimited_import import FS, G, _csv, _read, _signal, _text, _write

from hertzforge.core.data import TimeHistory
from hertzforge.io import delimited
from hertzforge.io.delimited import read_delimited, text_layout


def _rows(t, *columns, fmt='.17g', delimiter=','):
    return [delimiter.join(format(v, fmt) for v in row)
            for row in zip(t, *columns, strict=True)]


def test_a_head_read_holds_exactly_the_lines_asked_for(tmp_path):
    path, _ = _csv(tmp_path, samples=200)
    head = delimited._read_lines(path, lines=5)
    assert head == _text(path).splitlines()[:5]
    assert delimited._read_lines(path, lines=10_000) == _text(path).splitlines()


def test_a_read_cut_mid_character_is_still_utf8(tmp_path):
    """The layout reads a fixed number of bytes; a two-byte character
    the cut falls inside is not a sign of a foreign encoding."""
    path = _write(tmp_path, 'Run µ1\nabcµ\n')
    cut = len('Run µ1\nabc'.encode()) + 1          # half of the second µ
    assert delimited._read_lines(path, limit=cut) == ['Run µ1']
    assert delimited._read_lines(path, limit=cut - 1) == ['Run µ1'], (
        'a whole character, the line still cut short')
    legacy = tmp_path / 'legacy.csv'
    legacy.write_bytes('Run µ1\nabc\n'.encode('cp1252'))
    assert delimited._read_lines(legacy)[0] == 'Run µ1'


def test_a_large_file_is_read_a_megabyte_at_a_time(tmp_path):
    t, a, _f = _signal(60000)
    path = _write(tmp_path, 'Time (s),A\n' + '\n'.join(_rows(t, a)))
    assert delimited._read_lines(path, lines=50000)[-1] == _rows(t, a)[49998]
    assert len(delimited._read_lines(path, lines=50000)) == 50000


def test_a_file_of_a_header_and_two_rows_is_enough(tmp_path):
    path = _write(tmp_path, 'Time (s),A\n0,1\n0.5,2\n')
    history, _notes = _read(path)
    assert history.sample_rate == 2.0 and history.ordinate.tolist() == [[1.0, 2.0]]
    with pytest.raises(ValueError, match='holds 1 data row; a time history needs two'):
        read_delimited(path, max_rows=1)
    with pytest.raises(ValueError, match='holds 0 data rows'):
        read_delimited(path, max_rows=0)
    assert read_delimited(path, max_rows=2).ordinate.shape == (1, 2)


def test_one_column_of_numbers_split_by_nothing(tmp_path):
    _t, a, _f = _signal(32)
    path = _write(tmp_path, '\n'.join(format(v, '.17g') for v in a), name='one.txt')
    layout = text_layout(path)
    assert layout.delimiter == ' ' and layout.columns == ('column 1',)
    history, _notes = _read(path, sample_rate=1000)
    assert history.ordinate.shape == (1, 32)


def test_prose_with_a_few_numbers_at_its_end_is_not_data(tmp_path):
    """Most of the last lines must be numbers for them to be the data."""
    lines = [f'note {k}: the run was fine, nothing to add' for k in range(10)]
    numbers = [f'{k},{k + 1}' for k in range(7)]
    path = _write(tmp_path, '\n'.join(lines + numbers), name='notes.txt')
    assert text_layout(path).data_line == 11, '7 of the last 12'
    words = _write(tmp_path, '\n'.join(lines + numbers[:5]), name='words.txt')
    assert not delimited.sniff(words), '5 of the last 12'
    few = _write(tmp_path, '\n'.join(lines[:3] + numbers[:3]), name='few.txt')
    assert text_layout(few).data_line == 4, '3 of 6'


def test_a_row_ending_in_empty_fields_is_not_too_long(tmp_path):
    t, a, f = _signal(8)
    rows = _rows(t, a, f)
    rows[3] += ',,'
    path = _write(tmp_path, 'Time (s),A,B\n' + '\n'.join(rows))
    history, _notes = _read(path)
    assert history.ordinate.shape == (2, 8)


def test_a_header_without_the_trailing_delimiter_its_rows_have(tmp_path):
    t, a, _f = _signal(16)
    rows = [row + ',' for row in _rows(t, a)]
    path = _write(tmp_path, 'Time (s),Accel (g)\n' + '\n'.join(rows))
    layout = text_layout(path)
    assert layout.columns == ('Time', 'Accel') and layout.names_line == 1


def test_a_stated_data_line_finds_the_names_above_it(tmp_path):
    t, a, f = _signal(16)
    text = 'Run 4\nTime (s),A (g),B\n' + '\n'.join(_rows(t, a, f))
    path = _write(tmp_path, text)
    layout = text_layout(path, data_line=3)
    assert (layout.names_line, layout.data_line) == (2, 3)
    with pytest.raises(ValueError, match='line 2 does not split into numbers'):
        text_layout(path, data_line=2, delimiter=',')
    for line in (0, 99):
        with pytest.raises(ValueError, match=f'data_line {line} is past the 18 lines'):
            text_layout(path, data_line=line)


def test_a_stated_units_line_takes_the_names_from_above_it(tmp_path):
    t, a, f = _signal(16)
    text = 'Time,A,B\nseconds,g,lbf\n' + '\n'.join(_rows(t, a, f))
    path = _write(tmp_path, text)
    layout = text_layout(path, units_line=2)
    assert (layout.names_line, layout.units_line) == (1, 2)
    assert layout.units == ('seconds', 'g', 'lbf')
    bare = text_layout(path, units_line=0)
    assert bare.units_line == 0 and bare.names_line == 2, 'the nearest line then'


def test_a_names_line_on_the_data_is_refused(tmp_path):
    path, _ = _csv(tmp_path)
    with pytest.raises(ValueError, match='names_line 2 is not above the data, which '
                                         'start on line 2'):
        text_layout(path, names_line=2)


def test_a_stated_decimal_comma_reads_without_a_delimiter_stated(tmp_path):
    t, a, _f = _signal(16)
    rows = [row.replace('.', ',') for row in _rows(t, a, fmt='.9f', delimiter=';')]
    path = _write(tmp_path, 'Time (s);A\n' + '\n'.join(rows))
    assert text_layout(path, decimal=',').delimiter == ';'


def test_an_unnamed_first_column_that_does_not_rise_evenly_is_a_channel(tmp_path):
    rng = np.random.default_rng(2)
    a, b = rng.standard_normal((2, 32))
    path = _write(tmp_path, '\n'.join(_rows(a, b)))
    assert text_layout(path).time_column is None
    uneven = np.cumsum(rng.uniform(0.5, 1.5, 32))
    jagged = _write(tmp_path, '\n'.join(_rows(uneven, b)), name='jagged.csv')
    assert text_layout(jagged).time_column is None
    falling = _write(tmp_path, '\n'.join(_rows(-np.arange(32.0), b)), name='down.csv')
    assert text_layout(falling).time_column is None
    two = _write(tmp_path, '0,1\n1,2\n', name='two.csv')
    assert text_layout(two).time_column is None, 'two rows show no step'
    lone = _write(tmp_path, '\n'.join(f'{k}' for k in range(8)), name='lone.csv')
    assert text_layout(lone).time_column is None, 'a lone column is the data'
    gap = _write(tmp_path, '0,1\n1,2\nnan,3\n3,4\n', name='gap.csv')
    assert text_layout(gap).time_column is None


def test_a_repeated_time_is_out_of_order(tmp_path):
    t, a, _f = _signal(16)
    t[6] = t[5]
    path = _write(tmp_path, 'Time (s),A\n' + '\n'.join(_rows(t, a)))
    with pytest.raises(ValueError, match='in order; sample 6') as refused:
        read_delimited(path)
    # refused as out of order, not as a rounding its digits cannot explain
    assert 'digits' not in str(refused.value)


def test_rounding_is_forgiven_to_a_unit_of_the_last_digit_and_no_further(tmp_path):
    t, a, _f = _signal(400)
    path = _write(tmp_path, 'Time (s),A\n' + '\n'.join(_rows(t, a, fmt='.6f')))
    assert _read(path)[0].sample_rate == pytest.approx(FS, rel=1e-5)
    late = np.round(t, 6)
    late[200] += 3e-6                       # three units of the last digit
    path = _write(tmp_path, 'Time (s),A\n' + '\n'.join(
        _rows(late, a, fmt='.6f')), name='late.csv')
    with pytest.raises(ValueError, match='more than the digits it is written to'):
        read_delimited(path)


def test_rounding_read_from_exponent_and_quoted_times(tmp_path):
    t, a, _f = _signal(300)
    rows = [f'"{x:.5e}",{y:.17g}' for x, y in zip(t, a, strict=True)]
    path = _write(tmp_path, 'Time (s),A\n' + '\n'.join(rows))
    history, notes = _read(path)
    assert history.sample_rate == pytest.approx(FS, rel=1e-5)
    assert any('rounded when it was written' in n for n in notes)
    comma = [f'{x:.6f};{y:.6f}'.replace('.', ',') for x, y in zip(t, a, strict=True)]
    path = _write(tmp_path, 'Time (s);A\n' + '\n'.join(comma), name='comma.csv')
    assert _read(path)[0].sample_rate == pytest.approx(FS, rel=1e-5)


def test_a_stated_rate_inside_its_tolerance_is_the_rate_used(tmp_path):
    path, _ = _csv(tmp_path)
    assert _read(path, sample_rate=5120.4)[0].sample_rate == pytest.approx(5120.4)
    with pytest.raises(ValueError, match='one of them is wrong'):
        read_delimited(path, sample_rate=5121.0)


def test_a_stated_rate_is_taken_over_the_preamble(tmp_path):
    path, _ = _csv(tmp_path, header='A,B,C', preamble='Sample Rate: 10240\n')
    assert _read(path)[0].sample_rate == 10240.0
    assert _read(path, sample_rate=5120)[0].sample_rate == 5120.0
    timed, _ = _csv(tmp_path, preamble='Sample Rate: 10240\n', name='timed.csv')
    assert _read(timed, sample_rate=5120)[0].sample_rate == 5120.0, (
        'the preamble wrong, the stated rate and the column agreeing')


@pytest.mark.parametrize('rate', [0.0, -5.0, float('nan'), float('inf')])
def test_a_rate_that_is_not_a_positive_number_is_refused(tmp_path, rate):
    path, _ = _csv(tmp_path)
    with pytest.raises(ValueError, match='sample_rate must be a positive number'):
        read_delimited(path, sample_rate=rate)


@pytest.mark.parametrize('line, rate, at', [
    ('Sample Rate: 0\nfs = 2000', 2000.0, 2),
    ('dt = 0.25\n', 4.0, 1), ('Note: none\nSample Interval: 0.2 ms', 5000.0, 2)])
def test_the_preamble_rate_and_its_line(tmp_path, line, rate, at):
    path, _ = _csv(tmp_path, header='A,B,C', preamble=line + '\n')
    layout = text_layout(path)
    assert (layout.sample_rate, layout.rate_line) == (pytest.approx(rate), at)


def test_a_column_index_out_of_range_is_refused(tmp_path):
    path, _ = _csv(tmp_path)
    for key in (-1, 3):
        with pytest.raises(ValueError, match=f'columns names {key} is not a column: '
                                             'the file has 3'):
            read_delimited(path, columns=[key])
    assert _read(path, columns=[2])[0].block == ['Force']
    assert _read(path, columns=[1, 1])[0].block == ['Accel'], 'once each'


def test_the_first_missing_sample_in_the_file_is_the_one_refused(tmp_path):
    t, a, f = _signal(8)
    rows = _rows(t, a, f)
    rows[5] = rows[5].rsplit(',', 2)[0] + ',,' + rows[5].rsplit(',', 1)[1]  # A
    rows[2] = rows[2].rsplit(',', 1)[0] + ','                                # B
    path = _write(tmp_path, 'Time (s),A,B\n' + '\n'.join(rows))
    with pytest.raises(ValueError, match='line 4: B has a missing sample'):
        read_delimited(path, missing='refuse')


def test_a_record_with_no_label_is_written_by_its_number(tmp_path):
    t, a, _f = _signal(16)
    history = TimeHistory(t, a, response_dof=[''])
    target = str(tmp_path / 'out.csv')
    delimited.save(history, target)
    assert _text(target).splitlines()[1] == 'time (s),record 1'
    back, _notes = _read(target)
    assert back.block == ['record 1']
    assert np.array_equal(back.ordinate, history.ordinate)


def test_a_head_read_ending_on_a_chunk_boundary_does_not_cut_its_last_line(
        tmp_path):
    """The head is read a megabyte at a time. When the first chunk holds
    exactly the lines asked for, it ends partway into the next line, so
    the next chunk is read before the head is cut at a newline."""
    path = tmp_path / 'long.txt'
    path.write_bytes((b'x' * 99 + b'\n') * 20000)
    asked = (1 << 20) // 100                   # the newlines in the first chunk
    head = delimited._read_lines(path, lines=asked)
    assert len(head) == asked and head[-1] == 'x' * 99


def test_a_head_read_counts_an_empty_line_as_a_line(tmp_path):
    path = tmp_path / 'blank.txt'
    path.write_bytes(b'a\n\nb\nc\nd\n')
    assert delimited._read_lines(path, lines=3) == ['a', '', 'b']


def test_a_cut_four_byte_character_is_utf8_and_a_foreign_byte_is_not(tmp_path):
    """A character of four bytes cut after three is the cut, not a
    foreign encoding; a byte that is not UTF-8 four bytes before the cut
    cannot be part of a cut character, so the text is Windows-1252."""
    cut = tmp_path / 'cut.txt'
    face = '\N{GRINNING FACE}'.encode()
    cut.write_bytes('\N{MICRO SIGN}\ncd'.encode() + face[:3] + b'\n')
    assert delimited._read_lines(
        cut, limit=len('\N{MICRO SIGN}\ncd'.encode()) + 3) == ['\N{MICRO SIGN}']
    legacy = tmp_path / 'legacy.txt'
    legacy.write_bytes(b'\xe9a\nbcd\n')
    assert delimited._read_lines(legacy, limit=4) == [
        '\N{LATIN SMALL LETTER E WITH ACUTE}a']


def test_a_numeric_line_above_the_header_is_not_where_the_data_start(tmp_path):
    t, a, f = _signal(32)
    path = _write(tmp_path, '1,2,3\nTime (s),A,B\n' + '\n'.join(_rows(t, a, f)))
    layout = text_layout(path)
    assert (layout.names_line, layout.data_line) == (2, 3)


@pytest.mark.parametrize('line, rate', [
    ('Sample Rate (kHz): 5.12', 5120.0),
    ('Sample Rate: 5120 S/s', 5120.0),
    ('Sample Interval: 1.953125e-4 sec/sample', 5120.0)])
def test_a_word_after_the_rate_is_a_unit_only_when_it_reads_as_one(
        tmp_path, line, rate):
    path, _ = _csv(tmp_path, header='A,B,C', preamble=line + '\n')
    assert text_layout(path).sample_rate == pytest.approx(rate)


def test_a_headerless_file_of_two_columns_and_three_rows(tmp_path):
    """The smallest file whose first column is taken as time: two
    columns, three rows; and a stated data line may be the first line
    or the last."""
    path = _write(tmp_path, '0,1\n0.5,2\n1.0,3\n')
    layout = text_layout(path)
    assert layout.time_column == 0 and layout.columns == ('column 1', 'column 2')
    assert text_layout(path, data_line=1).data_line == 1
    assert text_layout(path, data_line=3).data_line == 3


def test_a_stated_data_line_takes_two_fields_or_one_split_by_spaces(tmp_path):
    two = _write(tmp_path, 'Time,A\n0,1\n0.5,2\n', name='two.csv')
    assert text_layout(two, data_line=2).delimiter == ','
    one = _write(tmp_path, 'A\n1\n2\n3\n', name='one.txt')
    assert text_layout(one, data_line=2).delimiter == ' '


def test_a_headerless_file_keeps_every_column(tmp_path):
    path = _write(tmp_path, '0,1,2\n0.5,2,3\n1.0,3,4\n')
    assert text_layout(path).columns == ('column 1', 'column 2', 'column 3')


def test_one_column_written_with_a_trailing_delimiter(tmp_path):
    path = _write(tmp_path, 'a\n0.1,\n0.2,\n0.3,\n')
    history, _notes = _read(path, sample_rate=10.0)
    assert history.num_records == 1 and history.block == ['a']


def test_a_title_line_is_not_the_header_of_a_trailing_delimiter_file(tmp_path):
    path = _write(tmp_path, 'Run 12\n0,1,\n0.5,2,\n1.0,3,\n')
    layout = text_layout(path)
    assert layout.names_line == 0
    assert layout.columns == ('column 1', 'column 2')


def test_a_word_in_an_unnamed_first_column_is_refused_by_line(tmp_path):
    path = _write(tmp_path, '0,1\n0.5,2\nabc,3\n1.5,4\n2.0,5\n2.5,6\n')
    with pytest.raises(ValueError, match="line 3: column 1 holds 'abc'"):
        read_delimited(path, sample_rate=10.0)


def test_a_constant_unnamed_first_column_is_a_channel(tmp_path):
    path = _write(tmp_path, '1,1\n1,2\n1,3\n1,4\n')
    assert text_layout(path).time_column is None


def test_a_row_ending_in_empty_fields_is_read_cell_by_cell_too(tmp_path):
    """The cell-by-cell read (forced here by an empty cell) takes a row
    that ends in empty fields past the header's count, as the fast read
    does."""
    path = _write(tmp_path, 'Time,A,B\n0,1,2\n0.5,,3\n1.0,3,4,,\n1.5,4,5\n')
    history, _notes = _read(path)
    assert history.ordinate.shape == (2, 4)
    assert np.isnan(history.ordinate[0, 1])


def test_a_decimal_comma_time_column_uneven_beyond_its_digits_is_refused(tmp_path):
    t, a, _f = _signal(400)
    late = np.round(t, 6)
    late[200] += 5e-6
    rows = [f'{x:.6f};{y:.6f}'.replace('.', ',') for x, y in zip(late, a, strict=True)]
    path = _write(tmp_path, 'Time (s);A\n' + '\n'.join(rows))
    with pytest.raises(ValueError, match='more than the digits it is written to'):
        read_delimited(path)


def test_a_millisecond_time_column_is_forgiven_its_own_digits_only(tmp_path):
    t, a, _f = _signal(400)
    ms = np.round(t * 1000.0, 3)
    path = _write(tmp_path, 'Time (ms),A\n' + '\n'.join(_rows(ms, a, fmt='.3f')))
    assert _read(path)[0].sample_rate == pytest.approx(FS, rel=1e-5)
    ms[200] += 0.005                            # five units of the last digit
    late = _write(tmp_path, 'Time (ms),A\n' + '\n'.join(_rows(ms, a, fmt='.3f')),
                  name='late.csv')
    with pytest.raises(ValueError, match='more than the digits it is written to'):
        read_delimited(late)


def test_an_even_time_column_starting_late_keeps_its_start(tmp_path):
    path = _write(tmp_path, 'Time,A\n3.25,1\n3.5,2\n3.75,3\n4.0,4\n')
    history, _notes = _read(path)
    assert history.sample_rate == 4.0
    assert history.abscissa[0] == 3.25 and history.abscissa[-1] == 4.0


def test_a_sample_rate_below_one_per_second_is_a_rate(tmp_path):
    path = _write(tmp_path, 'A,B\n1,2\n3,4\n5,6\n')
    history, _notes = _read(path, time_column=False, sample_rate=0.5)
    assert history.abscissa.tolist() == [0.0, 2.0, 4.0]


def test_rows_missing_everything_at_the_end_are_not_samples(tmp_path):
    """Rows that hold no value at all (written as NaN, so not blank) end
    the data; a short row just before them is still a sample and still
    noted."""
    path = _write(tmp_path, 'Time,A,B\n0,1,2\n0.5,2,3\n1.0,3\nNaN,NaN,NaN\nNaN,,\n')
    history, notes = _read(path)
    assert history.ordinate.shape == (2, 3)
    assert ('run.csv: 1 row shorter than the header (the first on line 4): the '
            'cells missing from the end of a short row read as missing '
            'samples') in notes


@pytest.mark.parametrize('line, said', [
    ('Sample Rate: 5120 samples', ("line 1 states a sample rate in "
                                   "'samples', which is not a frequency")),
    ('Sample Rate: 5120 ms', ("line 1 states a sample rate in 'ms', which "
                              'is not a frequency')),
    ('dt = 0.2 Hz', "line 1 states a sample step in 'Hz', which is not a time")])
def test_a_preamble_rate_in_a_unit_of_something_else_is_refused(
        tmp_path, line, said):
    path, _ = _csv(tmp_path, header='A,B,C', preamble=line + '\n')
    layout = text_layout(path)
    assert layout.sample_rate is None and layout.rate_problem == said
    with pytest.raises(ValueError, match=f'^run.csv: {said}; state the '
                                         'sample_rate$'):
        read_delimited(path)
    history, _notes = _read(path, sample_rate=100.0)
    assert history.sample_rate == 100.0


def test_a_short_row_split_by_spaces_is_refused_by_line(tmp_path):
    """Split by spaces, a missing value leaves no gap to place it by."""
    path = _write(tmp_path, '% dt = 0.5 ms\n1 2 3\n4 5 6\n7 8\n9 10 11\n',
                  name='run.txt')
    with pytest.raises(ValueError, match='run.txt, line 4: 2 fields where the '
                                         'data above it have 3, and in a file '
                                         'split by spaces the missing ones '
                                         'cannot be placed'):
        read_delimited(path)


def test_a_name_that_matches_two_columns_is_refused(tmp_path):
    t, a, f = _signal(16)
    path = _write(tmp_path, 'Time (s),A (g),A (g)\n' + '\n'.join(_rows(t, a, f)))
    for kwargs in ({'columns': ['A']}, {'units': {'A': 'g'}},
                   {'time_column': 'A'}):
        with pytest.raises(ValueError, match="'A' names column 2 and column 3; "
                                             'name it by its index'):
            read_delimited(path, **kwargs)
    history, _notes = _read(path, columns=[2])
    assert np.allclose(history.ordinate[0], f * G)


def test_two_columns_named_for_time_are_refused_until_one_is_chosen(tmp_path):
    t, a, _f = _signal(16)
    rows = [f'{x:.17g},{1000 * x:.17g},{y:.17g}' for x, y in zip(t, a, strict=True)]
    path = _write(tmp_path, 'Time (s),Time (ms),A (g)\n' + '\n'.join(rows))
    layout = text_layout(path)
    assert layout.time_column is None and layout.time_candidates == (0, 1)
    with pytest.raises(ValueError, match='Time and Time are each named for '
                                         'time; say which is the clock'):
        read_delimited(path)
    history, _notes = _read(path, time_column=1, columns=[2])
    assert history.sample_rate == pytest.approx(FS)


def test_a_stated_unit_that_contradicts_the_header_is_refused(tmp_path):
    path, (_t, a, _f) = _csv(tmp_path, header='Time (s),Accel (g),Force [lbf]')
    with pytest.raises(ValueError, match="units: Accel is stated in 'm/s\\*\\*2' "
                                         "and headed in 'g'"):
        read_delimited(path, units={'Accel': 'm/s**2'})
    with pytest.raises(ValueError, match="Force is stated in 'N' and headed in "
                                         "'lbf'"):
        read_delimited(path, units={'Force': 'N'})
    with pytest.raises(ValueError, match="Accel is stated in 'N' and headed "
                                         "in 'g'"):
        read_delimited(path, units={'Accel': 'N'})
    # the header's own unit, however spelled, says nothing new
    history, _notes = _read(path, units={'Accel': 'g', 'Force': 'lbf'})
    assert np.allclose(history.ordinate[0], a * G)
    spelled, (_t, a, _f) = _csv(tmp_path, header='Time (s),Accel (m/s^2),F',
                                name='spelled.csv')
    history, _notes = _read(spelled, units={'Accel': 'm/s**2'})
    assert np.allclose(history.ordinate[0], a)
