"""Marks on a record whose clock does not begin at zero.

An averaging's start and a shock's start count from the record's
beginning — a sample count in disguise — and the plots draw on the
record's own clock, which a truncation keeps and an import window
reads from the run. Brandon (2026-09-18): the last hundred seconds of
a run imported through the window, beginning at 1060 s, and the
averaging reading showed nothing: its frames were drawn at 0 s, off
the plot, while the panel said "start 0 s" against an axis that began
at 1060. Every reading in seconds takes the record's first instant
as its origin, once, in `Averaging.stop`, `frame_bounds` and
`from_span`; the overlays, the stage's geometry, the dragger, the
report and the panels hand it over.
"""

import numpy as np
import pytest

import hertzforge
from hertzforge.core.averaging import Averaging, from_span
from hertzforge.core.shocks import Shock

RATE = 1024.0

ORIGIN = 1060.0

SAMPLES = 8192


def _late_history(origin=ORIGIN, samples=SAMPLES, channels=2):
    t = origin + np.arange(samples) / RATE
    return hertzforge.TimeHistory(
        t, np.random.default_rng(2).standard_normal((channels, samples)),
        response_dof=[f'{101 + k}Z+' for k in range(channels)],
        ordinate_dim='acceleration', ordinate_unit='m/s**2')


def test_the_averaging_reads_its_seconds_on_the_clock():
    averaging = Averaging(frame_length=1024, overlap=0.5, frames=3, start=1.0)
    assert averaging.stop(RATE) == pytest.approx(3.0)
    assert averaging.stop(RATE, ORIGIN) == pytest.approx(ORIGIN + 3.0)
    plain = averaging.frame_bounds(RATE)
    clocked = averaging.frame_bounds(RATE, ORIGIN)
    assert plain[0] == (1.0, 2.0)
    assert clocked == [(a + ORIGIN, b + ORIGIN) for a, b in plain]


def test_a_drag_on_the_clock_means_a_start_from_the_beginning():
    averaging = Averaging(frame_length=1024, overlap=0.0, frames=1)
    dragged = from_span(averaging, ORIGIN + 2.0, ORIGIN + 5.0, RATE, SAMPLES,
                        origin=ORIGIN)
    assert dragged.start == pytest.approx(2.0)
    assert dragged.frames == 3
    assert from_span(averaging, 2.0, 5.0, RATE, SAMPLES) == dragged, \
        'the same drag on a record that begins at zero'


def test_the_stage_geometry_is_on_the_clock():
    from hertzforge.drawing.viz.marks import (
        averaging_stage_geometry,
        shock_stage_geometry,
    )

    extents = (ORIGIN, ORIGIN + (SAMPLES - 1) / RATE, -1.0, 1.0)
    averaging = Averaging(frame_length=1024, overlap=0.0, frames=2, start=1.0)
    geometry = averaging_stage_geometry(averaging, RATE, extents, ORIGIN)
    off = averaging_stage_geometry(averaging, RATE, extents)
    span = geometry['span']
    assert 0.0 <= span[0] < span[1], 'inside the stage'
    assert span != off['span'], 'the origin moved it'
    windows = shock_stage_geometry((Shock(1.0, 0.5),), extents, ORIGIN)['windows']
    assert windows[0]['span'] == pytest.approx(
        averaging_stage_geometry(
            Averaging(frame_length=512, overlap=0.0, frames=1, start=1.0),
            RATE, extents, ORIGIN)['span'])


def test_the_report_frames_are_on_the_clock():
    from hertzforge.report import _averaging_marks

    history = _late_history()
    history.averaging = Averaging(frame_length=1024, overlap=0.0, frames=2,
                                  start=1.0)
    frames = _averaging_marks(history)['frames']
    assert frames[0] == pytest.approx([ORIGIN + 1.0, ORIGIN + 2.0])
