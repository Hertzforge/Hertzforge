"""The datasets the service offers, how each is opened, and the cache that
makes a second open fast.

Two kinds (`schema/PROTOCOL.md`, "Operations"):

- `random_run`: a random vibration controller run read by
  `hertzforge.random_vibration_run`, with its PSDs and its
  specification.
- `modal_model`: a finite element model from `hertzforge.demo`, its
  normal modes solved by `fem.Model.eigensolution`.

Every number comes from the core. What this module adds is glue:
where the files are, the cache, the channel list the shell shows, and the
mesh in the form the viz kit draws (`element_cells`, triangulated).
"""

from __future__ import annotations

import os
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np

from .protocol import not_found, refused

#: the repository root (src/hertzforge/service/datasets.py -> the root),
#: where `testdata/` and the cache are in a checkout
ROOT = Path(__file__).resolve().parents[3]
FETCH_COMMAND = 'scripts/fetch_testdata.sh'

#: how many of the lowest modes a model is solved for, rigid ones included
MODES_SOLVED = 30


def testdata_root() -> Path:
    """Where the test data is: `HERTZFORGE_TESTDATA`, else the checkout's
    `testdata/`, where `scripts/fetch_testdata.sh` puts `plate/`."""
    given = os.environ.get('HERTZFORGE_TESTDATA')
    if given:
        return Path(given).expanduser()
    return ROOT / 'testdata'


def cache_root() -> Path:
    """Where solved and imported results are kept between runs:
    `HERTZFORGE_CACHE_DIR`, else `.data/cache` in the checkout."""
    given = os.environ.get('HERTZFORGE_CACHE_DIR')
    if given:
        return Path(given).expanduser()
    return ROOT / '.data' / 'cache'


def core_version() -> str:
    import hertzforge
    return hertzforge.__version__


def _cached(name: str, build: Callable[[], Any],
            notes: list[str]) -> tuple[Any, bool]:
    """(object, came from cache): `build()`'s result, saved with
    its own `save` under `name` and read back with
    `hertzforge.load` next time. A cache that cannot be read is rebuilt;
    one that cannot be written is noted and skipped, never an error."""
    import hertzforge as hz

    path = cache_root() / name
    if path.exists():
        try:
            return hz.load(path), True
        # any failure to read a cache is a reason to rebuild it, and the
        # reason is reported in the notes rather than swallowed
        except Exception as error:  # noqa: BLE001
            notes.append(f'cache {path.name} could not be read ({error}); '
                         'rebuilt')
    built = build()
    partial = path.with_name(f'.{path.stem}.{os.getpid()}.partial.vdyn')
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        built.save(partial)
        os.replace(partial, path)
    except OSError as error:
        notes.append(f'cache not written to {path.parent} ({error}); the next '
                     'open solves again')
    finally:
        partial.unlink(missing_ok=True)
    return built, False


@dataclass(frozen=True)
class Channel:
    """One record of a run's PSDs as the shell lists it."""

    value: str
    label: str
    record: int
    #: the specification record it is judged against, None when it is not
    #: a control channel
    spec_record: int | None

    def to_json(self) -> dict[str, str]:
        return {'value': self.value, 'label': self.label}


