"""Accelerometers fitted to points measured on them.

A coordinate measuring arm is touched to the faces of each accelerometer
and the points of each surface are exported as one feature, named
``<sensor>_<label>``. The label says which surface it is:

- ``1``, ``2``, ``3`` and ``-1``, ``-2``, ``-3``: a flat face whose
  outward normal is the accelerometer's X+, Y+, Z+ or X-, Y-, Z- axis.
- ``c``: the round side of a uni-axial accelerometer. Its sensor then has
  one flat face as well, the top, whose label names the axis that points
  out of it (``p`` is read as ``3``).

A tri-axial accelerometer is located by the planes fitted to two or
three of its faces, a uni-axial one by the cylinder fitted to its side
and the plane fitted to its top. Each fit says how good it is, and what
is wrong with it when something is.

Lengths are in the unit of the points. The limits on lengths
(`PLANE_RMS`, `CYLINDER_RMS`) are in meters and are applied only when
the caller says how many meters one unit is.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field

import numpy as np
from scipy.optimize import least_squares

from .instrumentation import surface_frame

#: a plane's residual RMS above this is warned about, in meters. Probed
#: faces came out between 0.0003 and 0.013 mm, a face scanned with many
#: points at 0.031 mm
PLANE_RMS = 0.05e-3

#: a cylinder's residual RMS above this is warned about, in meters
CYLINDER_RMS = 0.05e-3

#: two faces of one accelerometer further than this from square are
#: warned about, in degrees
FACE_ANGLE = 1.0

#: a uni-axial accelerometer's top face further than this from square
#: to its side is warned about, in degrees
TOP_ANGLE = 3.0

#: a cylinder measured over less than this much of its circumference is
#: warned about, in degrees: a short arc fixes the radius poorly
ARC = 120.0

#: the labels of a flat face, and of a cylinder
FACE_LABELS = ('1', '2', '3', '-1', '-2', '-3')
CYLINDER_LABEL = 'c'


@dataclass(frozen=True)
class Flag:
    """Something wrong with a fit: `level` is 'warn' or 'error'. A sensor
    with an error cannot be placed."""

    level: str
    text: str


@dataclass(frozen=True)
class PlaneFit:
    """A plane through points: a point on it (their centroid), its unit
    normal (either way round), the residuals' RMS and largest size, and
    how many points."""

    point: np.ndarray
    normal: np.ndarray
    rms: float
    worst: float
    count: int


@dataclass(frozen=True)
class CylinderFit:
    """A cylinder through points on its side: a point on its axis, the
    axis (unit), the radius, the residuals' RMS and largest size, the
    arc of the circumference the points cover in degrees, where along
    the axis from `point` they start and stop, and how many points."""

    point: np.ndarray
    axis: np.ndarray
    radius: float
    rms: float
    worst: float
    arc: float
    span: tuple[float, float]
    count: int


@dataclass
class SensorFit:
    """One accelerometer as its measured surfaces locate it.

    Attributes:
        sensor: The sensor's name, the part of its features' names
            before the last underscore.
        kind: 'triax' or 'uniaxial'.
        frame: (3, 3), rows the accelerometer's X+, Y+, Z+ in the
            points' coordinates, right-handed and orthonormal; None when
            the surfaces do not give one.
        anchor: The point the body is placed from: the corner where
            three faces meet, the middle of the edge two faces share, or
            where a uni-axial accelerometer's axis meets its top face.
        axes: The body axes of the measured faces, signed: 1 for a face
            whose outward normal is X+, -2 for Y-. The body lies behind
            each of them from `anchor`. One entry for a uni-axial
            accelerometer, its top face.
        extent: A tri-axial accelerometer's side is at least this: the
            furthest a measured point lies behind another face. A
            uni-axial accelerometer's diameter, as measured.
        planes: The plane fitted to each flat face, by its label.
        cylinder: The cylinder fitted to a uni-axial accelerometer's
            side.
        moved: The largest angle, in degrees, a measured axis was turned
            to make the frame square.
        flags: What is wrong with the fit.
    """

    sensor: str
    kind: str
    frame: np.ndarray | None = None
    anchor: np.ndarray | None = None
    axes: tuple[int, ...] = ()
    extent: float | None = None
    planes: dict[str, PlaneFit] = field(default_factory=dict)
    cylinder: CylinderFit | None = None
    moved: float = 0.0
    flags: list[Flag] = field(default_factory=list)

    @property
    def placed(self) -> bool:
        """Whether the fit locates the accelerometer: it has a frame and
        no error."""
        return self.frame is not None and not any(f.level == 'error' for f in self.flags)

    @property
    def direction(self) -> str | None:
        """The channel of a uni-axial accelerometer, 'X+', 'Y+' or 'Z+':
        the axis its top face was labelled with, always positive. None
        for a tri-axial one, which has all three."""
        if self.kind != 'uniaxial' or not self.axes:
            return None
        return f'{"XYZ"[abs(self.axes[0]) - 1]}+'

    def center(self, size: float | Sequence[float]) -> np.ndarray:
        """The center of the body, which is where its node goes.

        Parameters
        ----------
        size : float or sequence of 3 floats
            The body's size along its X, Y and Z, in the points' unit;
            one number for a cube. For a uni-axial accelerometer only
            the size along its sensing axis, its height, is used.

        Returns
        -------
        numpy.ndarray
            `anchor` moved half the size back from each measured face.

        Raises
        ------
        ValueError
            When the fit does not locate the accelerometer.
        """
        if self.frame is None or self.anchor is None:
            raise ValueError(f'sensor {self.sensor} is not located by its fit')
        sizes = np.broadcast_to(np.asarray(size, dtype=float), (3,))
        center = np.array(self.anchor, dtype=float)
        for axis in self.axes:
            k = abs(axis) - 1
            center -= 0.5 * sizes[k] * np.sign(axis) * self.frame[k]
        return center


# ---- one surface -------------------------------------------------------------

def fit_plane(points: np.ndarray) -> PlaneFit:
    """The least-squares plane through points.

    Parameters
    ----------
    points : array_like, [N, 3]
        Three points or more.

    Returns
    -------
    PlaneFit
        The plane through their centroid whose normal is the direction
        the points vary least in.

    Raises
    ------
    ValueError
        With fewer than three points.
    """
    points = np.asarray(points, dtype=float).reshape(-1, 3)
    if len(points) < 3:
        raise ValueError('a plane needs three points')
    centroid = points.mean(axis=0)
    normal = np.linalg.svd(points - centroid)[2][2]
    residual = (points - centroid) @ normal
    return PlaneFit(centroid, normal, float(np.sqrt(np.mean(residual ** 2))),
                    float(np.abs(residual).max()), len(points))


def _radial(points: np.ndarray, point: np.ndarray, axis: np.ndarray) -> np.ndarray:
    """Each point's offset from the line through `point` along `axis`."""
    offset = points - point
    return offset - np.outer(offset @ axis, axis)


