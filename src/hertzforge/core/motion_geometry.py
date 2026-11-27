"""Where the sensors are and how they are turned, from the motion they
measured and nothing else.

Below its first elastic mode a structure moves as a rigid body, and a
channel at position r measuring along the unit vector d reads

    a(t) = d . a0(t) + (r x d) . alpha(t)

where a0 is the body's translation and alpha its rotation. So the
low-frequency part of n channels is L q(t): q the six rigid motions and
L (n, 6) one row per channel, [d, r x d], the line the channel measures
along. The data gives L's column space, as the six dominant directions
of the channels' co-spectrum in a low band (`rigid_motion`). It does not
give L, since L M spans the same space for any 6 x 6 M. What picks M is
what every row of a true L satisfies: d is a unit vector, d and r x d are
at right angles, the axes of one sensor are at right angles to each
other, and the lines of one sensor meet at a point (`fit_poses`).

Three things the motion cannot say, whatever the data. The size: every
position scaled, with the rotations scaled the other way, reads the
same. The mirror image: every position reflected through a point, with
the sensors' axes kept and the rotations reversed, reads the same. And
where a sensor with one axis sits along that axis, since sliding it
along its own line changes nothing it reads. `place` takes the size and
the image from a geometry that already holds some of the nodes, or from
a length, and says what was left open.

All six rigid motions must be in the data. One test that shakes along
one axis rarely has them; several tests together can, and are pooled:
their co-spectra are summed, so each adds the motions it excited.

Tests on a table that only translates hold three motions, not six. Then
every channel reads d . a0(t) and nothing of r: the data says how each
sensor is turned and not where it is (`fit_directions`). The directions
come out in a frame of their own, and `turn` sets them in the frame of a
geometry that holds the nodes, against the coordinate systems it gives
them, and says which channels do not read the way it has them.
"""

from __future__ import annotations

import itertools
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field, replace
from typing import Any

import numpy as np
from scipy import fft as scipy_fft
from scipy.optimize import least_squares, minimize_scalar
from scipy.spatial.transform import Rotation

from .data import parse_dof

#: the rigid motions of a body
MOTIONS = 6
#: its translations, which alone turn the sensors without placing them
TRANSLATIONS = 3
#: the quantities a rigid motion is read in; one of them at a time, since a
#: velocity and an acceleration of one motion differ by a factor that changes
#: across the band
QUANTITIES = ('acceleration', 'velocity', 'displacement')
#: how far a motion must stand above the next, in amplitude, to be counted
CLEAR = 5.0
#: the candidate bands the default band is chosen among: third octaves
BAND_STEP = 2.0 ** (1.0 / 3.0)
#: how many bands above the lowest that moves the rigid band may start
REACH = 3
#: the parts a band is cut into to take its subspace to zero frequency, and
#: the lines each needs
SUB_BANDS = 6
SUB_BAND_LINES = 4
#: a channel with this little of the strongest channel's mean square does not move
DEAD = 1e-12
#: a random start that ends this close to the measured subspace ends the search
GOOD_MISFIT = 0.01
#: a fit further than this from the measured subspace is warned of
POOR_MISFIT = 0.05
#: how many sensors a poor fit is looked into by releasing, at most
MOST_RELEASED = 3
#: two channels are axes of one sensor when their directions are within this of a right
#: angle, degrees, and their lines pass this close, as a part of the layout's size
SQUARE = 5.0
MEETING = 0.03
#: a band holds the motions a run of bands started with while the largest sine between
#: the two subspaces is under this
SAME = 0.1
#: three motions that sensors which only translate leave this much of unexplained are
#: not the three translations
NOT_TRANSLATIONS = 0.15
#: a channel within this of its direction in a known geometry agrees with it, degrees
AGREES = 15.0
#: the pairs of one-axis sensors tried for the turn onto a known geometry, at most
PAIRS = 60
#: random starts tried when there is no linear one, and after a poor linear one, which
#: random starts seldom better
STARTS = 24
AFTER_LINEAR = 6

_UPPER = np.triu_indices(MOTIONS)


@dataclass(frozen=True)
class Channel:
    """One channel that reads a translation: its record's identity
    (`TimeHistory.channel_key`), its node, which of the node's axes it
    is along (0 X, 1 Y, 2 Z) and its sign."""

    key: tuple[str, str, str | None]
    node: int
    axis: int
    sign: float

    @property
    def dof(self) -> str:
        """Its DOF as its record names it, '12Y-'."""
        return self.key[0]


@dataclass(frozen=True)
class Motion:
    """The rigid motion found in the data: `basis` (channels, 6), the
    orthonormal directions the channels move in together in `band`, Hz,
    or (channels, 3) when only three motions stand clear, which are
    taken as the three translations. `bands` are the candidate bands'
    edges, Hz, and `values` (bands, k) the amplitudes of the strongest
    motions in each, largest first, the evidence the band was chosen on.
    `notes` say what was left out or could not be done, and `without_six`
    why six motions were not read when three were."""

    channels: tuple[Channel, ...]
    basis: np.ndarray
    band: tuple[float, float]
    bands: np.ndarray
    values: np.ndarray
    lines: int
    notes: tuple[str, ...] = ()
    without_six: str = ''

    @property
    def count(self) -> int:
        """How many motions were read: six, or three."""
        return int(self.basis.shape[1])


@dataclass(frozen=True)
class Sensor:
    """One sensor recovered: its node, the axes it measures (0 X, 1 Y,
    2 Z), `frame` (3, 3) whose rows are its X, Y and Z in the global
    system, and its position. A sensor with one axis is somewhere on the
    line through `position` along that axis, and the two axes it does
    not measure are any two that complete the frame. A sensor that was
    turned and not placed has no position: it is not a number."""

    node: int
    axes: tuple[int, ...]
    frame: np.ndarray
    position: np.ndarray
    #: the part of its channels' rigid motion the fit leaves unexplained
    misfit: float = 0.0
    #: the axes whose channel reads along minus the frame's axis: a sensor whose X, Y and Z
    #: are a left-handed set as labeled is given a right-handed frame with its Z reversed
    reversed: tuple[int, ...] = ()
    #: what is wrong with its DOFs as labeled, in a few words; '' when nothing was found
    note: str = ''

    @property
    def slides(self) -> bool:
        """Whether its place along its own axis is open."""
        return len(self.axes) == 1

    @property
    def placed(self) -> bool:
        """Whether it has a position."""
        return bool(np.isfinite(self.position).all())

    def reads_along(self, axis: int, sign: float = 1.0) -> np.ndarray:
        """The direction a channel on `axis` with `sign` reads along, as
        the data has it, which for a reversed axis is against the frame."""
        return (-sign if axis in self.reversed else sign) * self.frame[axis]


@dataclass(frozen=True)
class Recovered:
    """The sensors recovered from a rigid motion. Positions are in
    `length_unit`, or in no unit when it is None, the two sensors of two
    axes or more that are farthest apart (`span_nodes`) then one apart.
    `misfit` is the part of the measured motion the sensors as placed do
    not explain, 0 to 1. `image_open` says the mirror image fits the
    data as well and nothing chose between them. `left_out` are the DOFs
    of channels that do not measure where their node's other channels do,
    which are not part of any sensor here.

    `turned_only` says the data held the three translations and no
    rotation, so the sensors are turned and not placed: a sensor has a
    position only where `turn` took it from a known geometry. `off` is
    then how far each channel reads from its direction in that geometry,
    degrees by DOF, and `agree` how many of the channels compared read
    within `AGREES` of it, and how many were compared.

    `framed` says the sensors are in the frame of a known geometry, at
    its size: `place` moved them onto three of its nodes that are not in
    a line, or `turn` put them at its nodes. Until then they are in a
    frame of their own, and nothing else is in it with them."""

    sensors: tuple[Sensor, ...]
    motion: Motion
    misfit: float
    starts: int
    length_unit: str | None = None
    image_open: bool = True
    span_nodes: tuple[int, int] | None = None
    notes: tuple[str, ...] = field(default=())
    left_out: tuple[str, ...] = ()
    turned_only: bool = False
    off: Mapping[str, float] = field(default_factory=dict)
    agree: tuple[int, int] | None = None
    framed: bool = False

    def sensor(self, node: int) -> Sensor:
        """The sensor at `node`."""
        return next(one for one in self.sensors if one.node == node)

    def reads_along(self, channel: Channel) -> np.ndarray | None:
        """The direction `channel` reads along, or None for a channel
        that was left out of its node's sensor."""
        if channel.dof in self.left_out:
            return None
        one = next((one for one in self.sensors if one.node == channel.node), None)
        return None if one is None else one.reads_along(channel.axis, channel.sign)


# ---- the channels ---------------------------------------------------------


