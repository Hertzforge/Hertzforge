"""Finding the stretch of a record worth averaging.

A run holds the shaker coming up, a reduced-level check, the full level,
and whatever was still recording afterwards. These hold that the right
stretch comes out, that the number of averages comes from the record
rather than from a number chosen in advance, and that the compression to
one level per hop — which is what makes it quick enough for three
hundred channels — does not throw away what the answer depends on.
"""

from __future__ import annotations

import os

import numpy as np
import pytest
from conftest import fixture_path

from hertzforge.core import detect
from hertzforge.core.data import TimeHistory

RATE = 256.0


def test_a_frame_is_about_a_second_and_a_power_of_two():
    """A power of two for the FFT, a second because that is the
    resolution a random vibration test is usually specified at."""
    assert detect.frame_length_for(256) == 256
    assert detect.frame_length_for(4096) == 4096
    assert detect.frame_length_for(1000) == 1024, 'the nearest power of two'
    assert detect.frame_length_for(48000) == 65536


def test_a_record_of_nothing_at_all_is_taken_whole():
    """Every channel flat. There is no level to find, and reporting a
    floor would be inventing one."""
    t = np.arange(2048) / RATE
    data = TimeHistory(abscissa=t, ordinate=np.zeros((4, 2048)),
                       response_dof=['101Z+', '102Z+', '103Z+', '104Z+'],
                       ordinate_dim=['acceleration'] * 4)
    assert detect.analysis_window(data) == (0, 2048)
    assert detect.suggest(data).frames >= 1


def test_the_settled_middle_is_what_is_kept():
    """Closed-loop control does not arrive at a level at once — under
    match-trace it walks in over several frames — so the front of a
    stretch is not yet the level the rest of it is."""
    assert detect.trim(0, 100) == (20, 90)
    assert detect.trim(50, 150) == (70, 140)


def test_more_is_trimmed_from_the_front_than_the_back():
    """The settling is at the start; the tail only risks the level
    coming off early."""
    first, last = detect.trim(0, 100)
    assert first - 0 > 100 - last


def test_a_stretch_too_short_to_trim_is_used_whole():
    """Better the whole of a short stretch than nothing of it."""
    assert detect.trim(10, 11) == (10, 11)
    assert detect.trim(0, 0) == (0, 0)


def test_what_the_trim_refuses():
    for bad in ((0.9, 0.2), (-0.1, 0.5), (0.2, 1.5)):
        with pytest.raises(ValueError, match='fractions'):
            detect.trim(0, 100, bad)


STRESS_RUN = fixture_path('..', 'stressdata', 'plate', 'random.nc4')


@pytest.mark.skipif(not os.path.exists(STRESS_RUN),
                    reason='stressdata is not in the repository')
def test_the_real_run_lands_on_its_full_level_stretch():
    """The run steps -6, -3 and 0 dB and then stops. Full level is
    61-160 s, and what comes back is the settled middle of it.

    The record is 21 MB of streamed time data and lives outside the
    repository, so this is skipped wherever it has not been generated.
    """
    import hertzforge

    data = next(v for v in hertzforge.import_file(STRESS_RUN).values()
                if type(v).__name__ == 'TimeHistory')
    averaging = detect.suggest(data)
    rate = data.sample_rate
    assert averaging.frame_length == 256, 'one second at 256 Hz'
    assert averaging.window == 'hann' and averaging.overlap == 0.5
    # inside the full-level stretch, not on either edge of it. Which 
    # part of it is a judgment the numbers should not be pinned to;
    # that it is all of one level, and enough of it, is the claim.
    assert averaging.start >= 61.0, averaging.start
    assert averaging.stop(rate) <= 160.0, averaging.stop(rate)
    assert averaging.frames >= 30, averaging.frames
    assert averaging.fits(len(data.abscissa), rate)


@pytest.mark.skipif(not os.path.exists(STRESS_RUN),
                    reason='stressdata is not in the repository')
def test_most_of_the_full_level_stretch_is_found():
    """What the smoothing buys, and the only thing that shows it. The
    full-level stretch of a closed-loop run wanders by as much as the
    step to the next level, so read hop by hop it breaks into fragments
    and a fifth of it comes back. Median-filtered over a few seconds it
    comes out nearly whole — which is averages, and averages are the
    point."""
    import hertzforge

    data = next(v for v in hertzforge.import_file(STRESS_RUN).values()
                if type(v).__name__ == 'TimeHistory')
    first, last = detect.analysis_window(data, keep=(0.0, 1.0))
    rate = data.sample_rate
    covered = (min(last / rate, 160.0) - max(first / rate, 61.0)) / 99.0
    assert covered > 0.75, f'only {covered:.0%} of the full-level stretch'


@pytest.mark.skipif(not os.path.exists(STRESS_RUN),
                    reason='stressdata is not in the repository')
def test_the_real_run_is_not_averaged_across_a_level_change():
    """The point of the exercise. Every frame has to be at one level, so
    the window must not straddle 61 s, where the run steps to full."""
    import hertzforge

    data = next(v for v in hertzforge.import_file(STRESS_RUN).values()
                if type(v).__name__ == 'TimeHistory')
    rate = data.sample_rate
    for edge in (31.0, 61.0, 163.0):
        window = detect.suggest(data)
        assert not (window.start < edge < window.stop(rate)), (
            f'the window straddles the step at {edge} s')