def fit_cylinder(points: np.ndarray, axis_hint: np.ndarray | None = None) -> CylinderFit:
    """The least-squares cylinder through points on its side.

    The fit is nonlinear, so it is started several ways and the best is
    kept: along each direction the points spread in and along
    `axis_hint`, each with the axis through the points' middle and to
    either side of it, since points on one side of a cylinder have their
    middle a radius off its axis.

    Parameters
    ----------
    points : array_like, [N, 3]
        Five points or more, not all at one height on a line.
    axis_hint : array_like, optional
        A direction the axis is expected near, such as the normal of the
        accelerometer's top face.

    Returns
    -------
    CylinderFit
        The axis, radius and residuals; `point` is on the axis beside
        the points' centroid.

    Raises
    ------
    ValueError
        With fewer than five points.
    """
    points = np.asarray(points, dtype=float).reshape(-1, 3)
    if len(points) < 5:
        raise ValueError('a cylinder needs five points')
    middle = points.mean(axis=0)
    scale = float(np.linalg.norm(points - middle, axis=1).max()) or 1.0
    starts = list(np.linalg.svd(points - middle)[2])
    if axis_hint is not None:
        starts.insert(0, np.asarray(axis_hint, dtype=float) / np.linalg.norm(axis_hint))
    best: tuple[float, np.ndarray, np.ndarray, float] | None = None
    for start in starts:
        e1 = surface_frame(start)[0]
        e2 = np.cross(start, e1)

        def unpack(x: np.ndarray, start: np.ndarray = start, e1: np.ndarray = e1,
                   e2: np.ndarray = e2) -> tuple[np.ndarray, np.ndarray, float]:
            axis = start + x[0] * e1 + x[1] * e2
            return axis / np.linalg.norm(axis), middle + scale * (x[2] * e1 + x[3] * e2), scale * x[4]

        def residual(x: np.ndarray, unpack: object = unpack) -> np.ndarray:
            axis, point, radius = unpack(x)                  # type: ignore[operator]
            return (np.linalg.norm(_radial(points, point, axis), axis=1) - radius) / scale

        guess = float(np.linalg.norm(_radial(points, middle, start), axis=1).mean()) / scale
        for du, dv in ((0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)):
            solved = least_squares(residual, [0.0, 0.0, du * guess, dv * guess, max(guess, 1e-6)])
            if best is None or solved.cost < best[0]:
                axis, point, radius = unpack(solved.x)
                best = (float(solved.cost), axis, point, abs(radius))
    assert best is not None
    _cost, axis, point, radius = best
    point = point + ((middle - point) @ axis) * axis
    radial = _radial(points, point, axis)
    residual_ = np.linalg.norm(radial, axis=1) - radius
    along = (points - point) @ axis
    e1 = surface_frame(axis)[0]
    angles = np.sort(np.arctan2(radial @ np.cross(axis, e1), radial @ e1))
    gap = float(np.diff(np.append(angles, angles[0] + 2 * np.pi)).max())
    return CylinderFit(point, axis, float(radius), float(np.sqrt(np.mean(residual_ ** 2))),
                       float(np.abs(residual_).max()), float(np.degrees(2 * np.pi - gap)),
                       (float(along.min()), float(along.max())), len(points))


