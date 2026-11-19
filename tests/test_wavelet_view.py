"""The scalogram as a view: a button, a panel, and a picture drawn.

The math has its own tests (`test_wavelet`). This is the half those
cannot see — that the button reaches it, that the panel says what the
picture is of, and that a reader can get back out again.

That last one is not incidental. Kurtosis used to hide the other
reading buttons while it was up, on the argument that a record showing
bars has no trace to mark; when the readings became a visible group,
that left the group collapsed to whichever button had been pressed and
no way to another except unpicking it first (Brandon, 2026-08-27). The
readings stay offered; the *2-D/3-D toggle* is what goes away, because
neither the bars nor the scalogram has a depth axis to use.
"""

from __future__ import annotations

MODAL = ('plate', 'modal.nc4')


def test_the_transient_report_carries_the_scalogram():
    """Brandon's ask (2026-08-28): the 3-D wavelet plot in the
    transient report. The template names the figure and the prose
    references it, so `test_report`'s reference checks hold them to
    each other like every other figure."""
    from hertzforge.core.report import transient_template

    report = transient_template({})
    blocks = report.blocks
    scalograms = [b for b in blocks if b.get('mode') == 'scalogram']
    assert len(scalograms) == 1
    assert scalograms[0]['select'] == 'dim:acceleration', \
        'the control channels, not the drives'
    prose = ' '.join(b.get('text', '') for b in blocks)
    assert '{{figure:Control scalogram}}' in prose, \
        'the text points at the figure it discusses'


def held_columns(samples):
    """What `scalogram_peaks` holds `samples` to: equal slices of the
    smallest whole-sample width that fits the budget — at most
    `COLUMNS`, and a little under it when the slices do not divide."""
    from hertzforge.core.wavelet import COLUMNS

    step = -(-samples // COLUMNS)
    return -(-samples // step)
