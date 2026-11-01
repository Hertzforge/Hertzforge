"""Two densities on the stage: overlaid per shared channel, or divided.

The 3-D reading of two selected PSDs (Brandon, 2026-08-25): matched
stations recede along the depth axis, the louder wears the level
coloring, the quieter the compared gray — and the Ratio reading
divides through `snr.signal_to_noise`, in decibels, so the stage and the
flat plot cannot disagree."""

from __future__ import annotations

import numpy as np

from hertzforge.core.data import Psd

FREQ = np.linspace(2.0, 1000.0, 400)


def _pair(level=1e-3, floor=1e-6, channels=('101Z+', '104Z+', '110Z+')):
    def density(value):
        return Psd(abscissa=FREQ,
                   ordinate=np.full((len(channels), len(FREQ)), value),
                   response_dof=list(channels),
                   ordinate_dim=['acceleration**2/frequency']
                   * len(channels))
    return density(level), density(floor)


def test_stations_pair_by_shared_label():
    from hertzforge.drawing.viz.paired import paired_arrays

    loud, quiet = _pair()
    arrays = paired_arrays(loud, quiet)
    assert [s['label'] for s in arrays['stations']] == \
        ['101Z+', '104Z+', '110Z+']
    assert arrays['unmatched'] == 0
    lonely, _ = _pair(channels=('101Z+', '999X+'))
    arrays = paired_arrays(lonely, quiet)
    assert [s['label'] for s in arrays['stations']] == ['101Z+']
    assert arrays['unmatched'] == 3, 'each side counted its own strays'


def test_the_ratio_stage_draws_the_picked_channels_as_the_flat_plot_does():
    """Records picked in the louder density's grid: the flat ratio drew
    their channels and the stage divided every shared one (2026-10-14).
    Both keep the picks now, and every channel when none is shared."""
    from hertzforge.drawing.viz.paired import paired_stage_curves

    loud, quiet = _pair()
    ratio = paired_stage_curves(loud, quiet, loud_records=[0, 2], mode='ratio')
    assert [run['label'] for run in ratio['runs']] == ['101Z+', '110Z+']
    assert ratio['drawn'] == 2
    lonely, _ = _pair(channels=('101Z+', '104Z+', '110Z+', '999X+'))
    every = paired_stage_curves(lonely, quiet, loud_records=[3], mode='ratio')
    assert ratio['drawn'] == 2 and every['drawn'] == 3, 'none shared: every channel'


def _mixed_pair():
    """Eight accelerations and four forces on each side — the sysid
    stream's own shape."""
    channels = [f'{n}Z+' for n in (101, 104, 110, 113)] + \
               [f'{n}Z+' for n in (101, 110)]
    dims = ['acceleration**2/frequency'] * 4 + ['force**2/frequency'] * 2

    def density(value):
        return Psd(abscissa=FREQ,
                   ordinate=np.full((6, len(FREQ)), value),
                   response_dof=channels, ordinate_dim=dims)
    return density(1e-3), density(1e-6)


def _levels(pane, name):
    """The levels actually drawn into one of the stage's two meshes."""
    return np.asarray(
        pane.waterfall_plotter.actors[name].mapper.dataset
        .point_data['level'])


def _survey(scale, references=12, responses=50, lines=64):
    """A system ID's FRFs: every response against every reference."""
    from hertzforge.core.data import Frf

    freq = np.linspace(1.0, 500.0, lines)
    rows = references * responses
    ordinate = scale * (1.0 + 1j) * np.ones((rows, lines))
    return Frf(abscissa=freq, ordinate=ordinate,
               response_dof=[f'{100 + r}Z+' for _ref in range(references)
                             for r in range(responses)],
               reference_dof=[f'{900 + k}Z+' for k in range(references)
                              for _r in range(responses)],
               ordinate_dim=['acceleration/force'] * rows)