def motion_channels(history: Any, records: Sequence[int] | None = None
                    ) -> tuple[list[Channel], list[str]]:
    """The channels of `history` that read a translation, in record order,
    and what was left out, in words.

    A record is a channel here when its DOF names a node and one of X, Y
    and Z, and its quantity is an acceleration, a velocity or a
    displacement with its unit declared. Records of other quantities
    (forces, strains) are left out with a count; a rotation, a record
    with no node and a record with no unit are left out by name.

    Raises
    ------
    ValueError
        If the channels are of more than one quantity, or two channels
        claim one axis of one node.
    """
    rows = range(len(history.response_dof)) if records is None else [int(r) for r in records]
    found: dict[tuple, Channel] = {}
    notes: list[str] = []
    other = 0
    for row in rows:
        key = history.channel_key(row)
        if key in found:
            continue
        dof, dimension, hint = key
        node, direction = parse_dof(dof)
        if dimension not in QUANTITIES:
            if dimension == 'unknown' and hint in QUANTITIES:
                notes.append(f'{dof} is left out: its unit is not declared')
            else:
                other += 1
            continue
        letters = direction.rstrip('+-')
        if node is None or letters not in ('X', 'Y', 'Z'):
            notes.append(f'{dof} is left out: '
                         + ('it names no node' if node is None else 'it is not a translation'))
            continue
        found[key] = Channel(key, node, 'XYZ'.index(letters), -1.0 if direction.endswith('-') else 1.0)
    if other:
        notes.append(f'{other} record{"s" * (other != 1)} of other quantities left out')
    channels = list(found.values())
    quantities = sorted({channel.key[1] for channel in channels})
    if len(quantities) > 1:
        raise ValueError(f'the channels are {" and ".join(quantities)}; a rigid motion is read '
                         'in one quantity: pick the records of one')
    taken: dict[tuple[int, int], Channel] = {}
    for channel in channels:
        twin = taken.setdefault((channel.node, channel.axis), channel)
        if twin is not channel:
            raise ValueError(f'{twin.dof} and {channel.dof} are both the {"XYZ"[channel.axis]} '
                             f'axis of node {channel.node}; pick one of them')
    return channels, notes


def _sensors_of(channels: Sequence[Channel]) -> dict[int, list[tuple[int, int, float]]]:
    """{node: [(channel row, axis, sign), ...]}, nodes in first-seen order."""
    out: dict[int, list[tuple[int, int, float]]] = {}
    for row, channel in enumerate(channels):
        out.setdefault(channel.node, []).append((row, channel.axis, channel.sign))
    return out


# ---- the motion -----------------------------------------------------------


def _band_lines(history: Any, channels: Sequence[Channel], averaging: Any
                ) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """(frequencies, one-sided scale, lines (channels, frames, lines)) of
    the channels' frames, cut and windowed as a PSD's are."""
    keys = [channel.key for channel in channels]
    frequencies, scale, groups = history._spectral_frame(averaging, channels=set(keys))
    counts = {len(groups[key]) for key in keys}
    if len(counts) > 1:
        raise ValueError('the channels hold different numbers of captures, so they cannot be '
                         'read against each other')
    stacked = np.array([groups[key] for key in keys])
    if not np.isfinite(stacked).all():
        bad = next(key[0] for key in keys if not np.isfinite(groups[key]).all())
        raise ValueError(f'{bad} holds a sample that is not a number')
    return frequencies, scale, scipy_fft.rfft(stacked, axis=2, workers=-1)


class _Pool:
    """The co-spectra of several histories, summed band by band."""

    def __init__(self, histories: Sequence[Any], channels: Sequence[Channel],
                 averagings: Sequence[Any]) -> None:
        self.parts: list[tuple[np.ndarray, np.ndarray, np.ndarray]] = [
            _band_lines(history, channels, averaging)
            for history, averaging in zip(histories, averagings, strict=True)]
        #: the coarsest line spacing and the lowest top line among the histories
        self.spacing: float = max(float(freq[1] - freq[0]) for freq, _s, _x in self.parts)
        self.top: float = min(float(freq[-1]) for freq, _s, _x in self.parts)
        self.frames: int = sum(lines.shape[1] for _f, _s, lines in self.parts)

    def lines_in(self, low: float, high: float) -> int:
        """The lines of the history with the fewest in [low, high)."""
        return min(int(np.count_nonzero((freq >= low) & (freq < high)))
                   for freq, _s, _x in self.parts)

    def cospectrum(self, low: float, high: float) -> np.ndarray:
        """The channels' co-spectrum summed over the lines of [low, high)
        and averaged over every frame of every history, so a longer test
        counts for more: the real part of the cross-spectral matrix,
        which a rigid motion leaves of rank six however its six parts are
        phased. Its diagonal is each channel's mean square in the band."""
        count = self.parts[0][2].shape[0]
        out = np.zeros((count, count))
        for freq, scale, lines in self.parts:
            keep = (freq >= low) & (freq < high) & (freq > 0)
            if not keep.any():
                continue
            cut = lines[:, :, keep] * np.sqrt(scale[keep] * (freq[1] - freq[0]))
            flat = cut.reshape(count, -1)
            out += np.real(flat.conj() @ flat.T)
        return out / self.frames