# ---- one accelerometer -------------------------------------------------------

def _angle(a: np.ndarray, b: np.ndarray) -> float:
    """The angle between two unit vectors, in degrees."""
    return float(np.degrees(np.arccos(np.clip(a @ b, -1.0, 1.0))))


def _length_flags(what: str, rms: float, limit: float, meters: float | None) -> list[Flag]:
    if meters is None or rms * meters <= limit:
        return []
    return [Flag('warn', f'{what} is rough: its points lie {rms * meters * 1e3:.3f} mm RMS '
                         f'from the fit')]


def fit_triax(sensor: str, faces: Mapping[str, np.ndarray],
              meters: float | None = None) -> SensorFit:
    """A tri-axial accelerometer from the points on its faces.

    The outward normal of a face is the one pointing away from the
    points of the other faces, and a face labelled ``k`` gives the body
    axis ``|k|``, reversed for a negative label. The measured axes are
    turned to the nearest square, right-handed frame. With two faces the
    third axis is the one that makes them right-handed.

    Parameters
    ----------
    sensor : str
        The sensor's name.
    faces : mapping of str to array_like [N, 3]
        The points of each face by its label, one of `FACE_LABELS`.
    meters : float, optional
        How many meters one unit of the points is. Without it the
        roughness of a face is not judged.

    Returns
    -------
    SensorFit
        With a frame when two or three faces of different axes were
        measured; its flags say what is wrong otherwise.
    """
    fit = SensorFit(sensor, 'triax')
    clouds = {label: np.asarray(points, dtype=float).reshape(-1, 3)
              for label, points in faces.items()}
    for label, points in clouds.items():
        if len(points) < 3:
            fit.flags.append(Flag('error', f'face {label} has {len(points)} points; a plane '
                                           'needs three'))
        else:
            fit.planes[label] = fit_plane(points)
            fit.flags += _length_flags(f'face {label}', fit.planes[label].rms, PLANE_RMS, meters)
    labels = list(fit.planes)
    taken = [abs(int(label)) for label in labels]
    if len(set(taken)) < len(taken):
        fit.flags.append(Flag('error', 'two faces are labelled with the same axis'))
        return fit
    if len(labels) < 2:
        fit.flags.append(Flag('error', 'one face gives one axis; a second face is needed to '
                                       'orient the accelerometer'))
        return fit
    # each face's normal turned to point away from the other faces
    outward: dict[str, np.ndarray] = {}
    for label in labels:
        plane = fit.planes[label]
        others = np.vstack([clouds[other] for other in labels if other != label])
        away = plane.point - others.mean(axis=0)
        outward[label] = plane.normal * (1.0 if plane.normal @ away >= 0 else -1.0)
    for i, a in enumerate(labels):
        for b in labels[i + 1:]:
            off = abs(_angle(outward[a], outward[b]) - 90.0)
            if off > FACE_ANGLE:
                fit.flags.append(Flag('warn', f'faces {a} and {b} are {off:.1f} degrees from '
                                              'square'))
    measured = np.zeros((3, 3))
    for label in labels:
        measured[abs(int(label)) - 1] = np.sign(int(label)) * outward[label]
    if len(labels) == 2:
        i, j = sorted(taken)
        k = 3 - i - j + 3                                   # the axis not measured, 1 to 3
        third = np.cross(measured[i - 1], measured[j - 1])
        # X x Y = Z, Y x Z = X, but X x Z = -Y
        measured[k - 1] = third / np.linalg.norm(third) * (-1.0 if (i, j) == (1, 3) else 1.0)
    elif np.linalg.det(measured) < 0:
        fit.flags.append(Flag('error', 'the faces as labelled make a left-handed set of axes; '
                                       'one label is wrong'))
        return fit
    u, _s, vt = np.linalg.svd(measured)
    frame = u @ vt
    fit.frame = frame
    fit.moved = max(_angle(measured[k - 1], frame[k - 1]) for k in taken)
    fit.axes = tuple(int(label) for label in labels)
    normals = np.array([outward[label] for label in labels])
    offsets = np.array([outward[label] @ fit.planes[label].point for label in labels])
    if len(labels) == 3:
        fit.anchor = np.linalg.solve(normals, offsets)
    else:
        # the point of the shared edge nearest the origin, then along the
        # edge to the middle of where the points lie
        edge = frame[k - 1]
        on_edge = np.linalg.lstsq(np.vstack([normals, edge]), np.append(offsets, 0.0),
                                  rcond=None)[0]
        along = (np.vstack([clouds[label] for label in labels]) - on_edge) @ edge
        fit.anchor = on_edge + 0.5 * (along.min() + along.max()) * edge
    # how far the points of each face lie behind the other faces
    fit.extent = float(max(
        (-(clouds[a] - fit.planes[b].point) @ outward[b]).max()
        for a in labels for b in labels if a != b))
    return fit