@dataclass
class RandomRun:
    kind = 'random_run'
    title: str
    source: Path
    psds: Any
    specification: Any
    channels: list[Channel]
    scale_db: float
    #: `compliance.compare_all`, every control channel, at `scale_db`
    rows: list[tuple[str, dict[str, Any]]]
    #: `compliance.verdict(rows)`
    verdict: dict[str, Any]
    notes: list[str] = field(default_factory=list)
    from_cache: bool = False
    seconds: float = 0.0

    def channel(self, value: str) -> Channel:
        for channel in self.channels:
            if channel.value == value:
                return channel
        raise not_found(f'no channel {value!r} in {self.title}; dataset.open '
                        'lists its channels')

    def summary(self) -> dict[str, Any]:
        psds, spec = self.psds, self.specification
        spacing = np.diff(psds.abscissa)
        out: dict[str, Any] = {
            'source': self.source.name,
            'records': int(psds.num_records),
            'control_channels': sum(c.spec_record is not None
                                    for c in self.channels),
            'frequency_lines': int(psds.abscissa.size),
            'line_spacing_hz': float(np.median(spacing)) if spacing.size
            else None,
            'specification': spec is not None,
            'specification_limits': ', '.join(spec.limits) if spec is not None
            and spec.has_limits else 'none',
            'scale_db': self.scale_db,
            'from_cache': self.from_cache,
            'open_seconds': round(self.seconds, 3),
        }
        if self.verdict.get('passed') is not None:
            out['run_passed'] = bool(self.verdict['passed'])
        if self.notes:
            out['notes'] = '; '.join(self.notes)
        return out


@dataclass
class ModalModel:
    kind = 'modal_model'
    title: str
    geometry: Any
    shapes: Any
    #: geometry node rows drawn, in the order the view's nodes are sent
    node_rows: np.ndarray
    #: f32 [N, 3] in metres, the drawn nodes only
    nodes: np.ndarray
    triangles: np.ndarray
    lines: np.ndarray
    unit: str
    #: `deform.dof_directions` of the shapes' DOFs, read once: (geometry
    #: node row, global unit vector) per translational DOF kept, and the
    #: mask of the DOFs kept
    dof_rows: np.ndarray
    dof_vectors: np.ndarray
    dof_used: np.ndarray
    notes: list[str] = field(default_factory=list)
    from_cache: bool = False
    seconds: float = 0.0
    channels: list[Channel] = field(default_factory=list)

    def translations(self, mode: int) -> np.ndarray:
        """[drawn nodes, 3]: mode `mode`'s translation of each drawn node,
        in global coordinates. The sum `deform.node_displacements` forms
        (each DOF's value along its direction, DOFs on one node added),
        over the directions read once at open; `tests/test_ops.py` holds
        the two equal."""
        values = np.asarray(self.shapes.shape_matrix[mode])[self.dof_used]
        count = self.geometry.num_nodes
        moved = np.stack([np.bincount(self.dof_rows,
                                      self.dof_vectors[:, axis] * values,
                                      minlength=count) for axis in range(3)],
                         axis=1)
        return moved[self.node_rows]

    def summary(self) -> dict[str, Any]:
        frequency = np.asarray(self.shapes.frequency, dtype=float)
        elastic = frequency[frequency > 0.0]
        out: dict[str, Any] = {
            'nodes': int(self.geometry.num_nodes),
            'nodes_drawn': int(self.node_rows.size),
            'elements': len(self.geometry.elem_conn),
            'triangles': len(self.triangles),
            'lines': len(self.lines),
            'dofs': int(self.shapes.num_dofs),
            'modes_solved': int(self.shapes.num_shapes),
            'rigid_modes': int(np.sum(frequency == 0.0)),
            'first_elastic_hz': float(elastic[0]) if elastic.size else None,
            'highest_hz': float(frequency.max()) if frequency.size else None,
            'normalization': 'mass',
            'from_cache': self.from_cache,
            'open_seconds': round(self.seconds, 3),
        }
        if self.notes:
            out['notes'] = '; '.join(self.notes)
        return out


@dataclass(frozen=True)
class Entry:
    """A dataset the catalog names."""

    id: str
    kind: str
    title: str
    description: str
    open: Callable[[], Any] = field(repr=False, compare=False)

    def to_json(self) -> dict[str, str]:
        return {'id': self.id, 'kind': self.kind, 'title': self.title,
                'description': self.description}


# ---- random runs -------------------------------------------------------------

def quantity_of(dimension: str) -> str:
    """'acceleration**2/frequency' -> 'acceleration': what a PSD record
    measures, for its label."""
    head = dimension.split('/')[0]
    return head.split('**')[0].split('*')[0] or dimension


