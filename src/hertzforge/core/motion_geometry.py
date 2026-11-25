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
    orthonormal directions the channels move in together in `band`, Hz.
    `bands` are the candidate bands' edges, Hz, and `values` (bands, k)
    the amplitudes of the strongest motions in each, largest first, the
    evidence the band was chosen on. `notes` say what was left out or
    could not be done."""

    channels: tuple[Channel, ...]
    basis: np.ndarray
    band: tuple[float, float]
    bands: np.ndarray
    values: np.ndarray
    lines: int
    notes: tuple[str, ...] = ()


@dataclass(frozen=True)
class Sensor:
    """One sensor recovered: its node, the axes it measures (0 X, 1 Y,
    2 Z), `frame` (3, 3) whose rows are its X, Y and Z in the global
    system, and its position. A sensor with one axis is somewhere on the
    line through `position` along that axis, and the two axes it does
    not measure are any two that complete the frame."""

    node: int
    axes: tuple[int, ...]
    frame: np.ndarray
    position: np.ndarray
    #: the part of its channels' rigid motion the fit leaves unexplained
    misfit: float = 0.0

    @property
    def slides(self) -> bool:
        """Whether its place along its own axis is open."""
        return len(self.axes) == 1


@dataclass(frozen=True)
class Recovered:
    """The sensors recovered from a rigid motion. Positions are in
    `length_unit`, or in no unit when it is None, the two sensors of two
    axes or more that are farthest apart (`span_nodes`) then one apart.
    `misfit` is the part of the measured motion the sensors as placed do
    not explain, 0 to 1. `image_open` says the mirror image fits the
    data as well and nothing chose between them."""

    sensors: tuple[Sensor, ...]
    motion: Motion
    misfit: float
    starts: int
    length_unit: str | None = None
    image_open: bool = True
    span_nodes: tuple[int, int] | None = None
    notes: tuple[str, ...] = field(default=())

    def sensor(self, node: int) -> Sensor:
        """The sensor at `node`."""
        return next(one for one in self.sensors if one.node == node)


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
    values = np.array([_amplitudes(pool.cospectrum(low, high))[0][:shown]
                       for low, high in itertools.pairwise(edges)]).reshape(-1, shown)
    given = band is not None
    if band is None:
        band = _clear_band(edges, values, len(histories))
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
        raise ValueError(_too_few(judged, low, high, len(histories)))
    notes = list(notes)
    basis = _at_zero(pool, low, high, lines, notes)
    return Motion(tuple(channels), basis, (low, high), edges, values, lines, tuple(notes))


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


def _at_zero(pool: _Pool, low: float, high: float, lines: int, notes: list[str]) -> np.ndarray:
    """The six-dimensional subspace of the band, taken to zero frequency
    where the band has the lines for it."""
    parts = min(SUB_BANDS, lines // SUB_BAND_LINES)
    if parts < 3:
        notes.append(f'the band holds {lines} line{"s" * (lines != 1)}, too few to take the '
                     'bending out of it: lengthen the frames or widen the band')
        return _amplitudes(pool.cospectrum(low, high))[1][:, :MOTIONS]
    edges = np.linspace(low, high, parts + 1)
    projectors, squares = [], []
    for a, b in itertools.pairwise(edges):
        vectors = _amplitudes(pool.cospectrum(a, b))[1][:, :MOTIONS]
        projectors.append(vectors @ vectors.T)
        squares.append((a * a + a * b + b * b) / 3.0)
    design = np.column_stack([np.ones(parts), np.array(squares) / squares[-1]])
    fitted = np.linalg.lstsq(design, np.array(projectors).reshape(parts, -1), rcond=None)[0][0]
    side = projectors[0].shape[0]
    zero = fitted.reshape(side, side)
    return _amplitudes((zero + zero.T) / 2.0)[1][:, :MOTIONS]


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
        target = np.zeros((3, 3))
        for row, axis, sign in members:
            target[axis] = sign * lines[row, :3]
        if len(members) == 2:
            a, b = (axis for _row, axis, _sign in members)
            target[3 - a - b] = (1.0 if (b - a) % 3 == 1 else -1.0) * np.cross(target[a], target[b])
        if len(members) == 1:
            target = _completed(members[0][1], target[members[0][1]])
        u, _s, vt = np.linalg.svd(target)
        frame = u @ np.diag([1.0, 1.0, np.linalg.det(u @ vt)]) @ vt
        moments = np.vstack([-_skew(sign * lines[row, :3]) for row, _a, sign in members])
        where = np.linalg.lstsq(moments, np.concatenate([sign * lines[row, 3:]
                                                         for row, _a, sign in members]),
                                rcond=None)[0]
        out.append(np.concatenate([Rotation.from_matrix(frame.T).as_rotvec(), where]))
    return np.concatenate(out)


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


def fit_poses(motion: Motion, seed: int = 0) -> Recovered:
    """The sensors whose lines span the rigid motion `motion` found.

    The channels of one node are one sensor: its axes at right angles
    and its lines through one point. The fit starts from the linear
    conditions (`_linear_start`) where the sensors give enough of them,
    about twenty, and is taken unless it leaves more than `POOR_MISFIT`
    of the motion unexplained. Without that start, or after a poor one,
    it is started from random poses, `STARTS` times or `AFTER_LINEAR`,
    stopping at the first that leaves under `GOOD_MISFIT`, and the best
    is taken; three
    tri-axial sensors alone are placed this way. Every start is run, not
    only until some agree: a wrong layout that fits partly is found from
    more starts than the right one.

    The result is in a frame of its own (`_settled`): the first sensor
    with three axes, or else two, gives the global axes, the middle of
    the sensors is the origin, and the two sensors of two axes or more
    farthest apart are one apart.
    """
    sensors = _sensors_of(motion.channels)
    basis = motion.basis
    count = basis.shape[0]
    rng = np.random.default_rng(seed)
    tried: list[tuple[float, np.ndarray]] = []
    lines = _linear_start(basis, sensors)
    if lines is not None:
        poses, misfit = _refine(basis, sensors, _poses_from(lines, sensors))
        tried.append((misfit, poses))
    if not tried or tried[0][0] > POOR_MISFIT:
        for _ in range(AFTER_LINEAR if tried else STARTS):
            start = np.concatenate([np.concatenate([Rotation.random(random_state=rng).as_rotvec(),
                                                    rng.standard_normal(3)])
                                    for _ in sensors])
            tried.append(_refine(basis, sensors, start, 40)[::-1])
            if tried[-1][0] <= GOOD_MISFIT:
                break
    misfit, poses = min(tried, key=lambda one: one[0])
    fitted = _lines(poses, sensors, count)
    size = float(np.sqrt(np.mean(poses.reshape(-1, 6)[:, 3:] ** 2))) or 1.0
    scaled = fitted * np.array([1.0, 1.0, 1.0, 1 / size, 1 / size, 1 / size])
    q, _r = np.linalg.qr(scaled)
    left = basis - q @ (q.T @ basis)
    made = []
    for k, (node, members) in enumerate(sensors.items()):
        rows = [row for row, _a, _s in members]
        frame = Rotation.from_rotvec(poses[6 * k:6 * k + 3]).as_matrix().T
        whole = float(np.linalg.norm(basis[rows]))
        made.append(Sensor(node, tuple(sorted(axis for _r, axis, _s in members)), frame,
                           poses[6 * k + 3:6 * k + 6].copy(),
                           float(np.linalg.norm(left[rows]) / whole) if whole else 0.0))
    notes = list(motion.notes)
    if misfit > POOR_MISFIT:
        notes.append(f'{misfit:.0%} of the motion is not explained by rigid sensors: a channel '
                     'may be on a part that moves on its own, or its DOF may be wrong')
    elif lines is None and misfit > GOOD_MISFIT:
        notes.append(f'{len(tried)} starts did not settle on one layout: the best leaves '
                     f'{misfit:.1%} of the motion unexplained and may not be the right one. '
                     'More sensors of two or three axes would settle it')
    return _settled(Recovered(tuple(made), motion, misfit, len(tried), notes=tuple(notes)))


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
        or more, or `mirror` is asked for when the known nodes settle the
        image.
    """
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
    moved = _moved(out, rotation, scale, shift, length_unit=length_unit, image_open=flat)
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


def recover(histories: Sequence[Any], band: tuple[float, float] | None = None, *,
            records: Sequence[Sequence[int] | None] | None = None,
            averagings: Sequence[Any] | None = None, seed: int = 0) -> Recovered:
    """`rigid_motion` then `fit_poses`: the sensors of one or several
    time histories, from their motion alone."""
    return fit_poses(rigid_motion(histories, band, records=records, averagings=averagings), seed)


def assemble(recovered: Recovered) -> Any:
    """The sensors as a geometry: a node per sensor, numbered as its DOFs
    name it, placed in the global system and measured in a cartesian
    system of its own at the node, named ``Sensor <node>``, whose rows
    are the sensor's X, Y and Z. Its length unit is the recovered one,
    or none when no size was given."""
    from .geometry import Geometry

    sensors = recovered.sensors
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
