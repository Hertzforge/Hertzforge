"""A time column's rounding is half a unit of the digit it printed; a dropped
row is not rounding, and honest rounding of any step still reads."""

import numpy as np
import pytest

from hertzforge.io.delimited import _line_within, read_delimited


def csv(tmp_path, times):
    path = tmp_path / 'run.csv'
    path.write_text('Time (s),A (g)\n' + ''.join(f'{t},{i}\n' for i, t in enumerate(times)))
    return path


def written(times, places=3):
    return [f'{t:.{places}f}' for t in times]


@pytest.mark.parametrize('samples', [200, 10000])
def test_a_row_dropped_from_a_whole_step_is_refused(tmp_path, samples):
    times = np.delete(np.arange(samples) / 1000.0, samples // 2)
    with pytest.raises(ValueError, match='rows look to be missing'):
        read_delimited(csv(tmp_path, written(times)))


def test_a_row_dropped_from_a_short_column_is_refused(tmp_path):
    with pytest.raises(ValueError, match='rows look to be missing'):
        read_delimited(csv(tmp_path, ['0.0', '0.1', '0.2', '0.3', '0.4', '0.6']))


def test_a_refused_column_reads_with_the_rate_stated(tmp_path):
    times = np.delete(np.arange(200) / 1000.0, 100)
    history = read_delimited(csv(tmp_path, written(times)), time_column=False,
                             columns=[1], sample_rate=1000.0)
    assert history.sample_rate == 1000.0


def test_steps_the_digits_cannot_explain_are_refused(tmp_path):
    with pytest.raises(ValueError, match='more than the digits'):
        read_delimited(csv(tmp_path, ['0', '1', '2.5', '3', '7', '8']))


def test_one_time_a_unit_late_among_fine_digits_is_refused(tmp_path):
    times = np.round(np.arange(400) / 5120.0, 6)
    times[200] += 1e-6 * np.sign(times[200] - 200 / 5120.0)
    with pytest.raises(ValueError, match='more than the digits'):
        read_delimited(csv(tmp_path, written(times, 6)))


@pytest.mark.parametrize('rate, places, samples', [
    (5120.0, 6, 1000), (512.0, 3, 200), (333.0, 3, 500), (48000.0, 7, 4096),
    (998.0, 3, 5000), (7.0, 1, 64), (2048.0, 4, 32)])
def test_honest_rounding_of_any_step_reads_at_its_rate(tmp_path, rate, places, samples):
    times = np.arange(samples) / rate
    history = read_delimited(csv(tmp_path, written(times, places)))
    assert history.sample_rate == pytest.approx(rate, rel=2e-2 / samples ** 0.5)


def test_a_rate_just_off_a_digit_reads_when_its_long_steps_are_regular(tmp_path):
    times = np.arange(20000) / 999.0
    history = read_delimited(csv(tmp_path, written(times)))
    assert history.sample_rate == pytest.approx(999.0, rel=1e-5)


def test_honest_rounding_passes_for_every_step_near_a_whole_digit():
    rng = np.random.default_rng(0)
    for _ in range(300):
        samples = int(rng.integers(8, 2000))
        step = 1.0 + rng.uniform(0.0005, 0.05)
        values = np.round(rng.uniform(0, 100) + step * np.arange(samples))
        assert _line_within(values, np.full(samples, 0.5))


def test_the_line_ignores_a_row_with_no_digits_read():
    values = np.array([0.0, 1.0, 50.0, 3.0, 4.0])
    assert _line_within(values, np.array([0.5, 0.5, np.inf, 0.5, 0.5]))
    assert not _line_within(values, np.full(5, 0.5))