def _amplitudes(cospectrum: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    """The motions of a co-spectrum: amplitudes, largest first, and the
    directions as columns."""
    value, vector = np.linalg.eigh(cospectrum)
    order = np.argsort(value)[::-1]
    return np.sqrt(np.maximum(value[order], 0.0)), vector[:, order]


def clear_motions(values: np.ndarray) -> int:
    """How many motions stand clear in one band's amplitudes: the most,
    up to six, that are each `CLEAR` times the seventh strongest, or,
    with fewer than six, the count above the widest gap."""
    values = np.asarray(values, dtype=float)
    if len(values) <= MOTIONS or values[0] <= 0:
        return 0
    floor = values[MOTIONS]
    if values[MOTIONS - 1] >= CLEAR * floor and floor >= 0:
        return MOTIONS
    for count in range(MOTIONS - 1, 0, -1):
        if values[count - 1] >= CLEAR * values[count]:
            return count
    return 0


def _candidate_bands(pool: _Pool) -> np.ndarray:
    """Third-octave edges from the lowest band that holds two lines of
    every history to half the lowest top line."""
    edges = [2.0 * pool.spacing / (BAND_STEP - 1.0)]
    while edges[-1] * BAND_STEP <= pool.top / 2:
        edges.append(edges[-1] * BAND_STEP)
    return np.array(edges)


def rigid_motion(histories: Sequence[Any], band: tuple[float, float] | None = None, *,
                 records: Sequence[Sequence[int] | None] | None = None,
                 averagings: Sequence[Any] | None = None) -> Motion:
    """The rigid motion in the low band of one or several time histories.

    Every history must hold the same channels (`motion_channels`); their
    co-spectra are summed, so tests that each shake the structure another
    way together give the motions none has alone. The frames are each
    history's own averaging, or `averagings` where given.

    With no `band`, it is the lowest run of third-octave bands in which
    six motions stand `CLEAR` times above the seventh: below it the
    excitation has not started, and above it the structure bends. The
    subspace is then taken to zero frequency: a bending structure adds a
    part that grows as the frequency squared, so the subspace of each of
    `SUB_BANDS` parts of the band is fitted against the square of its
    frequency and read at zero.

    Raises
    ------
    ValueError
        If there is no history, the histories hold different channels,
        there are fewer than seven channels, the band holds no lines, a
        channel does not move in it, or six motions do not stand clear,
        in which case the message says by how much.
    """
    histories = list(histories)
    if not histories:
        raise ValueError('no time history to read')
    picks = list(records) if records is not None else [None] * len(histories)
    listed = [motion_channels(history, pick) for history, pick in zip(histories, picks, strict=True)]
    channels, notes = listed[0]
    for position, (theirs, _said) in enumerate(listed[1:], start=2):
        ours, other = {c.key for c in channels}, {c.key for c in theirs}
        if ours != other:
            odd = min(ours ^ other)[0]
            raise ValueError(f'the histories hold different channels: {odd} is in one and not in '
                             f'history {position}; pick histories of one set of sensors')
    if len(channels) <= MOTIONS:
        raise ValueError(f'{len(channels)} channel{"s" * (len(channels) != 1)} of translation; '
                         'at least seven are needed to tell six rigid motions from the rest')
    pool = _Pool(histories, channels, list(averagings) if averagings is not None
                 else [None] * len(histories))
    edges = _candidate_bands(pool)
    shown = min(len(channels), MOTIONS + 2)
    found = [_amplitudes(pool.cospectrum(low, high)) for low, high in itertools.pairwise(edges)]
    values = np.array([amplitudes[:shown] for amplitudes, _v in found]).reshape(-1, shown)
    given = band is not None
    count, without_six = MOTIONS, ''
    if band is None:
        try:
            band = _clear_band(edges, values, len(histories))
        except ValueError as six:
            translating = _translating_band(
                edges, values, [vectors[:, :TRANSLATIONS] for _a, vectors in found])
            if translating is None:
                raise
            band, count, without_six = translating, TRANSLATIONS, str(six)
    low, high = float(band[0]), float(band[1])
    if not 0 <= low < high:
        raise ValueError(f'a band runs from a lower frequency to a higher one, not {low:g} to '
                         f'{high:g} Hz')
    lines = pool.lines_in(low, high)
    if lines < 1:
        raise ValueError(f'no line of the spectra lies in {low:g} to {high:g} Hz; the lines are '
                         f'{pool.spacing:g} Hz apart and reach {pool.top:g} Hz')
    together = pool.cospectrum(low, high)
    power = np.diag(together)
    if not power.max() > 0:
        raise ValueError(f'the channels do not move in {low:g} to {high:g} Hz')
    still = [channel.dof for channel, level in zip(channels, power, strict=True)
             if level <= DEAD * power.max()]
    if still:
        raise ValueError(f'{", ".join(still[:4])}{" and others" * (len(still) > 4)} '
                         f'do{"es" * (len(still) == 1)} not move in {low:g} to {high:g} Hz: a '
                         'dead channel cannot be placed; pick the records without it')
    # a band the rule chose was judged third octave by third octave already. One that was
    # given is judged at its low end, where the bending the rest of it holds is least and
    # is taken out afterwards (`_at_zero`)
    parts = min(SUB_BANDS, lines // SUB_BAND_LINES)
    end = low + (high - low) / parts if parts >= 3 else high
    judged = _amplitudes(pool.cospectrum(low, end))[0]
    if given and clear_motions(judged) < MOTIONS:
        without_six = _too_few(judged, low, high, len(histories))
        if not _three_stand(judged):
            raise ValueError(without_six)
        count = TRANSLATIONS
    notes = list(notes)
    basis = _at_zero(pool, low, high, lines, notes, count)
    return Motion(tuple(channels), basis, (low, high), edges, values, lines, tuple(notes),
                  without_six)


def _too_few(values: np.ndarray, low: float, high: float, histories: int) -> str:
    """Why a band does not hold the rigid motion, in the numbers it was
    judged by: how far the sixth strongest motion stands above the
    seventh. It does not count the rigid motions, since a bending that
    stands clear of the noise would be counted with them."""
    ratio = values[MOTIONS - 1] / values[MOTIONS] if values[MOTIONS] > 0 else np.inf
    more = ('' if histories > 1 else
            ' Tests that shake it along other axes can be picked with this one.')
    return (f'six rigid motions do not stand clear in {low:.3g} to {high:.3g} Hz: the sixth '
            f'strongest motion is {ratio:.1f} times the seventh, and {CLEAR:g} times is needed. '
            'The three translations and the three rotations must all be in the data to place '
            f'the sensors.{more}')


def _clear_band(edges: np.ndarray, values: np.ndarray, histories: int) -> tuple[float, float]:
    """The run of candidate bands with six clear motions that starts where
    the data starts to move; refused, counting the motions there, when
    there is none.

    A rigid motion is what a structure does at its lowest frequencies, and
    its six parts answer the same forces, so they come out of the noise
    together. A band higher up in which six motions happen to stand clear
    among the elastic modes is not it: the run must start within `REACH`
    bands of the lowest one in which anything stands clear."""
    counts = [clear_motions(row) for row in values]
    moving = next((k for k, count in enumerate(counts) if count), None)
    if moving is None:
        raise ValueError('no motion stands clear of the noise in any band of the data'
                         if counts else 'the record is too short to hold a band of lines; '
                         'lengthen it or its frames')
    start = counts[moving:moving + REACH + 1]
    if MOTIONS not in start:
        ratios = [row[MOTIONS - 1] / row[MOTIONS] if row[MOTIONS] > 0 else np.inf
                  for row in values[moving:moving + REACH + 1]]
        best = moving + int(np.argmax(ratios))
        raise ValueError(_too_few(values[best], edges[best], edges[best + 1], histories))
    first = moving + start.index(MOTIONS)
    last = first
    while last + 1 < len(counts) and counts[last + 1] == MOTIONS:
        last += 1
    return float(edges[first]), float(edges[last + 1])


def _three_stand(values: np.ndarray) -> bool:
    """Whether the three strongest motions of a band stand `CLEAR` times
    above the fourth."""
    return bool(len(values) > TRANSLATIONS and values[TRANSLATIONS - 1] > 0
                and values[TRANSLATIONS - 1] >= CLEAR * values[TRANSLATIONS])


def _translating_band(edges: np.ndarray, values: np.ndarray, vectors: Sequence[np.ndarray]
                      ) -> tuple[float, float] | None:
    """The run of candidate bands in which three motions stand clear of
    the fourth, starting where the data starts to move as `_clear_band`
    has it, or None when there is none.

    Tests of one drive each hold as many motions as there are tests at
    every frequency, bending or not, so a count of motions does not end
    the run where the structure starts to bend. What does is that the
    motions stop being the same ones: the run ends at the first band
    whose three are further than `SAME` from those of the band it
    started in."""
    stands = [_three_stand(row) for row in values]
    moving = next((k for k, row in enumerate(values) if clear_motions(row)), None)
    if moving is None or True not in stands[moving:moving + REACH + 1]:
        return None
    first = moving + stands[moving:moving + REACH + 1].index(True)
    start = vectors[first]
    last = first
    while last + 1 < len(stands) and stands[last + 1]:
        other = vectors[last + 1]
        if np.linalg.norm(other - start @ (start.T @ other), 2) > SAME:
            break
        last += 1
    return float(edges[first]), float(edges[last + 1])


def _at_zero(pool: _Pool, low: float, high: float, lines: int, notes: list[str],
             count: int = MOTIONS) -> np.ndarray:
    """The subspace of the band's `count` strongest motions, taken to
    zero frequency where the band has the lines for it."""
    parts = min(SUB_BANDS, lines // SUB_BAND_LINES)
    if parts < 3:
        notes.append(f'the band holds {lines} line{"s" * (lines != 1)}, too few to take the '
                     'bending out of it: lengthen the frames or widen the band')
        return _amplitudes(pool.cospectrum(low, high))[1][:, :count]
    edges = np.linspace(low, high, parts + 1)
    projectors, squares = [], []
    for a, b in itertools.pairwise(edges):
        vectors = _amplitudes(pool.cospectrum(a, b))[1][:, :count]
        projectors.append(vectors @ vectors.T)
        squares.append((a * a + a * b + b * b) / 3.0)
    design = np.column_stack([np.ones(parts), np.array(squares) / squares[-1]])
    fitted = np.linalg.lstsq(design, np.array(projectors).reshape(parts, -1), rcond=None)[0][0]
    side = projectors[0].shape[0]
    zero = fitted.reshape(side, side)
    return _amplitudes((zero + zero.T) / 2.0)[1][:, :count]


# ---- the sensors ----------------------------------------------------------


def _skew(v: np.ndarray) -> np.ndarray:
    return np.array([[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]])


def _row_of(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    """The coefficients of a symmetric G's upper triangle in a G b."""
    m = np.outer(a, b)
    m = m + m.T
    m[np.diag_indices(MOTIONS)] /= 2.0
    return m[_UPPER]


def _symmetric(upper: np.ndarray) -> np.ndarray:
    out = np.zeros((MOTIONS, MOTIONS))
    out[_UPPER] = upper
    return out + np.triu(out, 1).T


def _linear_start(basis: np.ndarray, sensors: Mapping[int, list]) -> np.ndarray | None:
    """The lines [d, r x d] of every channel from the linear conditions,
    or None when the sensors give too few to settle them.

    With lines = basis M, the conditions on a pair of rows of one sensor
    are linear in two symmetric matrices: G1 = M J1 M' (d_i . d_j, one for
    a row with itself and zero for two axes) and G2 = M J2 M'
    (d_i . m_j + d_j . m_i, always zero). G2 is the null vector of the
    conditions. G1 is a solution of them plus whatever multiple of G2
    leaves it of rank three, since d has three components. M follows from
    the two up to a rigid move, a scale and the mirror image.
    """
    rows, ones = [], []
    for members in sensors.values():
        for i, (ri, _ai, _si) in enumerate(members):
            for rj, _aj, _sj in members[i:]:
                rows.append(_row_of(basis[ri], basis[rj]))
                ones.append(1.0 if ri == rj else 0.0)
    if len(rows) < len(_UPPER[0]):
        return None
    conditions, target = np.array(rows), np.array(ones)
    _u, singular, vt = np.linalg.svd(conditions)
    if singular[-2] < 1e-9 * singular[0]:
        return None
    g2 = _symmetric(vt[-1])
    g1 = _symmetric(np.linalg.lstsq(conditions, target, rcond=None)[0])

    def defect(c: float) -> float:
        value = np.linalg.eigvalsh(g1 + c * g2)
        return float(np.sum(np.abs(value[:3])) + np.sum(np.abs(np.minimum(value[3:], 0.0))))

    scale = np.linalg.norm(g1) / np.linalg.norm(g2)
    grid = np.linspace(-20.0, 20.0, 2001) * scale
    best = grid[int(np.argmin([defect(c) for c in grid]))]
    step = grid[1] - grid[0]
    c = minimize_scalar(defect, bounds=(best - step, best + step), method='bounded').x
    value, vector = np.linalg.eigh(g1 + c * g2)
    if value[3] <= 0:
        return None
    md = vector[:, 3:] * np.sqrt(value[3:])
    # G2 = Md Mm' + Mm Md' is linear in Mm; the least solution puts the origin somewhere
    columns = []
    for k in range(3 * MOTIONS):
        unit = np.zeros(3 * MOTIONS)
        unit[k] = 1.0
        mm = unit.reshape(MOTIONS, 3)
        columns.append((md @ mm.T + mm @ md.T)[_UPPER])
    mm = np.linalg.lstsq(np.array(columns).T, g2[_UPPER], rcond=None)[0].reshape(MOTIONS, 3)
    return basis @ np.hstack([md, mm])


def _poses_from(lines: np.ndarray, sensors: Mapping[int, list]) -> np.ndarray:
    """A rotation vector and a position per sensor, stacked, from its
    channels' lines: the frame nearest its measured axes and the point
    nearest its lines."""
    # the directions are known up to a reflection; a sensor's X, Y and Z are right-handed
    hand = [np.linalg.det(np.array([sign * lines[row, :3]
                                    for row, _axis, sign in sorted(members, key=lambda m: m[1])]))
            for members in sensors.values() if len(members) == 3]
    if hand and np.median(hand) < 0:
        lines = lines * np.array([1.0, 1.0, -1.0, -1.0, -1.0, 1.0])
    out = []
    for members in sensors.values():
        frame = _frame_of(members, lines[:, :3])
        moments = np.vstack([-_skew(sign * lines[row, :3]) for row, _a, sign in members])
        where = np.linalg.lstsq(moments, np.concatenate([sign * lines[row, 3:]
                                                         for row, _a, sign in members]),
                                rcond=None)[0]
        out.append(np.concatenate([Rotation.from_matrix(frame.T).as_rotvec(), where]))
    return np.concatenate(out)


def _frame_of(members: Sequence[tuple], along: np.ndarray) -> np.ndarray:
    """The right-handed frame, rows X, Y and Z, nearest the directions
    `along` gives a sensor's channels."""
    target = np.zeros((3, 3))
    for row, axis, sign in members:
        target[axis] = sign * along[row]
    if len(members) == 2:
        a, b = (axis for _row, axis, _sign in members)
        target[3 - a - b] = (1.0 if (b - a) % 3 == 1 else -1.0) * np.cross(target[a], target[b])
    if len(members) == 1:
        target = _completed(members[0][1], target[members[0][1]])
    u, _s, vt = np.linalg.svd(target)
    return u @ np.diag([1.0, 1.0, np.linalg.det(u @ vt)]) @ vt


def _completed(axis: int, along: np.ndarray) -> np.ndarray:
    """A right-handed frame, rows X, Y and Z, whose `axis` row is `along`:
    the other two are the global axis least aligned with it laid square
    to it, and what completes the set."""
    z = along / np.linalg.norm(along)
    x = np.eye(3)[int(np.argmin(np.abs(z)))]
    x = x - (x @ z) * z
    x /= np.linalg.norm(x)
    frame = np.zeros((3, 3))
    frame[axis], frame[(axis + 1) % 3], frame[(axis + 2) % 3] = z, x, np.cross(z, x)
    return frame


def _layout(sensors: Mapping[int, list]) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """Each channel's row, sensor (by place), axis and sign, as arrays."""
    listed = [(row, place, axis, sign) for place, members in enumerate(sensors.values())
              for row, axis, sign in members]
    rows, places, axes, signs = (np.array(column) for column in zip(*listed, strict=True))
    return rows.astype(int), places.astype(int), axes.astype(int), signs.astype(float)


def _lines(poses: np.ndarray, sensors: Mapping[int, list], count: int,
           layout: tuple | None = None) -> np.ndarray:
    """[d, r x d] per channel from the stacked poses."""
    rows, places, axes, signs = layout or _layout(sensors)
    poses = poses.reshape(-1, 6)
    # a rotation matrix's columns are the sensor's axes in the global system
    turned = Rotation.from_rotvec(poses[:, :3]).as_matrix()
    along = signs[:, None] * turned[places, :, axes]
    out = np.zeros((count, MOTIONS))
    out[rows, :3] = along
    out[rows, 3:] = np.cross(poses[places, 3:], along)
    return out


def _refine(basis: np.ndarray, sensors: Mapping[int, list], start: np.ndarray,
            evaluations: int = 200) -> tuple[np.ndarray, float]:
    """Least squares on the poses: what of the measured subspace does not
    lie in the span of the lines the poses give. Returns the poses and
    the misfit, the part left over, 0 to 1. `evaluations` a parameter
    bounds the work."""
    count = basis.shape[0]
    layout = _layout(sensors)
    size = float(np.sqrt(np.mean(start.reshape(-1, 6)[:, 3:] ** 2))) or 1.0

    def left_over(poses: np.ndarray) -> np.ndarray:
        lines = _lines(poses, sensors, count, layout)
        # the two halves of a line are a number and a length: brought to one size, or the
        # projection is taken in a badly scaled basis
        lines[:, 3:] /= size
        q, _r = np.linalg.qr(lines)
        return (basis - q @ (q.T @ basis)).ravel()

    fit = least_squares(left_over, start, method='lm', x_scale='jac',
                        max_nfev=evaluations * len(start))
    return fit.x, float(np.sqrt(2.0 * fit.cost / MOTIONS))


def _started(basis: np.ndarray, groups: Mapping[Any, list], rng: np.random.Generator | None,
             enough: float = POOR_MISFIT) -> tuple[np.ndarray, float, int, bool] | None:
    """A fit of `groups`: its poses, its misfit, how many starts it took
    and whether it had a linear one. With no `rng` only the linear start
    is tried, and None is returned where the groups give too few
    conditions for one. Random starts are tried when there is no linear
    start or it leaves more than `enough`."""
    tried: list[tuple[float, np.ndarray]] = []
    lines = _linear_start(basis, groups)
    if lines is not None:
        poses, misfit = _refine(basis, groups, _poses_from(lines, groups))
        tried.append((misfit, poses))
    if rng is None:
        return (tried[0][1], tried[0][0], 1, True) if tried else None
    if not tried or tried[0][0] > enough:
        for _ in range(AFTER_LINEAR if tried else STARTS):
            start = np.concatenate([np.concatenate([Rotation.random(random_state=rng).as_rotvec(),
                                                    rng.standard_normal(3)])
                                    for _ in groups])
            tried.append(_refine(basis, groups, start, 40)[::-1])
            if tried[-1][0] <= GOOD_MISFIT:
                break
    misfit, poses = min(tried, key=lambda one: one[0])
    return poses, misfit, len(tried), lines is not None


def _left_over(basis: np.ndarray, groups: Mapping[Any, list], poses: np.ndarray) -> np.ndarray:
    """What of each channel's row of the measured subspace the poses'
    lines do not span."""
    fitted = _lines(poses, groups, basis.shape[0])
    size = float(np.sqrt(np.mean(poses.reshape(-1, 6)[:, 3:] ** 2))) or 1.0
    fitted[:, 3:] /= size
    q, _r = np.linalg.qr(fitted)
    return basis - q @ (q.T @ basis)


def _share(left: np.ndarray, basis: np.ndarray, members: Sequence[tuple]) -> float:
    rows = [row for row, _axis, _sign in members]
    whole = float(np.linalg.norm(basis[rows]))
    return float(np.linalg.norm(left[rows]) / whole) if whole else 0.0


def _one_body(lines: np.ndarray, members: Sequence[tuple], size: float) -> list[tuple]:
    """The channels of `members` that are axes of one sensor: the most of
    them whose directions are square to each other (`SQUARE`) and whose
    lines meet (`MEETING` of `size`). Fewer than two is none."""
    def apart(a: tuple, b: tuple) -> tuple[float, float]:
        da, db = lines[a[0], :3], lines[b[0], :3]
        pa, pb = np.cross(da, lines[a[0], 3:]), np.cross(db, lines[b[0], 3:])
        across = np.cross(da, db)
        off = float(np.degrees(np.arcsin(min(1.0, abs(float(da @ db))))))
        length = float(np.linalg.norm(across))
        return off, abs(float((pb - pa) @ across)) / length if length > 1e-9 else np.inf

    pairs = [(a, b) for i, a in enumerate(members) for b in members[i + 1:]]
    scores = {pair: apart(*pair) for pair in pairs}
    fits = [pair for pair, (off, gap) in scores.items() if off <= SQUARE and gap <= MEETING * size]
    if len(fits) == len(pairs):
        return list(members)
    if not fits:
        return []
    return list(min(fits, key=lambda pair: scores[pair][0] / SQUARE + scores[pair][1] / (MEETING * size)))


def _looked_into(basis: np.ndarray, groups: Mapping[int, list], poses: np.ndarray,
                 rng: np.random.Generator | None = None
                 ) -> tuple[dict, np.ndarray, float, set[int], int] | None:
    """A poor fit looked into for DOFs that are labeled wrong.

    The sensor the fit explains worst is released: each of its channels
    is fitted as a sensor of one axis, bound by nothing its node says of
    it. When the rest then fit to within `GOOD_MISFIT`, up to
    `MOST_RELEASED` sensors released, the released channels' own lines
    say what they are. Each release is fitted from the linear start and,
    given `rng`, one that leaves more than `GOOD_MISFIT` from
    `AFTER_LINEAR` random starts as well. A random start is kept only
    when it fits; otherwise another sensor is still wrong, and the next
    is released from where the linear start left it. Those of one node
    that are square to each other and meet are that sensor after all
    (`_one_body`): a right-handed set needed only the better start, and a
    left-handed one has an axis reversed or two exchanged, and is fitted
    with its Z reversed. A channel that is not square to the others of
    its node, or does not pass where they meet, is kept apart from them.

    Returns the groups as they were found to be (a channel kept apart
    under (node, axis)), their poses, the misfit, the nodes fitted
    left-handed and the fits it took; None when releasing does not make
    the rest fit, so the DOFs are not what is wrong.
    """
    count = basis.shape[0]
    trial: dict[Any, list] = dict(groups)
    released: list[int] = []
    fit = None
    tries = 1
    for _ in range(MOST_RELEASED):
        left = _left_over(basis, trial, poses)
        whole = [(_share(left, basis, members), key) for key, members in trial.items()
                 if isinstance(key, int) and len(members) > 1]
        if not whole:
            return None
        worst = max(whole)[1]
        trial = {inner: inner_members
                 for key, members in trial.items()
                 for inner, inner_members in (
                     [((key, axis), [(row, axis, sign)]) for row, axis, sign in members]
                     if key == worst else [(key, members)])}
        # Released, four sensors of three axes leave exactly as many linear conditions as
        # unknowns, so the linear start is set by the noise and the refine from it can settle
        # in a minimum near POOR_MISFIT that depends on the last digits of the data. The lines
        # of a partial fit say nothing about the released channels: that minimum once called
        # every axis of a sound sensor wrong. A random start is kept only when it fits: while
        # another sensor is still wrong, random starts find layouts that fit the rest better
        # than the linear start and point the next release at a sound sensor.
        fit = _started(basis, trial, None)
        if fit is None:
            return None
        taken = 1
        if rng is not None and fit[1] > GOOD_MISFIT:
            again = _started(basis, trial, rng, GOOD_MISFIT)
            taken = again[2]
            if again[1] <= GOOD_MISFIT:
                fit = again
        released.append(worst)
        tries += taken
        poses = fit[0]
        if fit[1] <= GOOD_MISFIT:
            break
    if fit is None or fit[1] > GOOD_MISFIT:
        return None
    lines = _lines(poses, trial, count)
    places = np.array([poses[6 * k + 3:6 * k + 6] for k, (key, members) in enumerate(trial.items())
                       if isinstance(key, int) and len(members) > 1])
    size = (float(np.sqrt(np.mean(np.sum((places - places.mean(axis=0)) ** 2, axis=1))))
            if len(places) > 1 else 1.0) or 1.0
    found: dict[Any, list] = {}
    left_handed: set[int] = set()
    for node, members in groups.items():
        if node not in released:
            found[node] = members
            continue
        body = _one_body(lines, members, size)
        if len(body) == 3:
            axes = np.array([sign * lines[row, :3]
                             for row, _axis, sign in sorted(body, key=lambda m: m[1])])
            if np.linalg.det(axes) < 0:
                body = [(row, axis, -sign if axis == 2 else sign) for row, axis, sign in body]
                left_handed.add(node)
        if body:
            found[node] = body
        kept = {row for row, _axis, _sign in body}
        for row, axis, sign in members:
            if row not in kept:
                found[node, axis] = [(row, axis, sign)]
    again, misfit = _refine(basis, found, _poses_from(lines, found))
    if misfit > POOR_MISFIT:
        return None
    return found, again, misfit, left_handed, tries


def _label_notes(channels: Sequence[Channel], groups: Mapping[Any, list], poses: np.ndarray,
                 left_handed: set[int]) -> tuple[dict[int, str], list[str], list[str]]:
    """What was found wrong with the DOFs: a few words per node for the
    table, the notes in full, and the DOFs left out of their node."""
    keys = list(groups)
    place = {key: poses[6 * k + 3:6 * k + 6] for k, key in enumerate(keys)}
    along = {key: Rotation.from_rotvec(poses[6 * k:6 * k + 3]).as_matrix().T
             for k, key in enumerate(keys)}
    bodies = [key for key, members in groups.items() if isinstance(key, int) and len(members) > 1]
    centers = np.array([place[key] for key in bodies])
    size = (float(np.sqrt(np.mean(np.sum((centers - centers.mean(axis=0)) ** 2, axis=1))))
            if len(bodies) > 1 else 1.0) or 1.0
    short: dict[int, str] = {}
    notes, left_out = [], []
    for node in sorted(left_handed):
        short[node] = 'left-handed as labeled'
        notes.append(f'sensor {node}: its X, Y and Z are a left-handed set as labeled, so one of '
                     'them is reversed or two are exchanged; the data cannot say which. It is '
                     'placed, and the coordinate system it is given has its Z reversed')
    for key, members in groups.items():
        if isinstance(key, int):
            continue
        node, axis = key
        row = members[0][0]
        dof = channels[row].dof
        left_out.append(dof)
        direction = members[0][2] * along[key][axis]
        nearest, gap = None, np.inf
        for other in bodies:
            away = place[other] - place[key]
            off = float(np.linalg.norm(away - (away @ direction) * direction))
            if other != node and off < gap:
                nearest, gap = other, off
        where = (f'its line passes through sensor {nearest}' if gap <= MEETING * size
                 else 'it measures somewhere else')
        with_it = ' and '.join(channels[r].dof for r, _a, _s in groups.get(node, ()))
        notes.append(f'{dof} does not measure at node {node}'
                     + (f' with {with_it}' if with_it else '') + f': {where}. It is left out')
        short[node] = '; '.join(filter(None, [short.get(node, ''), f'{dof} left out']))
    return short, notes, left_out


def fit_poses(motion: Motion, seed: int = 0) -> Recovered:
    """The sensors whose lines span the rigid motion `motion` found.

    The channels of one node are one sensor: its axes at right angles
    and its lines through one point. The fit starts from the linear
    conditions (`_linear_start`) where the sensors give enough of them,
    about twenty, and is taken unless it leaves more than `POOR_MISFIT`
    of the motion unexplained. A poor one is looked into for DOFs labeled
    wrong (`_looked_into`), and what is found is in the notes: a sensor
    whose axes are a left-handed set, and a channel that does not measure
    where its node's others do. A cyclic exchange of a sensor's axes, two
    reversed axes, and the polarity of a sensor of one axis all leave a
    layout that fits, so they are not found.

    Without a linear start, or when a poor fit is not explained by its
    labels, the fit is started from random poses, `STARTS` times or
    `AFTER_LINEAR`, stopping at the first that leaves under
    `GOOD_MISFIT`, and the best is taken; three tri-axial sensors alone
    are placed this way. Every start is run, not only until some agree:
    a wrong layout that fits partly is found from more starts than the
    right one.

    The result is in a frame of its own (`_settled`): the first sensor
    with three axes, or else two, gives the global axes, the middle of
    the sensors is the origin, and the two sensors of two axes or more
    farthest apart are one apart.
    """
    groups: Mapping[Any, list] = _sensors_of(motion.channels)
    basis = motion.basis
    rng = np.random.default_rng(seed)
    left_handed: set[int] = set()
    first = _started(basis, groups, None)
    # the random starts of a release cost seconds on a large layout, so they are tried only
    # when releasing from the linear starts alone does not make the rest fit
    looked = (_looked_into(basis, groups, first[0]) or _looked_into(basis, groups, first[0], rng)
              if first is not None and first[1] > POOR_MISFIT else None)
    if looked is not None:
        groups, poses, misfit, left_handed, starts = looked
        linear = True
    elif first is not None and first[1] <= POOR_MISFIT:
        poses, misfit, starts, linear = first
    else:
        poses, misfit, starts, linear = _started(basis, groups, rng)
    left = _left_over(basis, groups, poses)
    short, said, left_out = _label_notes(motion.channels, groups, poses, left_handed)
    made = []
    for k, (key, members) in enumerate(groups.items()):
        if not isinstance(key, int):
            continue
        frame = Rotation.from_rotvec(poses[6 * k:6 * k + 3]).as_matrix().T
        made.append(Sensor(key, tuple(sorted(axis for _r, axis, _s in members)), frame,
                           poses[6 * k + 3:6 * k + 6].copy(), _share(left, basis, members),
                           reversed=(2,) if key in left_handed else (),
                           note=short.get(key, '')))
    for node in dict.fromkeys(channel.node for channel in motion.channels):
        if node not in groups:
            said.append(f'sensor {node}: no two of its channels measure at one place; it is '
                        'left out')
    notes = [*motion.notes, *said]
    if misfit > POOR_MISFIT:
        notes.append(f'{misfit:.0%} of the motion is not explained by rigid sensors: a channel '
                     'may be on a part that moves on its own, or its DOF may be wrong')
    elif not linear and misfit > GOOD_MISFIT:
        notes.append(f'{starts} starts did not settle on one layout: the best leaves '
                     f'{misfit:.1%} of the motion unexplained and may not be the right one. '
                     'More sensors of two or three axes would settle it')
    return _settled(Recovered(tuple(made), motion, misfit, starts, notes=tuple(notes),
                              left_out=tuple(left_out)))


def _held(sensors: Sequence[Sensor]) -> list[Sensor]:
    """The sensors whose position is a point: two axes or more."""
    return [one for one in sensors if not one.slides]


def _moved(recovered: Recovered, rotation: np.ndarray, scale: float, shift: np.ndarray,
           **changes: Any) -> Recovered:
    """Every sensor at scale * rotation @ position + shift, its axes
    turned with it. A negative scale is the mirror image: the positions
    go through the origin and the axes stay."""
    sensors = tuple(replace(one, position=scale * rotation @ one.position + shift,
                            frame=one.frame @ rotation.T)
                    for one in recovered.sensors)
    return replace(recovered, sensors=sensors, **changes)


def _slid(recovered: Recovered, toward: Mapping[int, np.ndarray] | np.ndarray) -> Recovered:
    """Each sensor of one axis slid along it to the point nearest
    `toward`: one point for all, or a point per node."""
    sensors = []
    for one in recovered.sensors:
        target = toward.get(one.node) if isinstance(toward, Mapping) else toward
        if one.slides and target is not None:
            along = one.frame[one.axes[0]]
            one = replace(one, position=one.position + ((target - one.position) @ along) * along)
        sensors.append(one)
    return replace(recovered, sensors=tuple(sensors))


def _span(sensors: Sequence[Sensor]) -> tuple[float, tuple[int, int] | None]:
    """The distance between the two sensors farthest apart and their nodes."""
    best, pair = 0.0, None
    for i, a in enumerate(sensors):
        for b in sensors[i + 1:]:
            gap = float(np.linalg.norm(a.position - b.position))
            if gap > best:
                best, pair = gap, (a.node, b.node)
    return best, pair


def _settled(recovered: Recovered) -> Recovered:
    """The fit in a frame that does not depend on where it started."""
    held = _held(recovered.sensors) or list(recovered.sensors)
    first = max(held, key=lambda one: len(one.axes))
    middle = np.mean([one.position for one in held], axis=0)
    out = _moved(recovered, first.frame, 1.0, -first.frame @ middle)
    out = _slid(out, np.zeros(3))
    span, pair = _span(_held(out.sensors) or list(out.sensors))
    if span <= 0:
        raise ValueError('the sensors recovered all lie at one point: the motion does not '
                         'tell them apart')
    # of the two images, the one whose farthest sensor has its largest coordinate positive
    far = max(_held(out.sensors) or list(out.sensors),
              key=lambda one: float(np.linalg.norm(one.position)))
    sign = 1.0 if far.position[int(np.argmax(np.abs(far.position)))] >= 0 else -1.0
    return _moved(out, np.eye(3), sign / span, np.zeros(3), span_nodes=pair,
                  length_unit=None, image_open=True)


def place(recovered: Recovered, *, span: float | None = None,
          known: Mapping[int, Sequence[float]] | None = None,
          length_unit: str | None = None, mirror: bool = False) -> Recovered:
    """The sensors at their size, and in a known geometry's frame where
    one is given.

    `known` is {node: its position} for the nodes already placed. With
    three of them that are not in a line among the sensors of two axes
    or more, the sensors are turned, scaled and moved onto them, and a
    sensor of one axis that is among them is slid along its axis to the
    point nearest its node. With four that are not in a plane the mirror
    image is settled too; with fewer it stays open, and `mirror` takes
    the other one. Two known nodes give the size alone.

    `span` is the distance between the two sensors farthest apart
    (`Recovered.span_nodes`), for when no node is known.

    Raises
    ------
    ValueError
        If both `span` and `known` are given, `span` is not positive, or
        `known` shares fewer than two nodes with the sensors of two axes
        or more, `mirror` is asked for when the known nodes settle the
        image, or the sensors were turned and not placed, which `turn`
        sets in a known geometry.
    """
    if recovered.turned_only:
        raise ValueError('the sensors were turned by three translations and not placed: there '
                         'is no layout to size or to move onto known nodes')
    out = _moved(recovered, np.eye(3), -1.0, np.zeros(3)) if mirror else recovered
    if span is not None and known is not None:
        raise ValueError('the size comes from a known geometry or from a span, not both')
    if span is not None:
        if not span > 0:
            raise ValueError(f'a span is a length above zero, not {span:g}')
        now, _pair = _span(_held(out.sensors) or list(out.sensors))
        return _moved(out, np.eye(3), span / now, np.zeros(3), length_unit=length_unit)
    if known is None:
        return out
    points = {int(node): np.asarray(xyz, dtype=float) for node, xyz in known.items()}
    shared = [one for one in _held(out.sensors) if one.node in points]
    if len(shared) < 2:
        raise ValueError(f'the known geometry shares {len(shared)} node'
                         f'{"s" * (len(shared) != 1)} with the sensors of two axes or more; two '
                         'give the size and three that are not in a line give the frame')
    found = np.array([one.position for one in shared])
    truth = np.array([points[one.node] for one in shared])
    spread = np.linalg.svd(truth - truth.mean(axis=0), compute_uv=False)
    in_line = len(shared) < 3 or spread[1] < 1e-6 * spread[0]
    flat = in_line or spread[2] < 1e-6 * spread[0]
    if mirror and not flat:
        raise ValueError('the known nodes settle which mirror image it is; there is nothing '
                         'for Mirror to choose')
    if in_line:
        scale = (np.linalg.norm(truth - truth.mean(axis=0))
                 / np.linalg.norm(found - found.mean(axis=0)))
        notes = (*out.notes, ('the known nodes are in a line: they give the size, and the frame '
                 'is the first sensor\'s'))
        return _moved(out, np.eye(3), float(scale), np.zeros(3), length_unit=length_unit,
                      notes=notes)
    fits = [_onto(found, truth, sign) for sign in (1.0, -1.0)]
    # known nodes in a plane fit both images alike, so they do not choose
    pick = 0 if flat or fits[0][3] <= fits[1][3] else 1
    rotation, scale, shift, _miss = fits[pick]
    moved = _moved(out, rotation, scale, shift, length_unit=length_unit, image_open=flat,
                   framed=True)
    return _slid(moved, {node: xyz for node, xyz in points.items()})


def _onto(found: np.ndarray, truth: np.ndarray, sign: float
          ) -> tuple[np.ndarray, float, np.ndarray, float]:
    """The rotation, scale (of `sign`) and shift that take `found` onto
    `truth` in least squares, and what is left."""
    a = sign * (found - found.mean(axis=0))
    b = truth - truth.mean(axis=0)
    u, s, vt = np.linalg.svd(b.T @ a)
    turn = np.diag([1.0, 1.0, np.linalg.det(u @ vt)])
    rotation = u @ turn @ vt
    scale = sign * float(np.sum(s * np.diag(turn)) / np.sum(a * a))
    shift = truth.mean(axis=0) - scale * rotation @ found.mean(axis=0)
    left = float(np.linalg.norm(scale * found @ rotation.T + shift - truth))
    return rotation, scale, shift, left


# ---- sensors that are turned and not placed --------------------------------


def _directions_start(basis: np.ndarray, sensors: Mapping[int, list]) -> np.ndarray | None:
    """The direction of every channel from the linear conditions, or
    None when they do not settle them.

    With directions = basis M, a direction is a unit vector and the axes
    of one sensor are at right angles: both are linear in the symmetric
    G = M M'. M follows from G up to a turn and the mirror image."""
    upper = np.triu_indices(TRANSLATIONS)
    rows, ones = [], []
    for members in sensors.values():
        for i, (ri, _ai, _si) in enumerate(members):
            for rj, _aj, _sj in members[i:]:
                m = np.outer(basis[ri], basis[rj])
                m = m + m.T
                m[np.diag_indices(TRANSLATIONS)] /= 2.0
                rows.append(m[upper])
                ones.append(1.0 if ri == rj else 0.0)
    solved, _left, rank, _singular = np.linalg.lstsq(np.array(rows), np.array(ones), rcond=None)
    if rank < len(upper[0]):
        return None
    g = np.zeros((TRANSLATIONS, TRANSLATIONS))
    g[upper] = solved
    value, vector = np.linalg.eigh(g + np.triu(g, 1).T)
    if value[0] <= 0:
        return None
    return basis @ (vector * np.sqrt(value))


def _directions(turns: np.ndarray, layout: tuple, count: int) -> np.ndarray:
    """The direction of every channel from the sensors' rotation vectors."""
    rows, places, axes, signs = layout
    turned = Rotation.from_rotvec(turns.reshape(-1, 3)).as_matrix()
    out = np.zeros((count, TRANSLATIONS))
    out[rows] = signs[:, None] * turned[places, :, axes]
    return out


@dataclass(frozen=True)
class _Turns:
    """A fit of sensors that only translate: the groups as fitted, a
    left-handed set with its Z reversed, their rotation vectors, the
    misfit, what is left of each channel's row, the nodes found
    left-handed, and whether as many sets were of each hand."""

    groups: dict[Any, list]
    turns: np.ndarray
    misfit: float
    left: np.ndarray
    left_handed: frozenset[int]
    tied: bool


def _turned_fit(basis: np.ndarray, groups: Mapping[Any, list]) -> _Turns | None:
    """The rotation of each group of `groups` whose channels' directions
    span the three motions of `basis` best, or None where the channels do
    not settle them. The image is the one in which most sets of three
    axes are right-handed, the first one's when as many are of each hand;
    a set of the other hand is fitted with its Z reversed."""
    count = basis.shape[0]
    along = _directions_start(basis, groups)
    if along is None:
        return None
    triads = {key: float(np.linalg.det(np.array(
        [sign * along[row] for row, _axis, sign in sorted(members, key=lambda m: m[1])])))
        for key, members in groups.items() if len(members) == 3}
    right = sum(hand > 0 for hand in triads.values())
    tied = bool(triads) and 2 * right == len(triads)
    if triads and (2 * right < len(triads) or (tied and next(iter(triads.values())) < 0)):
        along = along * np.array([1.0, 1.0, -1.0])
        triads = {key: -hand for key, hand in triads.items()}
    left_handed = frozenset(key for key, hand in triads.items() if hand < 0)
    fitted = {key: [(row, axis, -sign if axis == 2 and key in left_handed else sign)
                    for row, axis, sign in members] for key, members in groups.items()}
    layout = _layout(fitted)
    start = np.concatenate([Rotation.from_matrix(_frame_of(members, along).T).as_rotvec()
                            for members in fitted.values()])

    def left_over(turns: np.ndarray) -> np.ndarray:
        q, _r = np.linalg.qr(_directions(turns, layout, count))
        return (basis - q @ (q.T @ basis)).ravel()

    fit = least_squares(left_over, start, method='lm', x_scale='jac', max_nfev=200 * len(start))
    return _Turns(fitted, fit.x, float(np.sqrt(2.0 * fit.cost / TRANSLATIONS)),
                  left_over(fit.x).reshape(basis.shape), left_handed, tied)


def _turns_looked_into(channels: Sequence[Channel], basis: np.ndarray, groups: Mapping[int, list],
                       fit: _Turns) -> tuple[_Turns, list[str]] | None:
    """A poor fit of sensors that only translate, looked into for a
    channel that is not an axis of the sensor its DOF puts it in.

    As `_looked_into` does for sensors that are placed: the sensor the
    fit explains worst is released, each of its channels fitted alone,
    up to `MOST_RELEASED` sensors, until the rest fit. The released
    channels of a node that read within `SQUARE` of a right angle to each
    other are that sensor after all, and one that does not is kept apart
    from it under (node, axis). Returns the fit of the groups as found
    and what to say of each channel kept apart; None when releasing does
    not make the rest fit, so the DOFs are not what is wrong.
    """
    trial: dict[Any, list] = dict(groups)
    released: list[int] = []
    for _ in range(MOST_RELEASED):
        whole = [(_share(fit.left, basis, members), key) for key, members in trial.items()
                 if isinstance(key, int) and len(members) > 1]
        if not whole:
            return None
        worst = max(whole)[1]
        trial = {inner: inner_members
                 for key, members in trial.items()
                 for inner, inner_members in (
                     [((key, axis), [(row, axis, sign)]) for row, axis, sign in members]
                     if key == worst else [(key, members)])}
        again = _turned_fit(basis, trial)
        if again is None:
            return None
        fit = again
        released.append(worst)
        if fit.misfit <= POOR_MISFIT:
            break
    if fit.misfit > POOR_MISFIT:
        return None
    along = _directions(fit.turns, _layout(fit.groups), basis.shape[0])

    def off_square(a: tuple, b: tuple) -> float:
        return float(np.degrees(np.arcsin(min(1.0, abs(float(along[a[0]] @ along[b[0]]))))))

    found: dict[Any, list] = {}
    said: list[str] = []
    for node, members in groups.items():
        if node not in released:
            found[node] = members
            continue
        pairs = [(a, b) for i, a in enumerate(members) for b in members[i + 1:]]
        square = [pair for pair in pairs if off_square(*pair) <= SQUARE]
        body = (list(members) if len(square) == len(pairs)
                else list(min(square, key=lambda pair: off_square(*pair))) if square else [])
        if body:
            found[node] = body
        kept = {row for row, _axis, _sign in body}
        for member in members:
            if member[0] in kept:
                continue
            found[node, member[1]] = [member]
            if body:
                said.append(f'{channels[member[0]].dof} does not read at right angles to '
                            f'{" and ".join(channels[other[0]].dof for other in body)}: it is '
                            f'{max(off_square(member, other) for other in body):.0f} degrees '
                            'off square. It is left out')
        if not body:
            said.append(f'sensor {node}: {" and ".join(channels[row].dof for row, _a, _s in members)}'
                        ' do not read at right angles to each other, '
                        f'{max(off_square(*pair) for pair in pairs):.0f} degrees off square, so '
                        'they are not one sensor. It is left out')
    final = _turned_fit(basis, found)
    if final is None or final.misfit > POOR_MISFIT:
        return None
    return final, said


def fit_directions(motion: Motion) -> Recovered:
    """How each sensor is turned, from three translations.

    A channel along the unit vector d reads d . a0(t) of a body that only
    translates, wherever it is, so its row of the three motions `motion`
    found is d in some basis. The basis is the one in which every
    direction is a unit vector and the axes of one sensor are at right
    angles, found from those conditions as linear ones and then by least
    squares on a rotation per sensor. Nothing places a sensor: every
    position is not a number, and `turn` takes positions from a geometry
    that holds the nodes.

    The directions are known up to one turn of them all and their mirror
    image. The frame is that of the first sensor with the most axes. The
    image is the one in which the sensors of three axes are right-handed
    sets; one of the other hand than most is said to be left-handed as
    labeled and given a frame with its Z reversed. With no sensor of
    three axes the image is open.

    A fit that leaves more than `POOR_MISFIT` unexplained is looked into
    for a channel that is not at right angles to the others of its node
    (`_turns_looked_into`); such a channel is left out of the sensor and
    said. Where a channel is cannot be read from translations, so one
    labeled at another node that happens to be square to its axes is not
    found.

    A rotation the tests excite with their translation, in step with it,
    is read as part of the translation: it tilts each sensor's directions
    by about the rotation's share of the motion across the sensors, and
    leaves about that share unexplained.

    Raises
    ------
    ValueError
        If the directions are not settled by the channels there are, or
        sensors that only translate leave more than `NOT_TRANSLATIONS` of
        the three motions unexplained, so the three are not the
        translations: the message is why six motions were not read, with
        that added.
    """
    groups: Mapping[Any, list] = _sensors_of(motion.channels)
    basis = motion.basis
    fit = _turned_fit(basis, groups)
    if fit is None:
        raise ValueError(f'{motion.without_six} Three motions do stand clear, but the channels '
                         'do not settle how the sensors are turned: they are along too few '
                         'directions')
    said: list[str] = []
    if fit.misfit > POOR_MISFIT:
        looked = _turns_looked_into(motion.channels, basis, groups, fit)
        if looked is not None:
            fit, said = looked
    misfit = fit.misfit
    if misfit > NOT_TRANSLATIONS:
        raise ValueError(f'{motion.without_six} Three motions do stand clear, but sensors that '
                         f'only translate leave {misfit:.0%} of them unexplained, so they are '
                         'not the three translations')
    frames = Rotation.from_rotvec(fit.turns.reshape(-1, 3)).as_matrix().transpose(0, 2, 1)
    bodies = [(frame, key, members) for frame, (key, members)
              in zip(frames, fit.groups.items(), strict=True) if isinstance(key, int)]
    first = max(bodies, key=lambda body: len(body[2]))[0]
    left_out = [motion.channels[members[0][0]].dof for key, members in fit.groups.items()
                if not isinstance(key, int)]
    made = []
    for frame, node, members in bodies:
        share = _share(fit.left, basis, members)
        short = ['left-handed as labeled'] if node in fit.left_handed else []
        short += [f'{dof} left out' for dof in left_out if parse_dof(dof)[0] == node]
        if node in fit.left_handed:
            said.append(
                f'sensor {node}: its X, Y and Z are a left-handed set as labeled, so one of them '
                'is reversed or two are exchanged; the data cannot say which. The coordinate '
                'system it is given has its Z reversed'
                + ('. As many sensors are of each hand, and the first is taken as right-handed'
                   if fit.tied else ''))
        elif share > POOR_MISFIT >= misfit:
            said.append(f'sensor {node}: {share:.0%} of its motion is not explained by a sensor '
                        'that translates with the others: one of its channels may be labeled '
                        'wrong, or it is on a part that moves on its own')
        made.append(Sensor(node, tuple(sorted(axis for _r, axis, _s in members)),
                           frame @ first.T, np.full(3, np.nan), share,
                           reversed=(2,) if node in fit.left_handed else (),
                           note='; '.join(short)))
    notes = [*motion.notes, *said]
    if misfit > POOR_MISFIT:
        notes.append(f'{misfit:.0%} of the motion is not explained by sensors that only '
                     'translate: the tests may turn the structure as they move it, which tilts '
                     f'the directions read by about {np.degrees(misfit):.0f} degrees, or a DOF '
                     'may be wrong')
    return Recovered(tuple(made), motion, misfit, 1, notes=tuple(notes),
                     image_open=not any(len(members) == 3 for _f, _n, members in bodies),
                     left_out=tuple(left_out), turned_only=True)


def _turn_onto(found: np.ndarray, truth: np.ndarray, proper: bool) -> np.ndarray:
    """The rotation, or with `proper` False the reflection, that takes the
    directions `found` nearest `truth`."""
    u, _s, vt = np.linalg.svd(truth.T @ found)
    hand = np.linalg.det(u @ vt)
    return u @ np.diag([1.0, 1.0, hand if proper else -hand]) @ vt


def _angles(found: np.ndarray, truth: np.ndarray) -> np.ndarray:
    """The angle between each pair of unit vectors, degrees."""
    return np.degrees(np.arccos(np.clip(np.sum(found * truth, axis=1), -1.0, 1.0)))


def _carried(frame: np.ndarray, axis: int, along: np.ndarray) -> np.ndarray:
    """`frame` turned the least that brings its `axis` row onto `along`."""
    was = frame[axis]
    about = np.cross(was, along)
    sine, cosine = float(np.linalg.norm(about)), float(was @ along)
    if sine < 1e-9:
        return frame if cosine > 0 else _completed(axis, along)
    turn = Rotation.from_rotvec(about / sine * np.arctan2(sine, cosine)).as_matrix()
    return frame @ turn.T


def turn(recovered: Recovered, *, frames: Mapping[int, np.ndarray],
         positions: Mapping[int, Sequence[float]] | None = None,
         length_unit: str | None = None) -> Recovered:
    """Sensors that were turned and not placed (`fit_directions`), set in
    the frame of a geometry that holds their nodes.

    `frames` is {node: (3, 3)}, the X, Y and Z the geometry measures each
    node in, as rows. The one turn of all the directions that the data
    leaves open is the one under which the most channels read within
    `AGREES` of their directions there, fitted to those channels. Every
    channel compared is then given how far it reads from its direction
    in the geometry (`Recovered.off`), and one further than `AGREES` is
    said: its polarity reversed, read along another axis of its node, or
    how many degrees off. A sensor of one axis keeps the geometry's other
    two axes, turned the least that brings the measured one into line.

    `positions` is {node: xyz}: a sensor whose node is in it is put
    there, and the others stay without a place.

    Raises
    ------
    ValueError
        If `recovered` was placed, or `frames` gives the directions of
        fewer than two channels that are not parallel.
    """
    if not recovered.turned_only:
        raise ValueError('the sensors were placed by six rigid motions; `place` sets those in a '
                         'known geometry')
    listed = [(channel, recovered.reads_along(channel)) for channel in recovered.motion.channels
              if channel.node in frames and channel.dof not in recovered.left_out]
    found = np.array([along for _channel, along in listed]).reshape(-1, 3)
    truth = np.array([channel.sign * np.asarray(frames[channel.node], dtype=float)[channel.axis]
                      for channel, _along in listed]).reshape(-1, 3)
    if len(listed) < 2 or np.linalg.svd(truth, compute_uv=False)[1] < 1e-6:
        raise ValueError(f'the known geometry gives the direction of {len(listed)} channel'
                         f'{"s" * (len(listed) != 1)} that are not parallel; two are needed to '
                         'set the sensors in its frame')
    by_node: dict[int, list[int]] = {}
    for row, (channel, _along) in enumerate(listed):
        by_node.setdefault(channel.node, []).append(row)
    tries = [rows for rows in by_node.values() if len(rows) > 1]
    lone = [rows[0] for rows in by_node.values() if len(rows) == 1]
    tries += [[a, b] for a, b in itertools.islice(itertools.combinations(lone, 2), PAIRS)]
    hands = (True, False) if recovered.image_open else (True,)
    best: tuple[int, float, np.ndarray, bool] | None = None
    for rows, proper in itertools.product(tries, hands):
        rotation = _turn_onto(found[rows], truth[rows], proper)
        for _ in range(3):
            near = _angles(found @ rotation.T, truth) <= AGREES
            if np.count_nonzero(near) < 2:
                break
            rotation = _turn_onto(found[near], truth[near], proper)
        angles = _angles(found @ rotation.T, truth)
        near = angles <= AGREES
        score = (int(np.count_nonzero(near)), -float(np.sum(angles[near] ** 2)))
        if best is None or score > best[:2]:
            best = (*score, rotation, proper)
    assert best is not None
    agreeing, _spread, rotation, proper = best
    angles = _angles(found @ rotation.T, truth)
    flat = np.linalg.svd(truth[angles <= AGREES], compute_uv=False)
    image_open = recovered.image_open and (len(flat) < 3 or flat[2] < 1e-6 * flat[0])
    points = {int(node): np.asarray(xyz, dtype=float) for node, xyz in (positions or {}).items()}
    off = {channel.dof: float(angle) for (channel, _a), angle in zip(listed, angles, strict=True)}
    said: list[str] = []
    short: dict[int, list[str]] = {}
    # a sensor's channels in the order of its axes, whatever order the records are in
    for (channel, along), angle in sorted(zip(listed, angles, strict=True),
                                          key=lambda one: (one[0][0].node, one[0][0].axis)):
        if angle <= AGREES:
            continue
        reads = rotation @ along
        frame = np.asarray(frames[channel.node], dtype=float)
        letter = 'XYZ'[channel.axis]
        if angle >= 180.0 - AGREES:
            said.append(f'{channel.dof} reads opposite to its direction in the geometry: its '
                        'polarity is reversed')
            short.setdefault(channel.node, []).append(f'{letter} reversed')
            continue
        cosines = frame @ reads
        other = int(np.argmax(np.abs(cosines)))
        if np.degrees(np.arccos(min(1.0, abs(float(cosines[other]))))) <= AGREES:
            name = f'{"XYZ"[other]}{"+" if cosines[other] > 0 else "-"}'
            said.append(f'{channel.dof} reads along {channel.node}{name} of the geometry')
            short.setdefault(channel.node, []).append(f'{letter} reads along {name}')
        else:
            said.append(f'{channel.dof} reads {angle:.0f} degrees from its direction in the '
                        'geometry')
            short.setdefault(channel.node, []).append(f'{letter} {angle:.0f} degrees off')
    sensors = []
    for one in recovered.sensors:
        frame = one.frame @ rotation.T
        if one.slides and one.node in frames:
            axis = one.axes[0]
            frame = _carried(np.asarray(frames[one.node], dtype=float), axis, frame[axis])
        note = '; '.join(filter(None, [one.note, *short.get(one.node, [])]))
        sensors.append(replace(one, frame=frame, note=note,
                               position=points.get(one.node, one.position)))
    notes = list(recovered.notes)
    if 2 * agreeing < len(listed):
        notes.append(f'only {agreeing} of {len(listed)} channels read within {AGREES:g} degrees '
                     'of their directions in the geometry under any one turn of them all: the '
                     'frame is taken from those and may not be the right one')
    notes += said
    apart = [one.node for one in sensors if not one.placed]
    if points and apart:
        notes.append(f'{", ".join(str(node) for node in apart[:6])}'
                     f'{" and others" * (len(apart) > 6)}: not in the known geometry, so turned '
                     'and left without a place')
    return replace(recovered, sensors=tuple(sensors), notes=tuple(notes), off=off,
                   agree=(agreeing, len(listed)), image_open=image_open,
                   length_unit=length_unit if points else None,
                   framed=any(one.placed for one in sensors))


def recover(histories: Sequence[Any], band: tuple[float, float] | None = None, *,
            records: Sequence[Sequence[int] | None] | None = None,
            averagings: Sequence[Any] | None = None, seed: int = 0) -> Recovered:
    """`rigid_motion` then `fit_poses`: the sensors of one or several
    time histories, from their motion alone. Where three motions stand
    clear and six do not, `fit_directions`: the sensors turned and not
    placed."""
    motion = rigid_motion(histories, band, records=records, averagings=averagings)
    return fit_poses(motion, seed) if motion.count == MOTIONS else fit_directions(motion)


def assemble(recovered: Recovered) -> Any:
    """The sensors as a geometry: a node per sensor, numbered as its DOFs
    name it, placed in the global system and measured in a cartesian
    system of its own at the node, named ``Sensor <node>``, whose rows
    are the sensor's X, Y and Z. Its length unit is the recovered one,
    or none when no size was given. Of sensors that were turned and not
    placed, those a known geometry gave a position are in it.

    Raises
    ------
    ValueError
        If no sensor has a position.
    """
    from .geometry import Geometry

    sensors = tuple(one for one in recovered.sensors if one.placed)
    if not sensors:
        raise ValueError('the data holds three translations and no rotation, so it says how '
                         'each sensor is turned and not where it is: pick a geometry that holds '
                         'the nodes, and the sensors are given its positions')
    count = len(sensors)
    centers = np.array([one.position for one in sensors])
    cs_matrix = np.zeros((count + 1, 4, 3))
    cs_matrix[0, :3] = np.eye(3)
    cs_matrix[1:, :3] = np.array([one.frame for one in sensors])
    cs_matrix[1:, 3] = centers
    return Geometry(
        node_id=[one.node for one in sensors], node_xyz=centers,
        node_def_cs=np.ones(count, dtype=np.int64),
        node_disp_cs=np.arange(2, count + 2),
        cs_id=np.arange(1, count + 2),
        cs_name=['', *(f'Sensor {one.node}' for one in sensors)],
        cs_type=np.zeros(count + 1, dtype=np.int64),
        cs_matrix=cs_matrix, length_unit=recovered.length_unit)