def _channels(psds: Any, pairs: list[tuple[str, int, int]]) -> list[Channel]:
    judged = {measured: spec for _label, spec, measured in pairs}
    out, seen = [], set()
    for i in range(psds.num_records):
        quantity = quantity_of(str(psds.ordinate_dim[i]))
        value = f'{psds.record_label(i)}:{quantity}'
        if value in seen:                  # same DOF and quantity twice
            value = f'{value}#{i}'
        seen.add(value)
        label = f'{psds.record_label(i)} {quantity}'
        if i in judged:
            label += ' (control)'
        out.append(Channel(value, label, i, judged.get(i)))
    # control channels first, the order a control room reads them in
    return sorted(out, key=lambda c: (c.spec_record is None,
                                      c.spec_record or 0, c.record))


def open_random_run(path: Path, title: str, cache_name: str) -> RandomRun:
    import hertzforge as hz
    from hertzforge.core import compliance

    started = time.perf_counter()
    if not path.is_file():
        raise not_found(
            f'The test data for "{title}" is missing: {path} does not exist. '
            f'Run {FETCH_COMMAND} from the repository root, or set '
            'HERTZFORGE_TESTDATA to a directory that holds plate/random.nc4.')
    notes: list[str] = []
    stat = path.stat()
    name = (f'{cache_name}-hz{core_version()}-{stat.st_size}-'
            f'{stat.st_mtime_ns}.vdyn')
    project, hit = _cached(name, lambda: hz.random_vibration_run(path), notes)
    psds, _octave = project.psds
    spec = project.specification
    if spec is None:
        pairs, scale, rows = [], 0.0, []
        notes.append('the run carries no specification, so nothing can be '
                     'judged against one')
    else:
        pairs = compliance.matched_records(spec, psds)
        # the one scale the whole set is compared at, as compare_all
        # resolves it; said here because a detected scale shifts every number
        scale = float(compliance.comparison_scale_db(spec, psds))
        if scale and getattr(psds, 'scale_db', None) is None:
            notes.append(f'measurement scaled by {scale:+g} dB, detected from '
                         'the data')
        rows = compliance.compare_all(spec, psds, scale_db=scale)
    verdict = compliance.verdict(rows)
    return RandomRun(title=title, source=path, psds=psds, specification=spec,
                     channels=_channels(psds, pairs), scale_db=scale,
                     rows=rows, verdict=verdict, notes=notes, from_cache=hit,
                     seconds=time.perf_counter() - started)


# ---- modal models ------------------------------------------------------------

def _triangulate(faces: list[list[int]]) -> np.ndarray:
    """Polygons (3 or 4 corners, as `element_cells` draws faces and solid
    skins) as triangles, fanned from each polygon's first corner."""
    out = []
    by_count: dict[int, list[list[int]]] = {}
    for face in faces:
        by_count.setdefault(len(face), []).append(face)
    for count, group in by_count.items():
        if count < 3:
            continue
        polygons = np.asarray(group, dtype=np.int64)
        for k in range(1, count - 1):
            out.append(polygons[:, [0, k, k + 1]])
    return (np.concatenate(out) if out
            else np.zeros((0, 3), dtype=np.int64))


def _mesh(geometry: Any) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
    """(node rows drawn, triangles, lines), the last two indexing the drawn
    nodes. Faces and lines are VD's own drawing of the elements
    (`viz.geometry.element_cells`: a face element by its corners, a solid by
    its skin, a line by its two ends). Only nodes a triangle or a line uses
    are sent, so a brick model's interior nodes do not travel; a geometry
    with no faces or lines sends every node."""
    from hertzforge.drawing.viz.geometry import element_cells

    faces, lines, _points = element_cells(geometry)
    triangles = _triangulate([face for group in faces.values()
                              for face in group])
    segments = np.asarray([pair for group in lines.values() for pair in group],
                          dtype=np.int64).reshape(-1, 2)
    used = np.unique(np.concatenate([triangles.ravel(), segments.ravel()]))
    if not used.size:
        used = np.arange(geometry.num_nodes)
    remap = np.full(geometry.num_nodes, -1, dtype=np.int64)
    remap[used] = np.arange(used.size)
    return used, remap[triangles], remap[segments]