def fit_uniaxial(sensor: str, side: np.ndarray, top: np.ndarray, label: str = '3',
                 meters: float | None = None) -> SensorFit:
    """A uni-axial accelerometer from the points on its round side and on
    its flat top.

    The direction comes from the top's plane and the position from the
    side's cylinder: the side is often measured over a short height,
    which fixes where the axis is and not well which way it points. The
    top's outward normal points from the side's points toward the top,
    and is the body axis the top is labelled with. The other two axes
    are laid into the top by `instrumentation.surface_frame`.

    Parameters
    ----------
    sensor : str
        The sensor's name.
    side : array_like, [N, 3]
        The points on the round side.
    top : array_like, [N, 3]
        The points on the flat top.
    label : str, default '3'
        The top's label, one of `FACE_LABELS`.
    meters : float, optional
        How many meters one unit of the points is. Without it the
        roughness of the surfaces is not judged.

    Returns
    -------
    SensorFit
        With a frame unless a surface has too few points; `anchor` is
        where the cylinder's axis meets the top, and `extent` the
        diameter.
    """
    fit = SensorFit(sensor, 'uniaxial')
    side = np.asarray(side, dtype=float).reshape(-1, 3)
    top = np.asarray(top, dtype=float).reshape(-1, 3)
    if len(top) < 3:
        fit.flags.append(Flag('error', f'the top has {len(top)} points; a plane needs three'))
    if len(side) < 5:
        fit.flags.append(Flag('error', f'the side has {len(side)} points; a cylinder needs '
                                       'five'))
    if fit.flags:
        return fit
    plane = fit_plane(top)
    normal = plane.normal * (1.0 if plane.normal @ (plane.point - side.mean(axis=0)) >= 0
                             else -1.0)
    cylinder = fit_cylinder(side, axis_hint=normal)
    axis = cylinder.axis * (1.0 if cylinder.axis @ normal >= 0 else -1.0)
    fit.planes[label] = plane
    fit.cylinder = cylinder
    fit.extent = 2.0 * cylinder.radius
    fit.flags += _length_flags('the top', plane.rms, PLANE_RMS, meters)
    fit.flags += _length_flags('the side', cylinder.rms, CYLINDER_RMS, meters)
    lean = _angle(axis, normal)
    fit.moved = lean
    if lean > TOP_ANGLE:
        fit.flags.append(Flag('warn', f'the top is {lean:.1f} degrees from square to the side'))
    if cylinder.arc < ARC:
        fit.flags.append(Flag('warn', f'the side was measured over {cylinder.arc:.0f} degrees '
                                      'of its circumference, which fixes its diameter poorly'))
    off_axis = float(np.linalg.norm(_radial(plane.point[np.newaxis], cylinder.point, axis)))
    if off_axis > cylinder.radius:
        fit.flags.append(Flag('error', 'the top is not on this cylinder: its points are '
                                       f'{off_axis / cylinder.radius:.1f} radii from the axis'))
        return fit
    height = float((plane.point - cylinder.point) @ axis)
    _low, high = (cylinder.span if cylinder.axis @ normal >= 0
                 else (-cylinder.span[1], -cylinder.span[0]))
    if height < high:
        fit.flags.append(Flag('warn', 'the top is not past the end of the measured side, so it '
                                      'may be another surface'))
    # where the axis meets the top
    reach = ((plane.point - cylinder.point) @ normal) / (axis @ normal)
    fit.anchor = cylinder.point + reach * axis
    k = abs(int(label)) - 1
    laid = surface_frame(np.sign(int(label)) * normal)      # rows: two in the top, then the axis
    fit.frame = np.roll(laid, k + 1, axis=0)
    fit.axes = (int(label),)
    return fit


