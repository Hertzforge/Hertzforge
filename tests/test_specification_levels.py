"""A specification's own levels: a toggle, bars over the table.

The table of RMS levels used to sit beneath the spectra by default.
Brandon (2026-09-06): take it out of the main plot and make it a
toggle on the bar — on, the upper pane is a bar per channel and the
lower pane the table, the comparison's RMS reading without the ±3 dB
coloring; off, the spectra alone.
"""

from __future__ import annotations


def test_the_level_is_read_in_the_quantity_the_dimension_names():
    """The unit a specification's rms is shown in comes from the
    package's one reading of a dimension (`channel_quantities`), so a
    cross term's or an undeclared dimension cannot be converted as if
    it named a quantity it does not."""
    from hertzforge.core.data import channel_quantities
    from hertzforge.units import DEFAULT_SYSTEM, UNKNOWN

    assert channel_quantities('acceleration**2/frequency') == (
        'acceleration', 'acceleration')
    assert channel_quantities('force*acceleration/frequency')[0] == 'force'
    assert channel_quantities(UNKNOWN) == (UNKNOWN, UNKNOWN)
    assert DEFAULT_SYSTEM.from_si(2.5, channel_quantities(UNKNOWN)[0]) == 2.5