def open_fem_model(cache_name: str, title: str,
                   build: Callable[[], Any]) -> ModalModel:
    """A `hertzforge.demo` model and its lowest `MODES_SOLVED` modes.

    No damping is stated: a finite element model has none of its own, so
    every mode's damping is sent as null."""
    started = time.perf_counter()
    notes: list[str] = []
    model: list[Any] = []

    def fem_model() -> Any:
        if not model:
            model.append(build())
        return model[0]

    stem = f'{cache_name}-hz{core_version()}-{MODES_SOLVED}modes'
    geometry, hit_g = _cached(f'{stem}-geometry.vdyn',
                              lambda: fem_model().geometry(), notes)
    shapes, hit_s = _cached(
        f'{stem}-shapes.vdyn',
        lambda: fem_model().eigensolution(num_modes=MODES_SOLVED), notes)
    from hertzforge.deform import dof_directions

    if shapes.is_complex:
        raise refused(f'{title}: its shapes are complex, and the viz kit draws '
                      'real translations only.')
    rows, triangles, lines = _mesh(geometry)
    dof_rows, dof_vectors, dof_used = dof_directions(geometry,
                                                     shapes.coordinate)
    xyz = np.asarray(geometry.node_xyz, dtype=float)[rows]
    unit = 'm' if geometry.units_defined else 'undefined'
    if not geometry.units_defined:
        notes.append('geometry units undefined; coordinates as the file '
                     'wrote them')
    return ModalModel(title=title, geometry=geometry, shapes=shapes,
                      node_rows=rows, nodes=xyz.astype(np.float32),
                      triangles=triangles, lines=lines, unit=unit,
                      dof_rows=dof_rows, dof_vectors=dof_vectors,
                      dof_used=dof_used, notes=notes, from_cache=hit_g and hit_s,
                      seconds=time.perf_counter() - started)


def _plate_random() -> RandomRun:
    return open_random_run(testdata_root() / 'plate' / 'random.nc4',
                           'Plate Random Run', 'plate-random')


def _drone() -> ModalModel:
    from hertzforge.demo import drone
    return open_fem_model('drone', 'Quadcopter FE Model', drone.build)


def _plate_fem() -> ModalModel:
    from hertzforge.demo import plate
    return open_fem_model('plate-fem', 'Free Plate FE Model', plate.build)


def _frame_wing() -> ModalModel:
    from hertzforge.demo import frame_wing
    return open_fem_model('frame-wing', 'Frame With Thick Wing FE Model',
                          lambda: frame_wing.build('thick wing'))


CATALOG: tuple[Entry, ...] = (
    Entry('plate-random', 'random_run', 'Plate Random Run',
          'A random vibration controller run on a free plate: eight control '
          'channels against a specification with warning and abort limits, '
          'plus four force channels.', _plate_random),
    Entry('drone-fem', 'modal_model', 'Quadcopter FE Model',
          'A quadcopter of shells and beams, 1,422 nodes, its 30 lowest '
          'normal modes (six rigid). Solves in about a second.', _drone),
    Entry('plate-fem', 'modal_model', 'Free Plate FE Model',
          'A free square plate of 144 shell elements, its 30 lowest normal '
          'modes (six rigid).', _plate_fem),
    Entry('frame-wing-fem', 'modal_model', 'Frame With Thick Wing FE Model',
          'A frame of solid bricks with a thick wing screwed across two '
          'uprights, 65,585 nodes: the dense case for frame rate. About 30 s '
          'to solve the first time, then read from the cache.', _frame_wing),
)


def entry(dataset_id: Any) -> Entry:
    for item in CATALOG:
        if item.id == dataset_id:
            return item
    raise not_found(f'No dataset named "{dataset_id}"; catalog.list names the '
                    'ones that exist.')
