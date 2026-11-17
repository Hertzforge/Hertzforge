"""Scan: points measured on a test article, in space.

A coordinate measuring arm gives points two ways. Probed, a few points
are touched on each surface and the arm's software exports them by
feature, each feature named by the person measuring. Scanned with a
laser, it gives a cloud of points with nothing said about any of them.
A Scan holds either: the points, their length unit, and the features
when the file had them.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence

import numpy as np

from ..units import from_si, si_transform

#: a point that belongs to no feature
UNASSIGNED = -1


class Scan:
    """Measured points, with the features they were measured as.

    Coordinates are as the file gave them until a length unit is
    declared, and in meters from then on, the way a geometry holds its
    nodes.

    Attributes:
        points: float64 [N, 3], one row per point, in file order.
        length_unit: The unit the coordinates were measured in, None
            until declared.
        features: The name of each feature, in file order. Empty for a
            cloud of points.
        feature_of: int32 [N], the row in `features` each point belongs
            to, or `UNASSIGNED`.
        deviation: float64 [N], how far the measuring software found
            each point from the surface it fit to the feature, in the
            points' unit; None when the file gave none.
        properties: What the file's header said (the coordinate system,
            the alignment), by the header's own words.

    Raises ValueError if `points` is not N rows of three, or `feature_of`
    or `deviation` is not one value per point, or a point names a
    feature that is not there.
    """

    def __init__(self, points: np.ndarray, length_unit: str | None = None,
                 features: Sequence[str] | None = None,
                 feature_of: np.ndarray | None = None,
                 deviation: np.ndarray | None = None,
                 properties: Mapping[str, str] | None = None) -> None:
        self.points: np.ndarray = np.array(points, dtype=np.float64).reshape(-1, 3)
        count = len(self.points)
        self.features: list[str] = [str(name) for name in (features or [])]
        self.feature_of: np.ndarray = (
            np.full(count, UNASSIGNED, dtype=np.int32) if feature_of is None
            else np.array(feature_of, dtype=np.int32).reshape(-1))
        if len(self.feature_of) != count:
            raise ValueError('feature_of holds one feature per point')
        if count and (self.feature_of.max() >= len(self.features)
                      or self.feature_of.min() < UNASSIGNED):
            raise ValueError('a point names a feature the scan does not have')
        self.deviation: np.ndarray | None = (
            None if deviation is None else np.array(deviation, dtype=np.float64).reshape(-1))
        if self.deviation is not None and len(self.deviation) != count:
            raise ValueError('deviation holds one value per point')
        self.properties: dict[str, str] = {str(k): str(v) for k, v in (properties or {}).items()}
        self.length_unit: str | None = None
        if length_unit is not None:
            self.define_units(length_unit)

    def __repr__(self) -> str:
        return f'Scan({self.describe()})'

    @property
    def num_points(self) -> int:
        """How many points there are."""
        return len(self.points)

    @property
    def num_features(self) -> int:
        """How many features the points were measured as."""
        return len(self.features)

    @property
    def units_defined(self) -> bool:
        """Whether the length unit of the coordinates has been declared."""
        return self.length_unit is not None

    def describe(self) -> str:
        """A few words on what the scan holds: its points and features."""
        points = f'{self.num_points:,} point{"s" * (self.num_points != 1)}'
        if not self.features:
            return points
        return f'{points} in {self.num_features} feature{"s" * (self.num_features != 1)}'

    def feature_points(self, feature: str | int) -> np.ndarray:
        """The points of one feature, by its name or its row, in file order.

        Raises ValueError if the scan has no feature of that name.
        """
        if isinstance(feature, str):
            if feature not in self.features:
                raise ValueError(f'the scan has no feature named {feature!r}')
            feature = self.features.index(feature)
        return self.points[self.feature_of == int(feature)]

    def define_units(self, length_unit: str) -> Scan:
        """Declare the length unit of the coordinates, converting them to
        meters. Declaring again reads the original values in the new
        unit, so a wrong unit can be corrected. Returns self."""
        scale, _ = si_transform(length_unit, 'length')
        back = (1.0 if self.length_unit is None
                else 1.0 / si_transform(self.length_unit, 'length')[0])
        self.points = self.points * back * scale
        if self.deviation is not None:
            self.deviation = self.deviation * back * scale
        self.length_unit = length_unit
        return self

    def undefine_units(self) -> Scan:
        """Remove the length unit and restore the file's coordinates;
        returns self."""
        if self.length_unit is not None:
            self.points = from_si(self.points, self.length_unit, 'length')
            if self.deviation is not None:
                self.deviation = from_si(self.deviation, self.length_unit, 'length')
            self.length_unit = None
        return self