# ---- every accelerometer of a set of features --------------------------------

def parse_feature(name: str) -> tuple[str, str] | None:
    """A feature's sensor and label from its name, ``<sensor>_<label>``.

    Parameters
    ----------
    name : str
        The feature's name, such as ``'31_1'``, ``'3_-1'`` or ``'11_C'``.

    Returns
    -------
    tuple of (str, str) or None
        The sensor and the label, lower-cased, with ``p`` read as ``3``;
        None when the name has no underscore, no sensor, or a label that
        is neither a face's nor a cylinder's.
    """
    sensor, mark, label = str(name).strip().rpartition('_')
    label = label.strip().lower()
    label = '3' if label == 'p' else label
    if not mark or not sensor.strip() or label not in (*FACE_LABELS, CYLINDER_LABEL):
        return None
    return sensor.strip(), label


def fit_sensors(features: Mapping[str, np.ndarray], meters: float | None = None
                ) -> tuple[list[SensorFit], list[str]]:
    """Every accelerometer the features locate.

    Parameters
    ----------
    features : mapping of str to array_like [N, 3]
        The points of each feature by its name, ``<sensor>_<label>``.
    meters : float, optional
        How many meters one unit of the points is. Without it the
        roughness of a surface is not judged.

    Returns
    -------
    fits : list of SensorFit
        One per sensor, in the order the sensors first appear. A sensor
        with a ``c`` feature is uni-axial, any other tri-axial.
    notes : list of str
        The features left out, and why.
    """
    notes: list[str] = []
    sensors: dict[str, dict[str, np.ndarray]] = {}
    for name, points in features.items():
        parsed = parse_feature(name)
        if parsed is None:
            notes.append(f'{name} is not named <sensor>_<label>, with a label of 1, 2, 3, -1, '
                         '-2, -3, c or p, and was left out')
            continue
        sensor, label = parsed
        held = sensors.setdefault(sensor, {})
        if label in held:
            notes.append(f'{name} repeats a label of sensor {sensor} and was left out')
            continue
        held[label] = np.asarray(points, dtype=float).reshape(-1, 3)
    fits = []
    for sensor, held in sensors.items():
        if CYLINDER_LABEL not in held:
            fits.append(fit_triax(sensor, held, meters))
            continue
        tops = [label for label in held if label != CYLINDER_LABEL]
        if len(tops) != 1:
            fit = SensorFit(sensor, 'uniaxial')
            fit.flags.append(Flag('error', 'a cylinder needs one flat top; this sensor has '
                                           f'{len(tops)}'))
            fits.append(fit)
            continue
        fits.append(fit_uniaxial(sensor, held[CYLINDER_LABEL], held[tops[0]], tops[0], meters))
    return fits, notes
