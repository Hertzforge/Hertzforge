"""Drawing decisions from Visual Dynamics' `plot`: what a figure needs
to know, with none of the code that draws it."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, Any

import numpy as np
from numpy.typing import ArrayLike

from ...units import DEFAULT_SYSTEM, UNKNOWN

#: a phase axis: every quarter turn labeled, and room past the half
#: turns for their labels. Pinned at ±185° the ±180° labels sat a few
#: pixels from the edge and pyqtgraph, which draws no label that does
#: not fit, dropped them — the axis read -90, 0 and 90 alone (Brandon,
#: 2026-09-27, through the band-average paper). The report's phase
#: figures take the same two.
PHASE_LIMITS = (-200.0, 200.0)


PHASE_TICKS = (-180, -90, 0, 90, 180)


#: every ordinary curve's line on screen, in the app, a standalone plot
#: and the report page (sent in its payload). One pixel: anything wider
#: leaves Qt's fast path, and two dozen dense FRFs at two repainted a
#: cursor's strip in 41 ms where one takes 2.5, the whole plot in 113
#: where one takes 31 (Retina, 2026-09-27). Brandon kept the speed; the
#: light theme's darker palette and `EXPORT_CURVE_WIDTH` answer "the
#: lines are hard to see" instead.
CURVE_WIDTH = 1


#: a measurement's line when it is the one run read against its
#: specification: wider than the rest, so it stands out from the gray
#: specification behind it in a small printed legend (Brandon,
#: 2026-09-27: "maybe thicker line")
MEASURED_WIDTH = 2.5


#: and translucent, so where it lies exactly on its specification — a
#: band value sitting on the narrowband target it was taken from — the
#: gray still shows through it, not only at its edges. Dashes would
#: have separated them too; Brandon does not want dashes (2026-09-27)
MEASURED_ALPHA = 0.75


#: the specification standing back behind that measurement: wider still,
#: a pixel of gray either side of the line over it — at one pixel under
#: a dense measurement it vanished (Brandon, 2026-09-25), and a
#: half-pixel edge was lost in print where the two coincided
STOOD_BACK_WIDTH = 4.5


#: line styles that tell curves apart without color, as dash lengths in
#: pixels (on, off, ...; empty is solid): solid, dashed, dotted,
#: dash-dot, long dash, dash-dot-dot, short dash, long dash-dot. Eight,
#: one for each curve of a figure that is labeled at its ends. Drawn
#: only where the report feature `plot_conventions` asks for them: the
#: app and the default report tell curves apart by color, and the one
#: measurement read against its specification by width and gray.
LINE_PATTERNS = (
    (), (7, 4), (1.5, 3), (8, 3, 1.5, 3), (14, 5), (8, 3, 1.5, 3, 1.5, 3),
    (4, 3), (14, 4, 2, 4),
)

if TYPE_CHECKING:
    from ...units import UnitSystem


#: the most curves a report figure names at their ends; beyond this the
#: labels crowd each other and the legend says it better
END_LABEL_CURVES = 8


#: categorical curve colors on a dark background
CURVE_COLORS = [
    '#4c92d9', '#ff8c2b', '#3fb950', '#e5534b', '#a371f7',
    '#b07d62', '#e668c3', '#8b949e', '#d2c14e', '#39c5cf',
]


#: the same hues on a light one, darkened to 4.5:1 against white. The
#: dark set stood at 1.8:1 (yellow) to 3.7:1 there, and a one-pixel line
#: that faint was hard to see (Brandon, 2026-09-27). Wider lines would
#: have helped too and cost ten times the repaint; color costs nothing.
CURVE_COLORS_LIGHT = [
    '#2a77c6', '#bc5600', '#2e863a', '#df2d23', '#8c4df5',
    '#9b694e', '#d223a1', '#6b7580', '#827621', '#218086',
]


def axis_label(dimension: str, unit_system: UnitSystem,
                hint: str | None = None) -> str:
    """Axis label for a dimension, or a note when units are undefined.

    The unit is written as HTML, which pyqtgraph renders, so a quotient
    dimension is shown as a stacked fraction and not as '(in/s**2)/lbf'.

    A `hint` is the quantity the source named without giving its units.
    The label includes it, as in 'acceleration [units undefined]',
    because that is more useful than an axis with no name at all.
    """
    if dimension == UNKNOWN:
        return f'{hint} [units undefined]' if hint else 'units undefined'
    return unit_system.label_html(dimension) or '-'


#: points per decade a breakpoint specification is filled in to, so the
#: polyline drawn for it follows the power law its breakpoints mean
POWER_LAW_POINTS = 400


def pair_label(pair: tuple[str, str]) -> str:
    """How a DOF pair is written in a label.

    The DOF alone when it is its own reference, otherwise
    'response/reference'.
    """
    return pair[0] if pair[0] == pair[1] else f'{pair[0]}/{pair[1]}'


def cmif_curves(data: Any, records: Sequence[int] | None = None,
                unit_system: UnitSystem | None = None
                ) -> tuple[np.ndarray, np.ndarray]:
    """The CMIF's singular values (k, frequencies) and display abscissa.

    The records are assembled into the response-by-reference matrix they
    belong to, with a missing pair set to zero (the usual practical
    treatment of an incomplete matrix), and a singular value
    decomposition is taken at every frequency line. The largest singular
    value peaks at every mode. A second singular value peaking at the
    same frequency indicates a repeated root. A line holding an entry
    that is not a finite number has no decomposition, and its singular
    values are NaN, as the conditioning reading leaves such a line
    unjudged.
    """
    from ...core.matrix_checks import frf_matrix

    us = unit_system or DEFAULT_SYSTEM
    wanted = (list(range(data.num_records)) if records is None
              else [int(i) for i in records])
    # assembled where the conditioning reading assembles its matrix; a
    # missing pair stays zero here, where the reading refuses it
    _rows, _columns, matrix, _filled = frf_matrix(
        data, wanted, data.display_ordinate(us, wanted))
    # a record with one sample that is not a number makes every line of
    # its FRF one, and the decomposition of such a line does not converge
    finite = np.isfinite(matrix).all(axis=(-2, -1))
    singular = np.full(matrix.shape[:1] + (min(matrix.shape[1:]),), np.nan)
    if finite.any():
        singular[finite] = np.linalg.svd(matrix[finite], compute_uv=False)
    return singular.T, data.display_abscissa(us)


def display_factor(shown, raw):
    """The factor that converts one curve's SI values to displayed ones.

    A unit conversion is one factor, so it is read from the first point
    where both values are finite and the SI value is positive. Returns 1
    when there is no such point. The plot and the report page both scale
    a compliance verdict's level by it.
    """
    shown = np.asarray(np.real(shown), dtype=float)
    raw = np.asarray(np.real(raw), dtype=float)
    with np.errstate(invalid='ignore'):
        good = np.isfinite(shown) & np.isfinite(raw) & (raw > 0.0)
    if not good.any():
        return 1.0
    k = int(np.flatnonzero(good)[0])
    return float(shown[k] / raw[k])


def drawing_shape(data: Any, tagged: bool = False) -> str:
    """How this object's curve is drawn: 'steps', 'law' or 'line'.

    The rule, for the 2-D plot and the 3-D waterfall alike, is that a
    density is drawn so that its RMS is the area under the curve on
    screen. A PSD, a CPSD's terms and an octave-band PSD (`interpolation
    == 'bin'`) are drawn flat across each bin ('steps'), which is what
    is summed when the object is integrated, and a specification
    (`interpolation == 'log_log'`) follows the power law between its
    breakpoints ('law'). Anything that is a value at a frequency and not
    an area, such as an FRF, a coherence, a linear spectrum or any
    signed component, is drawn as a line ('line'). The exception is a
    value that belongs to a whole band, which is drawn flat across the
    band whatever component is read: a banded coherence, or the phase of
    a banded cross term, is one number for its band and not a sample at
    its center. Data that is not on a frequency axis is always a line.

    There is one implementation, used by `build_plots` and by
    `viz.waterfall`, because a curve that disagrees with `Psd.area()`
    shows a number nobody computed. `tagged` says a signed component
    (real, imag or phase) is being drawn, which is never a density,
    though for a banded array it is still a band's value.
    """
    # Brandon, 2026-09-27: a banded value is one number for its band and
    # is drawn flat across it, whatever component is read.
    if data.abscissa_dim != 'frequency':
        return 'line'
    banded = getattr(data, 'bandwidth', None) is not None
    if tagged:
        return 'steps' if banded else 'line'
    shape = {'bin': 'steps', 'log_log': 'law'}.get(
        getattr(data, 'interpolation', None))
    if shape is None and banded:
        return 'steps'
    return shape or 'line'


def step_outline(centers: ArrayLike, values: ArrayLike,
                 widths: ArrayLike | None = None
                 ) -> tuple[np.ndarray, np.ndarray]:
    """The flat-across-each-bin trace as explicit points.

    This is what `stepMode='center'` has pyqtgraph draw in the 2-D plot,
    written out for a renderer that only takes polylines, the 3-D
    waterfall. There are two points per bin, on the bin's edges (from
    `bin_edges`), with the value repeated, so the area under the trace
    is exactly `sum(value * width)` and the RMS is the area drawn.
    `values` may be (records, lines), in which case each row is
    outlined. Returns (x, values).
    """
    centers = np.asarray(centers, dtype=float)
    values = np.atleast_2d(np.asarray(values))
    if centers.size < 2:
        return centers, values[0] if values.shape[0] == 1 else values
    edges = bin_edges(centers, widths)
    x = np.repeat(edges, 2)[1:-1]
    expanded = np.repeat(values, 2, axis=1)
    return x, expanded[0] if expanded.shape[0] == 1 else expanded


def bin_edges(centers: ArrayLike,
              widths: ArrayLike | None = None) -> np.ndarray:
    """The edges of the bins that a set of line centers represents.

    A line of a discrete spectrum is a density over its own bin, so with
    no other information each bin runs to the midpoint of the gap on
    either side, and the outer edges are as far beyond the end lines as
    the nearest midpoints are inside them. Arithmetic midpoints are used
    because FFT lines are evenly spaced in frequency.

    `widths` is for a spectrum that knows its bins. One more value than
    there are centers is taken as the edges themselves
    (`Psd.bin_edges`). Otherwise `widths` is each band's width. An
    octave band's center is the geometric mean of its edges, so neither
    the midpoints between neighbors nor the center plus and minus half a
    width gives them. From the center and the width the edges follow
    exactly: with `c = sqrt(l u)` and `w = u - l`,

        u = (w + sqrt(w^2 + 4 c^2)) / 2,   l = u - w

    which is the positive root and does not assume the bands tile. These
    bands do tile, so one array of edges serves them all.
    """
    centers = np.asarray(centers, dtype=float)
    if centers.size < 2:
        return centers
    if widths is not None:
        widths = np.asarray(widths, dtype=float)
        if widths.size == centers.size + 1:
            # the object's own drawn edges (`Psd.bin_edges`), handed
            # over whole: a banded spectrum's end band is drawn only
            # where its source covered it (2026-09-19), which no
            # center-and-width can say
            return widths
        from ...core.octave import bin_bounds

        lower, upper = bin_bounds(centers, widths)
        return np.concatenate([lower, [upper[-1]]])
    mids = 0.5 * (centers[:-1] + centers[1:])
    return np.concatenate([[2 * centers[0] - mids[0]], mids,
                           [2 * centers[-1] - mids[-1]]])


def as_power_law(frequencies: ArrayLike, values: ArrayLike,
                 per_decade: int = POWER_LAW_POINTS
                 ) -> tuple[np.ndarray, np.ndarray]:
    """A breakpoint curve resampled densely enough to draw as a curve.

    A specification's segments are power laws, and the plot's frequency
    axis is linear, so a straight line drawn between two breakpoints is
    not the curve the breakpoints define. For a decade-wide segment it
    runs well above that curve through the middle. Resampled at
    `per_decade` points per decade (`POWER_LAW_POINTS` by default), the
    polyline is within a pixel of the power law.

    Points already that dense are not resampled: a controller writes its
    specification on the same line grid as everything else, and there is
    nothing between two adjacent lines to fill in.

    Only the band the specification is written for is returned. A
    controller writes zero outside it, and zero cannot be drawn on a log
    axis (it maps to -inf), while interpolating across it gives NaN.
    Either way the curve would have gaps, and pyqtgraph cannot safely
    build a path from an array with gaps (see `gapless`). With fewer
    than two points that can be drawn, the input is returned unchanged.
    """
    from ...core.compliance import log_interpolate

    frequencies = np.asarray(frequencies, dtype=float)
    values = np.asarray(values, dtype=float)
    good = (np.isfinite(frequencies) & (frequencies > 0.0)
            & np.isfinite(values) & (values > 0.0))
    if good.sum() < 2:
        return frequencies, values
    said = np.flatnonzero(good)
    low, high = frequencies[said[0]], frequencies[said[-1]]
    decades = np.log10(high / low)
    wanted = int(decades * per_decade)
    if wanted <= good.sum():
        span = slice(said[0], said[-1] + 1)
        return frequencies[span], values[span]
    filled = np.geomspace(low, high, wanted)
    return filled, log_interpolate(filled, frequencies, values)


def _zone_decibels(found: dict, bands: Mapping[str, Any], target: Any,
                   ordinate_dim: str) -> None:
    """Each limit's distance from the target it bounds, in decibels,
    gathered into `found` per limit — for the legend to state when it
    is one number everywhere. Ten log ten of a power quantity (a PSD's
    dimension is a square), twenty of an amplitude, as `compliance`
    reads them."""
    per = 10.0 if '**2' in str(ordinate_dim) else 20.0
    target = np.asarray(np.real(target), dtype=float)
    for bound, values in bands.items():
        values = np.asarray(np.real(values), dtype=float)
        if values.shape != target.shape:
            continue
        with np.errstate(divide='ignore', invalid='ignore'):
            db = per * np.log10(values / target)
        db = db[np.isfinite(db)]
        if db.size:
            found.setdefault(bound, []).append(db)


def _uniform_db(found: dict, bound: str) -> float | None:
    """The one value a limit sits at in decibels, or None when it moves."""
    values = found.get(bound)
    if not values:
        return None
    every = np.concatenate(values)
    if np.ptp(every) > 0.05:
        return None
    return float(np.round(np.mean(every), 1))


def _signed_db(value: float) -> str:
    return f'{value:+g} dB'.replace('-', '\u2212')


def zone_key(zones: set, exceeded: set, found: Mapping[str, Any]
             ) -> list[tuple[str, str]]:
    """The legend entries for a specification's shading.

    The entries are the warning band, the zones above and below abort,
    and the exceedance marks, each only when it was drawn: `zones` holds
    any of 'warning', 'above' and 'below', and `exceeded` any of
    'exceed_over' and 'exceed_under'. When the specification holds a
    band at one value, its level in decibels is added to the name, as in
    'warning band (±3 dB)'. `found` is the decibel values gathered by
    `_zone_decibels`.

    The app's plots (`build_plots`) and the report's both use this, so
    their legends match.

    Returns
    -------
    list of (str, str)
        (zone, name), in the order a legend lists them.
    """
    # Brandon, 2026-09-27: the app's legend and the report's must always
    # match.
    def db_note(*bounds):
        values = [_uniform_db(found, bound) for bound in bounds]
        if any(v is None for v in values):
            return ''
        if len(values) == 2 and np.isclose(values[0], -values[1]):
            return f' (\u00b1{abs(values[0]):g} dB)'
        return ' (' + '/'.join(_signed_db(v) for v in values) + ')'

    entries = []
    if 'warning' in zones:
        entries.append(('warning', 'warning band'
                        + db_note('warning_upper', 'warning_lower')))
    if 'above' in zones:
        entries.append(('above', 'above abort' + db_note('abort_upper')))
    if 'below' in zones:
        entries.append(('below', 'below abort' + db_note('abort_lower')))
    if 'exceed_over' in exceeded:
        entries.append(('exceed_over', 'line over abort'))
    if 'exceed_under' in exceeded:
        entries.append(('exceed_under', 'line under abort'))
    return entries
