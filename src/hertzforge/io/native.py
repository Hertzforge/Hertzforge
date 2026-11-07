"""The native .hforge file format, which is HDF5, and the reader for
Visual Dynamics' .vdyn files it started from.

A file with a single object has one group at the root, named for the
object's kind ('geometry', 'data', 'shapes', and so on). A file with a
whole test has an `objects/` group of numbered groups, numbered in the
test's order, with each object's `name` and `kind` as attributes, so a
name may contain anything, including characters h5py would read as a
path. The test's name, active geometry, project type, object groups and
provenance are attributes of the root. Ragged connectivity is stored as
a flat array plus offsets. Every file is stamped with its schema
version, and a file from a newer schema is refused with a message to
update. `docs/hforge-format.md` writes the layout down.

A `.hforge` file is a `.vdyn` file of schema 3 under a new stamp, with
the element-group fields and the object groups under their current
names (`VDYN_NAMES`). A `.vdyn` file is read, by its own stamp, and
never written: saving it again writes `.hforge`.

A project can hold an object by reference to another `.hforge` file
instead of a copy (schema 2): the object's group names the file and the
object, with the file's size and SHA-256, and the reader reads the
object from there (`Reference`).
"""

from __future__ import annotations

import contextlib
import contextvars
import hashlib
import mmap
import os
import uuid
import warnings
from collections.abc import Callable, Iterator, Mapping, Sequence
from typing import TYPE_CHECKING, Any

import numpy as np

from ..project import Project

if TYPE_CHECKING:                                    # pragma: no cover
    import h5py

    from ..core.channel_table import ChannelTable
    from ..core.data import DataArray
    from ..core.data_index import Reference
    from ..core.geometry import Geometry
    from ..core.matches import MatchedModes
    from ..core.photos import Photos
    from ..core.prediction import Predictions
    from ..core.report import Report
    from ..core.run_log_extract import RunLogExtract
    from ..core.shapes import ShapeSet
    from ..core.sine import SineLevelSet, SineSweepSpecification

#: the root attribute every file written here is stamped with
STAMP = 'hertzforge_schema'

#: the newest .hforge schema this build reads. Schema 1 holds everything
#: `.vdyn` schema 3 held; schema 2 adds objects held by reference to
#: another file. The schema grows by addition, and a file is stamped
#: with the earliest version that holds everything in it, so an older
#: build opens every file that does not need a later one.
SCHEMA_VERSION = 2

#: the stamp of a file with no object held by reference
BASE_SCHEMA = 1

#: the stamp of a file with one or more objects held by reference
REFERENCE_SCHEMA = 2

#: the smallest object, in bytes stored, that a project imported from
#: another `.hforge` holds by reference rather than copying: the size a
#: run is mapped from rather than read (`MAP_FROM_BYTES`), where a
#: second copy starts to cost
REFERENCE_FROM_BYTES = 64 * 2**20

#: how many references deep the reader follows a chain (a project
#: referring to a project that refers to the run) before calling it
#: broken; a loop would otherwise never end
REFERENCE_DEPTH = 8

#: the stamp of a Visual Dynamics file
VDYN_STAMP = 'visualdynamics_schema'

#: the newest .vdyn schema there is. Visual Dynamics stopped at 3 (2
#: added predictions, 3 the run log extract) when its core moved here.
VDYN_SCHEMA = 3

#: the suffix `save` adds
SUFFIX = '.hforge'

#: the smallest array a compressed save compresses; below it gzip saves
#: too little to pay for the chunk index
COMPRESS_FROM_BYTES = 64 * 1024

#: the bytes in one chunk of a compressed array, the size HDF5's own
#: guidance puts between 10 KB and 1 MB
CHUNK_BYTES = 1024 * 1024

#: whether the save in progress compresses its large arrays (`compressed`)
_compressing: contextvars.ContextVar[bool] = contextvars.ContextVar(
    'compressing', default=False)

#: whether the save in progress is a fingerprint (`fingerprint`), which
#: names a mapped array by where it is mapped instead of copying it
_fingerprinting: contextvars.ContextVar[bool] = contextvars.ContextVar(
    'fingerprinting', default=False)


@contextlib.contextmanager
def compressed(on: bool = True) -> Iterator[None]:
    """Within the block, saves write each array of `COMPRESS_FROM_BYTES`
    or more with gzip and the shuffle filter, chunked along its last axis
    (time, for a time history). Arrays are uncompressed otherwise: gzip
    saved 12% on a random-vibration run and wrote at 58 MB/s, so it is
    for archiving, not for every save (FILE_FORMAT_PLAN.md, section 3).
    Every HDF5 reader reads the file either way."""
    token = _compressing.set(on)
    try:
        yield
    finally:
        _compressing.reset(token)


#: the smallest array the reader maps from the file instead of reading
#: (`_read_array`); below it the bytes are read, which costs less than a
#: mapping kept open
MAP_FROM_BYTES = 64 * 2**20


def _read_array(dataset: h5py.Dataset) -> np.ndarray:
    """A numeric dataset as an array, mapped from the file when it is
    large, or read.

    A mapped array is an ordinary numpy array whose bytes stay in the
    file until something touches them, so opening a 23 GB run reads none
    of its samples, and a span read pages in only that span. It is mapped
    copy-on-write: an edit in memory never reaches the file. Mapping
    needs the bytes laid out plainly in a file on disk, so an array that
    is compressed or chunked (a compressed save), one in a file held in
    memory (the `.mat` and ESCDF readers), and every array on Windows,
    where a mapped file cannot be replaced by the next save, are read
    as before. `save` and `save_test` replace a file whole
    (`_replaced`), so saving over a file an open project maps leaves
    the mapping on the bytes it had."""
    if (os.name != 'nt' and dataset.nbytes >= MAP_FROM_BYTES
            and dataset.chunks is None and dataset.compression is None
            and dataset.dtype.kind in 'biufc'
            and dataset.file.driver == 'sec2'):
        offset = dataset.id.get_offset()
        if offset is not None:
            return np.memmap(dataset.file.filename, dtype=dataset.dtype,
                             mode='c', offset=offset, shape=dataset.shape)
    return dataset[()]


@contextlib.contextmanager
def _replaced(path: str) -> Iterator[str]:
    """A path beside `path` to write the file to; on success it replaces
    `path` in one rename, and on failure it is removed and `path` is left
    as it was. A crash mid-save therefore never costs the last saved
    file, and an open project whose arrays are mapped from `path` keeps
    reading the bytes it mapped, which the rename does not touch."""
    folder, name = os.path.split(os.path.abspath(path))
    partial = os.path.join(folder, f'.{name}.{os.getpid()}.partial')
    try:
        yield partial
        os.replace(partial, path)
    finally:
        if os.path.exists(partial):
            os.remove(partial)


def _write_array(group: h5py.Group, name: str, data: Any) -> None:
    """A numeric dataset, compressed when the save asks and the array is
    large enough (`compressed`)."""
    array = np.asarray(data)
    if _fingerprinting.get() and (mapped := _mapping_of(array)) is not None:
        group.attrs[f'{name}@mapped'] = (
            f'{id(mapped)} {array.__array_interface__["data"][0]} '
            f'{array.shape} {array.strides} {array.dtype.str}')
        return
    if (not _compressing.get() or array.nbytes < COMPRESS_FROM_BYTES
            or array.dtype.kind not in 'biufc'):
        group.create_dataset(name, data=array)
        return
    run = max(1, CHUNK_BYTES // array.itemsize)
    chunks = (*(1,) * (array.ndim - 1), min(array.shape[-1], run))
    group.create_dataset(name, data=array, chunks=chunks, compression='gzip',
                         compression_opts=1, shuffle=True)


def _mapping_of(array: np.ndarray) -> mmap.mmap | None:
    """The file mapping `array`'s bytes are in, or None for an array
    in memory."""
    base = array
    while base is not None:
        if isinstance(base, mmap.mmap):
            return base
        base = getattr(base, 'base', None)
    return None


def _write_ragged(group: h5py.Group, name: str,
                  arrays: Sequence[np.ndarray]) -> None:
    flat = (np.concatenate(arrays) if arrays
            else np.empty(0, dtype=np.int64))
    offsets = np.cumsum([0] + [len(a) for a in arrays])
    _write_array(group, f'{name}_flat', flat)
    _write_array(group, f'{name}_offsets', offsets)


def _read_ragged(group: h5py.Group, name: str) -> list[np.ndarray]:
    flat = group[f'{name}_flat'][()]
    offsets = group[f'{name}_offsets'][()]
    return [flat[offsets[i]:offsets[i + 1]] for i in range(len(offsets) - 1)]


def _write_strings(group: h5py.Group, name: str,
                   strings: Sequence[str]) -> None:
    import h5py
    group.create_dataset(name, data=strings, dtype=h5py.string_dtype('utf-8'))


def _read_strings(group: h5py.Group, name: str) -> list[str]:
    return [s.decode('utf-8') for s in group[name][()]]


#: the names `.vdyn` keeps for a geometry's element-group fields, by
#: the name `.hforge` and the code use. Visual Dynamics called element
#: groups blocks when its format was set and kept the old names on disk
#: after the rename (2026-10-08), so its files load the same.
VDYN_NAMES = {'elem_group': 'elem_block', 'group_id': 'block_id',
              'group_name': 'block_name',
              'group_properties': 'block_properties'}


def _on_disk(group: h5py.Group, name: str) -> str:
    """The name `name` is stored under in the file `group` is in: its
    `.vdyn` name in a Visual Dynamics file, itself otherwise."""
    if VDYN_STAMP in group.file.attrs:
        return VDYN_NAMES.get(name, name)
    return name


def save_geometry(geom: Geometry, group: h5py.Group) -> None:
    """Write a geometry into `group`: nodes, coordinate systems and
    elements as datasets, connectivity as a flat array plus offsets.

    The rigid-body settings, view, solids, mates and element-group
    properties are written only when set, so a file from before each
    existed reads back the same.
    """
    group.attrs['dimension'] = geom.dimension
    group.attrs['length_unit'] = geom.length_unit or ''
    # a deck's geometry, whose properties convert with its units, and the
    # mass unit declared for it; absent for any other, so a file from
    # before them reads back the same (2026-10-26)
    if getattr(geom, 'from_deck', False):
        group.attrs['from_deck'] = True
        group.attrs['mass_unit'] = geom.mass_unit or ''
    for name in ('node_id', 'node_xyz', 'node_def_cs', 'node_disp_cs', 'node_color',
                 'cs_id', 'cs_type', 'cs_matrix',
                 'elem_id', 'elem_type', 'elem_color', 'elem_group',
                 'group_id'):
        _write_array(group, name, getattr(geom, name))
    _write_strings(group, 'cs_name', geom.cs_name)
    _write_strings(group, 'group_name', geom.group_name)
    _write_ragged(group, 'elem_conn', geom.elem_conn)
    # the rigid-body settings, when set — the point always, the mass
    # and inertia only when the set is to be mass-normalized, so an
    # unscaled setting and a file from before the setting existed read
    # back the same way (absent, not sentinel-valued)
    properties = getattr(geom, 'mass_properties', None)
    if properties is not None:
        group.attrs['mass_point'] = np.asarray(properties.point)
        if properties.scaled:
            group.attrs['mass'] = properties.mass
            group.attrs['inertia'] = np.asarray(properties.inertia)
    # the view it opens on, when one was set — absent otherwise, so a
    # file from before views existed reads back as the default
    view = getattr(geom, 'view', None)
    if view is not None:
        group.attrs['view_eye'] = np.asarray(view.eye, dtype=np.float64)
        group.attrs['view_up'] = np.asarray(view.up, dtype=np.float64)
    # the solids, as the numbers typed (2026-10-11): a recipe, so the file
    # reopens editable without the geometry kernel; absent when there
    # are none, so a file from before solids reads back the same
    solids = getattr(geom, 'solids', None) or []
    if solids:
        import json

        group.attrs['solids'] = json.dumps([s.as_record() for s in solids])
    # the mated surfaces, as definitions (`core.mates`); absent when there
    # are none, so a file from before them reads back the same
    mates = getattr(geom, 'mates', None) or []
    if mates:
        import json

        group.attrs['mates'] = json.dumps([m.as_record() for m in mates])
    # what each element group is made of, one subgroup per element group that has it:
    # the material's numbers, and a thickness or a section — the model
    # a geometry can build of itself (2026-09-25) — or, for an element group of
    # point masses, the mass alone (2026-10-07)
    groups = getattr(geom, 'group_properties', None) or {}
    if groups:
        holder = group.create_group('group_properties')
        for group_id, props in groups.items():
            entry = holder.create_group(str(int(group_id)))
            if props.mass is not None:
                entry.attrs['mass'] = float(props.mass)
            # a spring's six stiffnesses, NaN where a direction is free;
            # a ground group, the directions it holds, 'X Y Z RX RY RZ'
            # (a39 wrote True for all six, and still reads)
            if props.stiffness is not None:
                entry.attrs['stiffness'] = np.array(
                    [np.nan if k is None else float(k)
                     for k in props.stiffness], dtype=np.float64)
            if props.ground:
                entry.attrs['ground'] = ' '.join(props.ground)
            if props.material is not None:
                entry.attrs['material_name'] = props.material.name
                entry.attrs['youngs_modulus'] = float(
                    props.material.youngs_modulus)
                entry.attrs['density'] = float(props.material.density)
                entry.attrs['poissons_ratio'] = float(
                    props.material.poissons_ratio)
                if props.material.modulus_of_rigidity is not None:
                    entry.attrs['modulus_of_rigidity'] = float(
                        props.material.modulus_of_rigidity)
            if props.thickness is not None:
                entry.attrs['thickness'] = float(props.thickness)
            if props.section is not None:
                entry.attrs['section_name'] = props.section.name
                entry.attrs['section'] = np.array(
                    [props.section.area, props.section.iy,
                     props.section.iz, props.section.j], dtype=np.float64)
                # a section built from a shape keeps it, so the table
                # shows its dimensions again (the four numbers stay, for
                # a reader that knows nothing of shapes)
                if props.section.shape:
                    entry.attrs['section_shape'] = props.section.shape
                    entry.attrs['section_dimensions'] = np.asarray(
                        props.section.dimensions, dtype=np.float64)
            if props.orientation is not None:
                entry.attrs['orientation'] = np.asarray(props.orientation,
                                                       dtype=np.float64)


def _load_group_properties(group) -> dict:
    held = _on_disk(group, 'group_properties')
    if held not in group:
        return {}
    from ..core.fem import GroupProperties, Material, Section

    out = {}
    for key, entry in group[held].items():
        attrs = entry.attrs
        material = None
        if 'youngs_modulus' in attrs:
            material = Material(
                str(attrs.get('material_name', '')),
                float(attrs['youngs_modulus']), float(attrs['density']),
                float(attrs['poissons_ratio']),
                (float(attrs['modulus_of_rigidity'])
                 if 'modulus_of_rigidity' in attrs else None))
        section = None
        if 'section' in attrs:
            area, iy, iz, j = (float(v) for v in attrs['section'])
            section = Section(str(attrs.get('section_name', '')),
                              area, iy, iz, j,
                              str(attrs.get('section_shape', '')),
                              tuple(float(v) for v in
                                    attrs.get('section_dimensions', ())))
        out[int(key)] = GroupProperties(
            material,
            thickness=(float(attrs['thickness']) if 'thickness' in attrs
                       else None),
            section=section,
            orientation=(tuple(float(v) for v in attrs['orientation'])
                         if 'orientation' in attrs else None),
            mass=float(attrs['mass']) if 'mass' in attrs else None,
            stiffness=(tuple(None if np.isnan(k) else float(k)
                             for k in attrs['stiffness'])
                       if 'stiffness' in attrs else None),
            ground=_ground(attrs.get('ground')))
    return out


def _ground(value):
    """A ground group's directions as stored: a name list, or a flag
    from a39 meaning all six."""
    if isinstance(value, bytes):
        value = value.decode('utf-8')
    if isinstance(value, str):
        return value
    return bool(value)


def _load_mass_properties(group):
    if 'mass_point' not in group.attrs:
        return None
    from ..core.rigid import MassProperties

    return MassProperties(
        tuple(group.attrs['mass_point']),
        mass=float(group.attrs['mass']) if 'mass' in group.attrs else None,
        inertia=(tuple(group.attrs['inertia']) if 'inertia' in group.attrs
                 else None))


def load_geometry(group: h5py.Group) -> Geometry:
    """Read a geometry written by `save_geometry`.

    A file from before 2026-09-30, which kept tracelines apart from
    elements, is converted on the way in: each traceline becomes an
    element group of two-node lines, and element groups are split by
    family.
    """
    from ..core.geometry import Geometry

    data = {name: group[_on_disk(group, name)][()] for name in (
        'node_id', 'node_xyz', 'node_def_cs', 'node_disp_cs', 'node_color',
        'cs_id', 'cs_type', 'cs_matrix',
        'elem_id', 'elem_type', 'elem_color', 'elem_group', 'group_id')}
    data['group_name'] = _read_strings(group, _on_disk(group, 'group_name'))
    data['cs_name'] = _read_strings(group, 'cs_name')
    data['elem_conn'] = _read_ragged(group, 'elem_conn')
    data['length_unit'] = group.attrs.get('length_unit', '') or None
    data['group_properties'] = _load_group_properties(group)
    data['from_deck'] = bool(group.attrs.get('from_deck', False))
    data['mass_unit'] = group.attrs.get('mass_unit', '') or None
    geometry = Geometry(**data)
    if 'traceline_conn' in group:
        # a file from before 2026-09-30 carried tracelines apart from
        # elements, and could hold an element group of more than one family: a
        # traceline becomes an element group of two-node line elements with no
        # properties, one element group per id, and the element groups are split by
        # family — the geometry it reads as now, saved back that way
        geometry.split_groups_by_family()
        by_id: dict[int, tuple[str, int, list]] = {}
        for tl_id, color, desc, conn in zip(
                group['traceline_id'][()], group['traceline_color'][()],
                _read_strings(group, 'traceline_desc'),
                _read_ragged(group, 'traceline_conn')):
            _n, _c, runs = by_id.setdefault(int(tl_id), (desc, int(color), []))
            runs.append([int(n) for n in conn])
        geometry.attach_drawn_lines(list(by_id.values()))
    geometry.mass_properties = _load_mass_properties(group)
    if 'view_eye' in group.attrs:
        from ..core.geometry import View

        geometry.view = View(tuple(group.attrs['view_eye']),
                             tuple(group.attrs['view_up']))
    if 'solids' in group.attrs:
        import json

        from ..core.solids import Solid

        geometry.solids = [Solid.from_record(r)
                           for r in json.loads(group.attrs['solids'])]
    if 'mates' in group.attrs:
        import json

        from ..core.mates import MatedSet

        geometry.mates = [MatedSet.from_record(r)
                          for r in json.loads(group.attrs['mates'])]
    return geometry


def save_data(data: DataArray, group: h5py.Group) -> None:
    """Write a data object into `group`: abscissa, ordinate and the
    per-record strings as datasets, and what its kind carries (an SRS's
    Q, a sine level's tone, interpolation, band constraints, averaging,
    filtering, run identity and so on) beside them, each only when set.

    The class name is stored as well as the function type, because a
    `Specification` shares the PSD's and the code alone cannot say
    which class to rebuild.
    """
    from ..core.data import DataArray  # noqa: F401 (documented schema owner)

    group.attrs['function_type'] = data.function_type
    _write_array(group, 'abscissa', data.abscissa)
    _write_array(group, 'ordinate', data.ordinate)
    _write_strings(group, 'response_dof', data.response_dof)
    if data.reference_dof is not None:
        _write_strings(group, 'reference_dof', data.reference_dof)
    # which repeat a record is, where records are the same measurement more
    # than once; absent entirely when they are not
    if data.block is not None:
        _write_strings(group, 'block', data.block)
    _write_strings(group, 'ordinate_dim', data.ordinate_dim)
    _write_strings(group, 'comment', data.comment)
    # '' marks a record whose units have not been defined
    _write_strings(group, 'ordinate_unit',
                   [u or '' for u in data.ordinate_unit])
    _write_strings(group, 'reference_unit',
                   [u or '' for u in data.reference_unit])
    # what the source said the quantity was, where it said that but gave no
    # scale; '' where it said nothing
    _write_strings(group, 'dimension_hint',
                   [h or '' for h in data.dimension_hint])
    # the concrete class: a Specification shares the PSD function type, so
    # the code alone cannot say which to rebuild
    group.attrs['data_class'] = type(data).__name__
    # what an SRS was worked out at. Two numbers that change what the
    # curve means, so they travel with it rather than being assumed
    if getattr(data, 'q', None) is not None:
        group.attrs['srs_q'] = float(data.q)
        group.attrs['srs_kind'] = str(data.kind)
    # which specification tone an extracted sine level answers to, and
    # where its sweep was found in the recording
    if getattr(data, 'tone', ''):
        group.attrs['sine_tone'] = str(data.tone)
        group.attrs['sine_onset'] = float(data.onset)
        group.attrs['sine_drift_hz'] = float(getattr(data, 'drift_hz', 0.0))
        if getattr(data, 'seconds', None) is not None:
            group.create_dataset('sine_seconds', data=data.seconds)
        # the noise floor beside each reading and where the tone was
        # under it (2026-09-30): what the plot marks and the judge skips
        if getattr(data, 'floor', None) is not None:
            group.create_dataset('sine_floor', data=data.floor)
        if getattr(data, 'below_floor', None) is not None:
            group.create_dataset('sine_below_floor',
                                 data=data.below_floor.astype(np.uint8))
    # how a spectrum's values are read between the points. It travels
    # because it cannot be worked out again from the numbers: a
    # specification computed from a record and one written by hand can
    # both arrive as several hundred evenly spaced lines — Rattlesnake
    # writes its own on the control lines — and only the first is a
    # density. Left behind, a reopened project drew four thousand lines
    # of one as a power law through themselves.
    if getattr(data, 'interpolation', None) is not None:
        group.attrs['interpolation'] = str(data.interpolation)
    # a specification's band constraints — symmetric, uniform, per
    # channel — are how its sheet moves the bands, not derivable from
    # limits that happen to mirror (2026-09-06)
    if getattr(data, 'band_constraints', None):
        import json

        group.attrs['band_constraints'] = json.dumps(data.band_constraints)
    # a held comparison scale is the user's judgment, not a derivable
    # number — auto-detected scaling is None here and re-detected on
    # every comparison, so only a hand-set value travels
    if getattr(data, 'scale_db', None) is not None:
        group.attrs['scale_db'] = int(data.scale_db)
    # simulated or measured, only when something said; absent reads as
    # unknown, so a file from before the field reads back the same
    if data.source is not None:
        group.attrs['source'] = data.source
    # a mixed-mode floor's narrowbands and the width left out around its
    # tones: attributes of the specification's own group, as its band
    # constraints are, so a mixed test adds no group of its own to the
    # file and an older reader opens the floor as the random
    # specification it is
    if getattr(data, 'narrowbands', ()):
        from ..core.mixed_mode import narrowbands_json

        group.attrs['narrowbands'] = narrowbands_json(data.narrowbands)
    if getattr(data, 'tone_exclusion', None) is not None:
        group.attrs['tone_exclusion'] = float(data.tone_exclusion)
    for name, values in getattr(data, 'limits', {}).items():
        group.create_dataset(f'limit_{name}', data=values)
    # how a time history is to be cut into frames, when something has
    # said — from the file it was imported from, or from the averaging
    # view. Five attributes rather than a group: it is five numbers.
    # each channel's role, when something has said — DOF, quantity and
    # role in threes, flat, as `TimeHistory.roles` holds them keyed
    roles = getattr(data, 'roles', None)
    if roles is not None:
        _write_strings(group, 'roles',
                       [str(v) for (dof, quantity), role in roles.items()
                        for v in (dof, quantity, role)])
    # which run the history is, when something has said: JSON, the
    # known fields and where each came from. Absent when nothing has,
    # so a history without one is stored as it always was
    identity = getattr(data, 'run_identity', None)
    if identity is not None and identity.to_json():
        import json

        group.attrs['run_identity'] = json.dumps(identity.to_json())
    averaging = getattr(data, 'averaging', None)
    if averaging is not None:
        group.attrs['averaging_frame_length'] = averaging.frame_length
        group.attrs['averaging_overlap'] = averaging.overlap
        group.attrs['averaging_window'] = averaging.window
        group.attrs['averaging_frames'] = averaging.frames
        group.attrs['averaging_start'] = averaging.start
        group.attrs['averaging_detrend'] = averaging.detrend
        group.attrs['averaging_start_set'] = bool(averaging.start_set)
        if averaging.window_parameter is not None:
            group.attrs['averaging_window_parameter'] = \
                averaging.window_parameter
    # the low-pass the history is read through, when something has
    # said — two numbers, so two attributes, like the averaging's five
    filtering = getattr(data, 'filtering', None)
    if filtering is not None:
        # a pass-band edge that is not set is *absent*, matching the
        # class: None is the meaning, not a value to spell
        if filtering.low is not None:
            group.attrs['filtering_low'] = filtering.low
        if filtering.high is not None:
            group.attrs['filtering_high'] = filtering.high
        group.attrs['filtering_order'] = filtering.order
    setting = getattr(data, 'sine_extraction', None)
    if setting is not None:
        # the automatic leaves `sine_cycles` out, which is how it reads
        # back as automatic; what it chose travels beside it
        if setting.cycles is not None:
            group.attrs['sine_cycles'] = float(setting.cycles)
        group.attrs['sine_target_db'] = float(setting.target_db)
        group.attrs['sine_refine'] = bool(setting.refine)
        if setting.chosen is not None:
            group.attrs['sine_chosen'] = float(setting.chosen)
    truncation = getattr(data, 'truncation', None)
    if truncation is not None:
        group.attrs['truncation_start'] = truncation.start
        group.attrs['truncation_stop'] = truncation.stop
    # where the shocks are, when something has said. One row of
    # (start, duration) per event rather than attributes: there is no
    # fixed number of them
    # the width of each band, for a spectrum whose bins are not the
    # midpoints between its own lines
    if getattr(data, 'bandwidth', None) is not None:
        group.create_dataset('bandwidth', data=np.asarray(data.bandwidth))
    # what each record's average is worth. It travels because it cannot
    # be worked out from the spectrum: the frames it came from are the
    # source history's, which may have been re-framed since or deleted
    if getattr(data, 'statistical_dof', None) is not None:
        group.create_dataset('statistical_dof',
                             data=np.asarray(data.statistical_dof,
                                             dtype=float))
    # and how its window ties neighboring lines, for the same reason
    if getattr(data, 'line_correlation', None) is not None:
        group.create_dataset('line_correlation',
                             data=np.asarray(data.line_correlation,
                                             dtype=float))
    found = getattr(data, 'shocks', None)
    if found:
        group.create_dataset('shocks', data=np.array(
            [[s.start, s.duration] for s in found], dtype=float))
    # the segment marks, as the shocks are: a row of (start, end) per
    # mark, with its kind and its name beside it (2026-10-16 review)
    marks = getattr(data, 'segments', None)
    if marks:
        group.create_dataset('segments', data=np.array(
            [[m[0], m[1]] for m in marks], dtype=float))
        _write_strings(group, 'segment_kinds', [str(m[2]) for m in marks])
        _write_strings(group, 'segment_names',
                       [str(m[3]) if len(m) > 3 else '' for m in marks])


def _load_segments(group):
    if 'segments' not in group:
        return None
    from ..core.replication_segments import Segment

    return tuple(Segment(float(start), float(end), kind, name)
                 for (start, end), kind, name
                 in zip(group['segments'][()],
                        _read_strings(group, 'segment_kinds'),
                        _read_strings(group, 'segment_names')))


def _load_shocks(group):
    if 'shocks' not in group:
        return None
    from ..core.shocks import Shock

    return tuple(Shock(float(start), float(duration))
                 for start, duration in group['shocks'][()])


def _load_averaging(group):
    if 'averaging_frame_length' not in group.attrs:
        return None
    from ..core.averaging import Averaging

    return Averaging(
        frame_length=int(group.attrs['averaging_frame_length']),
        overlap=float(group.attrs['averaging_overlap']),
        window=str(group.attrs['averaging_window']),
        frames=int(group.attrs['averaging_frames']),
        start=float(group.attrs['averaging_start']),
        # absent in every file written before 2026-08-29: the
        # default is the behavior those files were computed with
        detrend=str(group.attrs.get('averaging_detrend', 'none')),
        window_parameter=(
            float(group.attrs['averaging_window_parameter'])
            if 'averaging_window_parameter' in group.attrs
            else None),
        # absent before 2026-09-30: a start those files carried was
        # chosen only if it is not the record's beginning
        start_set=bool(group.attrs.get(
            'averaging_start_set', float(group.attrs['averaging_start']) > 0.0)))


def load_data(group: h5py.Group) -> DataArray:
    """Read a data object written by `save_data`, as the class its name
    was stored under, or by its function type in a file without one.
    An attribute the file lacks leaves the object's default, so older
    files load unchanged."""
    from ..core.data import NAMED_CLASSES, class_for_function_type

    cls = NAMED_CLASSES.get(group.attrs.get('data_class'))
    if cls is None:
        cls = class_for_function_type(group.attrs['function_type'])
    limits = {key[len('limit_'):]: group[key][()]
              for key in group if key.startswith('limit_')}
    if 'bandwidth' in group:
        limits['bandwidth'] = group['bandwidth'][()]
    if 'srs_q' in group.attrs:
        limits['q'] = float(group.attrs['srs_q'])
        limits['kind'] = str(group.attrs['srs_kind'])
    if 'sine_tone' in group.attrs:
        limits['tone'] = str(group.attrs['sine_tone'])
        limits['onset'] = float(group.attrs['sine_onset'])
        limits['drift_hz'] = float(group.attrs.get('sine_drift_hz', 0.0))
        if 'sine_seconds' in group:
            limits['seconds'] = group['sine_seconds'][()]
        if 'sine_floor' in group:
            limits['floor'] = group['sine_floor'][()]
        if 'sine_below_floor' in group:
            limits['below_floor'] = group['sine_below_floor'][()].astype(bool)
    data = cls(
        **limits,
        abscissa=_read_array(group['abscissa']),
        ordinate=_read_array(group['ordinate']),
        response_dof=_read_strings(group, 'response_dof'),
        reference_dof=(_read_strings(group, 'reference_dof')
                       if 'reference_dof' in group else None),
        block=_read_strings(group, 'block') if 'block' in group else None,
        ordinate_dim=_read_strings(group, 'ordinate_dim'),
        comment=_read_strings(group, 'comment'),
        ordinate_unit=_read_strings(group, 'ordinate_unit'),
        reference_unit=_read_strings(group, 'reference_unit'),
        dimension_hint=_read_strings(group, 'dimension_hint'),
    )
    if 'interpolation' in group.attrs:
        data.interpolation = str(group.attrs['interpolation'])
    if 'band_constraints' in group.attrs:
        import json

        data.band_constraints = json.loads(str(group.attrs['band_constraints']))
    if 'scale_db' in group.attrs:
        data.scale_db = int(group.attrs['scale_db'])
    if 'source' in group.attrs:
        source = group.attrs['source']
        data.source = source.decode('utf-8') if isinstance(source, bytes) else str(source)
    if 'narrowbands' in group.attrs:
        from ..core.mixed_mode import narrowbands_from_json

        data.narrowbands = narrowbands_from_json(
            str(group.attrs['narrowbands']))
    if 'tone_exclusion' in group.attrs:
        data.tone_exclusion = float(group.attrs['tone_exclusion'])
    if 'statistical_dof' in group:
        data.statistical_dof = group['statistical_dof'][()]
    if 'line_correlation' in group:
        data.line_correlation = group['line_correlation'][()]
    averaging = _load_averaging(group)
    if averaging is not None:
        data.averaging = averaging
    if 'roles' in group:
        flat = _read_strings(group, 'roles')
        data.roles = {(dof, quantity): role for dof, quantity, role
                      in zip(flat[::3], flat[1::3], flat[2::3])}
    if 'sine_target_db' in group.attrs:
        from ..core.sine import SineExtraction

        data.sine_extraction = SineExtraction(
            cycles=(float(group.attrs['sine_cycles'])
                    if 'sine_cycles' in group.attrs else None),
            target_db=float(group.attrs['sine_target_db']),
            refine=bool(group.attrs['sine_refine']),
            chosen=(float(group.attrs['sine_chosen'])
                    if 'sine_chosen' in group.attrs else None))
    if 'filtering_low' in group.attrs or 'filtering_high' in group.attrs:
        from ..core.filters import Filtering

        low = group.attrs.get('filtering_low')
        high = group.attrs.get('filtering_high')
        data.filtering = Filtering(
            low=None if low is None else float(low),
            high=None if high is None else float(high),
            order=int(group.attrs['filtering_order']))
    if 'truncation_start' in group.attrs:
        from ..core.truncate import Truncation

        data.truncation = Truncation(
            float(group.attrs['truncation_start']),
            float(group.attrs['truncation_stop']))
    found = _load_shocks(group)
    if found is not None:
        data.shocks = found
    marks = _load_segments(group)
    if marks is not None:
        data.segments = marks
    if 'run_identity' in group.attrs:
        import json

        from ..core.runs import RunIdentity

        stored = group.attrs['run_identity']
        if isinstance(stored, bytes):
            stored = stored.decode('utf-8')
        data.run_identity = RunIdentity.from_json(
            json.loads(stored), str(group.attrs.get('name', 'a time history')))
    return data


def save_channel_table(table: ChannelTable, group: h5py.Group) -> None:
    """Write a channel table into `group`, one dataset per column in its
    order, and its control loops as numbered subgroups: an environment
    name is free text, and h5py reads a slash in a group name as a
    path."""
    group.attrs['column_order'] = table.column_names
    for name in table.column_names:
        # the typed columns decide the storage: node joined channel as an
        # integer when the schema grew rules, and h5py refuses to write
        # int64 through a string dtype rather than coercing it
        if table.COLUMNS[name].kind == 'int':
            group.create_dataset(name, data=table[name])
        else:
            _write_strings(group, name, list(table[name]))
    if not table.environments:
        return
    # the control loops, one subgroup each, numbered rather than named:
    # a controller's environment name is free text, and h5py reads a
    # slash in a group name as a path
    loops = group.create_group('environments')
    for k, (name, environment) in enumerate(table.environments.items()):
        sub = loops.create_group(str(k))
        sub.attrs['name'] = name
        sub.create_dataset('controls', data=np.asarray(environment.controls,
                                                      dtype=np.int64))
        sub.create_dataset('drives', data=np.asarray(environment.drives,
                                                    dtype=np.int64))
        for form in ('response', 'reference'):
            matrix = environment.matrix(form)
            if matrix is not None:
                sub.create_dataset(f'{form}_transformation', data=matrix)


def load_channel_table(group: h5py.Group) -> ChannelTable:
    """Read a channel table written by `save_channel_table`. Columns are
    read by their stored type rather than the current column specs, so
    a column the schema has since dropped still reads and the
    constructor decides what is kept."""
    from ..core.channel_table import ChannelTable, Environment

    columns = {}
    for name in group.attrs['column_order']:
        # read by what is stored rather than by the column specs, so
        # the storage rule lives once, in the writer — and a column the
        # schema has since dropped still reads (the constructor is what
        # decides which columns survive)
        data = group[name]
        columns[name] = (data[()] if data.dtype.kind in 'iuf'
                         else _read_strings(group, name))
    environments = {}
    if 'environments' in group:
        loops = group['environments']
        for key in sorted(loops, key=int):
            sub = loops[key]
            environments[str(sub.attrs['name'])] = Environment(
                controls=[int(n) for n in sub['controls'][()]],
                drives=[int(n) for n in sub['drives'][()]],
                response_transformation=(
                    sub['response_transformation'][()]
                    if 'response_transformation' in sub else None),
                reference_transformation=(
                    sub['reference_transformation'][()]
                    if 'reference_transformation' in sub else None))
    return ChannelTable(columns, environments=environments)


def save_shapes(shapes: ShapeSet, group: h5py.Group) -> None:
    """Write a shape set into `group`: frequencies, damping, shape matrix
    and modal mass as datasets with its strings, mass unit and unscaled
    flag; modal damping only when the set has it."""
    group.attrs['mass_unit'] = shapes.mass_unit or ''
    group.attrs['unscaled'] = bool(getattr(shapes, 'unscaled', False))
    group.create_dataset('frequency', data=shapes.frequency)
    group.create_dataset('damping', data=shapes.damping)
    _write_array(group, 'shape_matrix', shapes.shape_matrix)
    group.create_dataset('modal_mass', data=shapes.modal_mass)
    # written only when a source carried one, so an old file and a set
    # that never had it read back identically
    if shapes.modal_damping is not None:
        group.create_dataset('modal_damping', data=shapes.modal_damping)
    _write_strings(group, 'coordinate', shapes.coordinate)
    _write_strings(group, 'comment', shapes.comment)
    _write_strings(group, 'description', shapes.description)


def load_shapes(group: h5py.Group) -> ShapeSet:
    """Read a shape set written by `save_shapes`; an empty mass unit
    reads back as None."""
    from ..core.shapes import ShapeSet

    return ShapeSet(
        frequency=group['frequency'][()],
        damping=group['damping'][()],
        coordinate=_read_strings(group, 'coordinate'),
        shape_matrix=_read_array(group['shape_matrix']),
        modal_mass=group['modal_mass'][()],
        comment=_read_strings(group, 'comment'),
        description=_read_strings(group, 'description'),
        mass_unit=group.attrs['mass_unit'] or None,
        unscaled=bool(group.attrs['unscaled']),
        modal_damping=(group['modal_damping'][()]
                       if 'modal_damping' in group else None),
    )


def save_report(report: Report, group: h5py.Group) -> None:
    """Write a report into `group`: title and marking as attributes, its
    blocks as one JSON string. Features and identity are written only
    when entered, so a report with neither is stored as it was before
    they existed."""
    import json

    import h5py

    group.attrs['title'] = report.title
    group.attrs['marking'] = report.marking
    group.attrs['marking_color'] = report.marking_color
    if report.features:
        # written only when there are any, so a report with none is
        # stored exactly as it was before features existed
        group.attrs['features'] = json.dumps(report.features)
    if report.identity:
        # likewise only when entered: identification fields and the
        # issue record (`report/identity.py`)
        group.attrs['identity'] = json.dumps(report.identity)
    group.create_dataset('blocks', data=json.dumps(report.blocks),
                         dtype=h5py.string_dtype('utf-8'))


def load_report(group: h5py.Group) -> Report:
    """Read a report written by `save_report`; a file from before
    features or identity loads with none."""
    import json

    from ..core.report import Report

    blocks = group['blocks'][()]
    if isinstance(blocks, bytes):
        blocks = blocks.decode('utf-8')
    # a file from before report features has none, and loads with none
    features = group.attrs.get('features', '[]')
    if isinstance(features, bytes):
        features = features.decode('utf-8')
    features = json.loads(features)
    identity = group.attrs.get('identity', '{}')
    if isinstance(identity, bytes):
        identity = identity.decode('utf-8')
    return Report(title=str(group.attrs['title']),
                  blocks=json.loads(blocks),
                  marking=str(group.attrs['marking']),
                  marking_color=str(group.attrs['marking_color']),
                  features=features, identity=json.loads(identity))


def save_predictions(predictions: Predictions, group: h5py.Group) -> None:
    """Write a `Predictions` object into `group`: its comment, and one
    subgroup per prediction holding its values as JSON
    (`Prediction.as_record`) and its curve, when it has one."""
    import json

    group.attrs['comment'] = predictions.comment
    group.attrs['count'] = len(predictions)
    for k, prediction in enumerate(predictions):
        # index-named subgroups, in the set's order: a channel is user
        # text, which an HDF5 group name cannot always hold
        sub = group.create_group(str(k))
        sub.attrs['record'] = json.dumps(prediction.as_record())
        if prediction.curve is not None:
            sub.create_dataset('curve_frequency', data=prediction.curve[0])
            sub.create_dataset('curve_values', data=prediction.curve[1])


def load_predictions(group: h5py.Group) -> Predictions:
    """Read a `Predictions` object written by `save_predictions`, each
    prediction rebuilt from its JSON record and its curve."""
    import json

    from ..core.prediction import Prediction, Predictions

    held = []
    for k in range(int(group.attrs['count'])):
        sub = group[str(k)]
        record = sub.attrs['record']
        if isinstance(record, bytes):
            record = record.decode('utf-8')
        curve = ((sub['curve_frequency'][()], sub['curve_values'][()])
                 if 'curve_frequency' in sub else None)
        held.append(Prediction.from_record(json.loads(record), curve))
    return Predictions(held, comment=str(group.attrs['comment']))


def save_photos(photos: Photos, group: h5py.Group) -> None:
    """Write photos into `group`: names and formats as strings, and each
    image as its file's own bytes, neither decoded nor re-encoded."""
    _write_strings(group, 'names', photos.names)
    _write_strings(group, 'formats', photos.formats)
    for i, image in enumerate(photos.images):
        # the file's own bytes, verbatim — no decode, no re-encode
        group.create_dataset(f'image_{i:04d}',
                             data=np.frombuffer(image, dtype=np.uint8))


def load_photos(group: h5py.Group) -> Photos:
    """Read photos written by `save_photos`, each image as the bytes it
    was saved with."""
    from ..core.photos import Photos

    names = _read_strings(group, 'names')
    return Photos(names=names,
                  formats=_read_strings(group, 'formats'),
                  images=[group[f'image_{i:04d}'][()].tobytes()
                          for i in range(len(names))])


def _file_path(group: h5py.Group) -> str | None:
    """The path of the file `group` is in, or None for a file held in
    memory (a MATLAB or ESCDF layout being built), whose name is only a
    label."""
    if group.file.driver in ('core', 'fileobj'):
        return None
    return os.path.abspath(group.file.filename)


def save_run_log_extract(extract: RunLogExtract, group: h5py.Group) -> None:
    """Write a `RunLogExtract` into `group`: the snapshot's JSON Lines
    whole, in a dataset (it can pass the 64 KiB an attribute holds), and
    what it was taken from as attributes. The log's path is stored
    relative to the project file when the log is in the file's folder
    or one beneath it (`run_log_extract.stored_path`), so a project
    moved with its log still finds it."""
    import json

    import h5py

    from ..core.run_log_extract import stored_path

    group.attrs['path'] = stored_path(extract.path, _file_path(group))
    group.attrs['test'] = extract.test
    group.attrs['upto'] = extract.upto
    group.attrs['taken'] = extract.taken.isoformat()
    group.attrs['log_size'] = extract.log_size
    group.attrs['after'] = extract.after
    group.attrs['after_in_test'] = extract.after_in_test
    group.attrs['problems'] = json.dumps(list(extract.problems))
    group.create_dataset('log', data=extract.text,
                         dtype=h5py.string_dtype('utf-8'))


def load_run_log_extract(group: h5py.Group) -> RunLogExtract:
    """Read a `RunLogExtract` written by `save_run_log_extract`. A log
    path stored relative to the project file is resolved against where
    the file is now (`run_log_extract.resolved_path`)."""
    import json

    from ..core.run_log_extract import RunLogExtract, resolved_path

    def text(value: Any) -> str:
        return value.decode('utf-8') if isinstance(value, bytes) else str(value)

    attrs = group.attrs
    return RunLogExtract(
        text(group['log'][()]),
        path=resolved_path(text(attrs['path']), _file_path(group)),
        test=int(attrs['test']), upto=int(attrs['upto']),
        taken=text(attrs['taken']), log_size=int(attrs['log_size']),
        after=int(attrs['after']), after_in_test=int(attrs['after_in_test']),
        problems=json.loads(text(attrs['problems'])))


def save_matches(matches: MatchedModes, group: h5py.Group) -> None:
    """Write matched modes into `group`: the two shape sets' names and
    geometries as attributes, the pairs as an (n, 2) integer array and
    their MACs as committed."""
    group.attrs['first'] = matches.first
    group.attrs['second'] = matches.second
    group.attrs['first_geometry'] = matches.first_geometry or ''
    group.attrs['second_geometry'] = matches.second_geometry or ''
    group.create_dataset('pairs', data=np.asarray(
        matches.pairs, dtype=np.int64).reshape(-1, 2))
    group.create_dataset('macs', data=np.asarray(
        matches.macs, dtype=np.float64))


def load_matches(group: h5py.Group) -> MatchedModes:
    """Read matched modes written by `save_matches`; an empty geometry
    name reads back as None."""
    from ..core.matches import MatchedModes

    return MatchedModes(
        group.attrs['first'], group.attrs['second'],
        pairs=group['pairs'][()].tolist(),
        macs=group['macs'][()].tolist(),
        first_geometry=group.attrs['first_geometry'] or None,
        second_geometry=group.attrs['second_geometry'] or None)


def save_sine_specification(spec: SineSweepSpecification,
                            group: h5py.Group) -> None:
    """Write a sine sweep specification into `group`, one subgroup per
    tone: its breakpoints with the sweep law of each segment, and its
    bands where it has them."""
    _write_strings(group, 'response_dof', spec.response_dof)
    group.attrs['ordinate_dim'] = spec.ordinate_dim
    group.attrs['ordinate_unit'] = spec.ordinate_unit or ''
    group.attrs['comment'] = spec.comment
    _write_strings(group, 'tone_order', [tone.name for tone in spec.tones])
    tones = group.create_group('tones')
    for k, tone in enumerate(spec.tones):
        # index-named subgroups: tone names are user text and h5py
        # group names cannot hold a '/', so the order array holds the
        # names and the groups hold the numbers
        sub = tones.create_group(str(k))
        sub.attrs['name'] = tone.name
        sub.attrs['start_time'] = tone.start_time
        sub.create_dataset('frequency', data=tone.frequency)
        sub.create_dataset('amplitude', data=tone.amplitude)
        sub.create_dataset('phase', data=tone.phase)
        sub.create_dataset('segment_type', data=tone.segment_type)
        sub.create_dataset('segment_rate', data=tone.segment_rate)
        for name, values in tone.limits.items():
            sub.create_dataset(name, data=values)


def load_sine_specification(group: h5py.Group) -> SineSweepSpecification:
    """Read a sine sweep specification written by
    `save_sine_specification`, its tones in `tone_order`'s order; an
    empty unit reads back as None, undeclared."""
    from ..core.sine import SineSweepSpecification, SineTone

    tones = []
    for k, name in enumerate(_read_strings(group, 'tone_order')):
        sub = group['tones'][str(k)]
        limits = {limit: sub[limit][()]
                  for limit in SineTone.LIMITS if limit in sub}
        tones.append(SineTone(
            name=name, start_time=float(sub.attrs['start_time']),
            frequency=sub['frequency'][()],
            amplitude=sub['amplitude'][()], phase=sub['phase'][()],
            segment_type=sub['segment_type'][()],
            segment_rate=sub['segment_rate'][()], from_file=True, **limits))
    return SineSweepSpecification(
        tones=tones, response_dof=_read_strings(group, 'response_dof'),
        ordinate_dim=str(group.attrs['ordinate_dim']),
        ordinate_unit=str(group.attrs['ordinate_unit']) or None,
        comment=str(group.attrs['comment']))


def save_sine_levels(levels: SineLevelSet, group: h5py.Group) -> None:
    """Write a sine level set into `group`, one subgroup per tone, each
    in the ordinary data layout, since the set only groups the levels."""
    _write_strings(group, 'tone_order',
                   [level.tone for level in levels.levels])
    if levels.cycles is not None:
        group.attrs['sine_cycles'] = float(levels.cycles)
    tones = group.create_group('tones')
    for k, level in enumerate(levels.levels):
        save_data(level, tones.create_group(str(k)))


def load_sine_levels(group: h5py.Group) -> SineLevelSet:
    """Read a sine level set written by `save_sine_levels`, each tone's
    level by `load_data`."""
    from ..core.sine import SineLevelSet

    return SineLevelSet([load_data(group['tones'][str(k)])
                         for k in range(len(
                             _read_strings(group, 'tone_order')))],
                        cycles=(float(group.attrs['sine_cycles'])
                                if 'sine_cycles' in group.attrs else None))


def _savers():
    from ..core.channel_table import ChannelTable
    from ..core.data import DataArray
    from ..core.geometry import Geometry
    from ..core.matches import MatchedModes
    from ..core.photos import Photos
    from ..core.prediction import Predictions
    from ..core.report import Report
    from ..core.run_log_extract import RunLogExtract
    from ..core.shapes import ShapeSet
    from ..core.sine import SineLevelSet, SineSweepSpecification

    return [(Geometry, 'geometry', save_geometry),
            (SineSweepSpecification, 'sine_specification',
             save_sine_specification),
            (SineLevelSet, 'sine_levels', save_sine_levels),
            (DataArray, 'data', save_data),
            (ShapeSet, 'shapes', save_shapes),
            (Report, 'report', save_report),
            (Photos, 'photos', save_photos),
            (MatchedModes, 'matches', save_matches),
            (ChannelTable, 'channel_table', save_channel_table),
            (Predictions, 'predictions', save_predictions),
            (RunLogExtract, 'run_log_extract', save_run_log_extract)]


_LOADERS = {'geometry': load_geometry,
            'sine_specification': load_sine_specification,
            'sine_levels': load_sine_levels,
            'data': load_data,
            'shapes': load_shapes,
            'report': load_report,
            'photos': load_photos,
            'matches': load_matches,
            'channel_table': load_channel_table,
            'predictions': load_predictions,
            'run_log_extract': load_run_log_extract}


def _saver_for(obj):
    for cls, group_name, saver in _savers():
        if isinstance(obj, cls):
            return group_name, saver
    raise TypeError(f"Don't know how to save {type(obj).__name__}")


def hforge_path(path: str | os.PathLike) -> str:
    """`path` with `.hforge` added when it does not end in it. A `.vdyn`
    path is refused: Visual Dynamics' files are opened, never written."""
    path = str(path)
    if path.endswith('.vdyn'):
        raise ValueError(
            f'{path}: Hertzforge saves .hforge files. A .vdyn file opens, '
            'and saving it again writes .hforge.')
    return path if path.endswith(SUFFIX) else path + SUFFIX


def save(obj: Any, path: str | os.PathLike, compress: bool = False) -> None:
    """Save one Hertzforge object to a .hforge (HDF5) file.

    Raises TypeError for an object of a kind the format does not hold,
    and ValueError for a `.vdyn` path.

    Parameters
    ----------
    obj : object
        The object to save.
    path : str or os.PathLike
        Where; `.hforge` is added when the suffix is not there.
    compress : bool, optional
        Write the large arrays with gzip (`compressed`), for archiving.

    Returns
    -------
    None
    """
    import h5py

    with (_replaced(hforge_path(path)) as partial,
          h5py.File(partial, 'w') as f, compressed(compress)):
        save_into(obj, f)


def save_into(obj: Any, f: h5py.File) -> None:
    """Write one object into an open, empty HDF5 file.

    This is `save` without opening the file, so that other formats
    (`io.matlab`, `io.escdf_objects`) can write the same layout into
    memory and store it inside their own files.
    """
    group_name, saver = _saver_for(obj)
    f.attrs[STAMP] = BASE_SCHEMA
    saver(obj, f.create_group(group_name))


def fingerprint(obj: Any) -> str:
    """A digest of everything saving `obj` would write, with each mapped
    array named by where it is mapped rather than read.

    Two fingerprints of one object differ when anything a save would
    write has changed: a unit, a name, a value in memory, or an array
    replaced by another. A project compares them to tell an object it
    holds by reference that is still the one in the other file from one
    edited since, which a reference would lose. A mapped array costs
    nothing here, so a run of any size is fingerprinted in the time its
    small fields take. Within one session only: the mapping's address
    is part of the name.

    What it cannot see is a value changed inside a mapped array in
    place, since the array is named, not read. The core never does that
    without also changing a unit or a name (`DataArray.define_units`
    rescales in place and records the unit); a script that writes into
    a referenced run's samples should copy the array first.
    """
    import h5py

    group_name, saver = _saver_for(obj)
    token = _fingerprinting.set(True)
    try:
        with h5py.File(f'fingerprint-{uuid.uuid4().hex}', 'w', driver='core',
                       backing_store=False) as f:
            saver(obj, f.create_group(group_name))
            return _tree_digest(f)
    finally:
        _fingerprinting.reset(token)


def _tree_digest(f: h5py.File) -> str:
    import h5py

    hasher = hashlib.sha256()

    def add(value: Any) -> None:
        if isinstance(value, np.ndarray) and value.dtype.kind != 'O':
            hasher.update(value.dtype.str.encode())
            hasher.update(repr(value.shape).encode())
            hasher.update(np.ascontiguousarray(value).tobytes())
        elif isinstance(value, np.ndarray):
            hasher.update(repr(value.tolist()).encode())
        else:
            hasher.update(repr(value).encode())

    names: list[str] = []
    f.visit(names.append)
    for name in ['', *sorted(names)]:
        item = f[name] if name else f
        hasher.update(f'\0{name}\0'.encode())
        for key in sorted(item.attrs):
            hasher.update(f'@{key}='.encode())
            add(item.attrs[key])
        if isinstance(item, h5py.Dataset):
            add(item[()])
    return hasher.hexdigest()


def object_sizes(path: str | os.PathLike) -> dict[str, tuple[str, int]]:
    """{object name: (kind, bytes stored)} for the objects in a
    `.hforge` file, with '' as the name of a file's single object. An
    object the file itself holds by reference stores nothing."""
    import h5py

    def stored(group: h5py.Group) -> int:
        sizes: list[int] = []
        group.visititems(lambda _name, item: sizes.append(
            item.id.get_storage_size()) if isinstance(item, h5py.Dataset)
            else None)
        return sum(sizes)

    with h5py.File(path, 'r') as f:
        if 'objects' in f:
            return {group.attrs['name']: (group.attrs['kind'], stored(group))
                    for group in f['objects'].values()}
        kind = next((kind for kind in _LOADERS if kind in f), None)
        return {} if kind is None else {'': (kind, stored(f[kind]))}


def _write_reference(group: h5py.Group, reference: Reference,
                     folder: str) -> None:
    """An object held by reference: where the file is, relative to the
    project's folder and in full, the object's name in it, and the
    file's size, SHA-256 and modification time when it was referenced."""
    try:
        relative = os.path.relpath(reference.path, folder) if folder else ''
    except ValueError:
        # another drive, on Windows: there is no relative path
        relative = ''
    group.attrs['reference_path'] = (relative.replace(os.sep, '/')
                                     or reference.path)
    group.attrs['reference_absolute'] = reference.path
    group.attrs['reference_object'] = reference.object
    group.attrs['reference_size'] = int(reference.size)
    group.attrs['reference_sha256'] = reference.sha256
    group.attrs['reference_modified_ns'] = int(reference.modified_ns)


def _follow(group: h5py.Group, folder: str, depth: int = 0
            ) -> tuple[Any, Reference]:
    """The object a reference group names, and its reference as found.

    The file is looked for by the path relative to `folder` (the
    project's), then at its absolute path, then by its name in `folder`.
    A file with the size and modification time recorded is taken as
    the one referenced; one with another time is hashed, and taken when
    the SHA-256 matches, since a copied or moved file keeps its bytes.
    When no candidate is the file, the object is None and the reference
    says why (`NOT_FOUND`, `CHANGED`)."""
    import h5py

    from ..core.data_index import CHANGED, NOT_FOUND, Reference, digest, file_state

    attrs = group.attrs
    relative = str(attrs['reference_path'])
    absolute = str(attrs['reference_absolute'])
    expected = Reference(
        os.path.join(folder, *relative.split('/'))
        if folder and not os.path.isabs(relative) else absolute,
        str(attrs['reference_object']), str(attrs['kind']),
        int(attrs['reference_size']), str(attrs['reference_sha256']),
        int(attrs['reference_modified_ns']))
    candidates = [expected.path, absolute]
    if folder:
        candidates.append(os.path.join(folder, relative.split('/')[-1]))
    found = False
    for where in dict.fromkeys(os.path.abspath(c) for c in candidates):
        if not os.path.isfile(where):
            continue
        found = True
        size, modified = file_state(where)
        if size != expected.size:
            continue
        if (modified != expected.modified_ns
                and digest(where)[1] != expected.sha256):
            continue
        with h5py.File(where, 'r') as f:
            if expected.object:
                target = next((g for g in f.get('objects', {}).values()
                               if g.attrs['name'] == expected.object), None)
            else:
                target = f.get(expected.kind)
            if target is None:
                continue
            if 'reference_sha256' in target.attrs:
                if depth >= REFERENCE_DEPTH:
                    continue
                return _follow(target, os.path.dirname(where), depth + 1)
            reference = Reference(where, expected.object, expected.kind,
                                  size, expected.sha256, modified)
            return _LOADERS[expected.kind](target), reference
    reason = CHANGED if found else NOT_FOUND
    return None, Reference(expected.path, expected.object, expected.kind,
                           expected.size, expected.sha256,
                           expected.modified_ns, reason)


# A whole test read back from one file is a Project: {name: object} in
# saved order, plus what the test carried — its name, active geometry,
# type and links. The alias stays because a saved test is exactly what
# `Project` means, and callers testing `isinstance(x, TestContents)`
# are asking "is this a project, not a foreign reader's dict?"
TestContents = Project


def save_test(path: str | os.PathLike, name: str,
              objects: Mapping[str, Any],
              active_geometry: str | None = None,
              project_type: str | None = None,
              object_groups: Sequence[Mapping[str, Any]] | None = None,
              provenance: Mapping[str, Any] | None = None,
              data_index: Sequence[Mapping[str, Any]] | None = None,
              compress: bool = False,
              references: Mapping[str, Reference] | None = None) -> None:
    """Save a whole test, every named object, to one .hforge file.

    Objects go in numbered groups with the name as an attribute, so a
    name may contain anything h5py would read as a path. `objects` maps
    each name to its object, in order. `object_groups` is the object
    groups, each a mapping with its `members` (object names);
    `provenance` records how each derived object was computed. Both are
    stored as JSON. `data_index` is the project's data index, its
    entries as plain values (`DataIndex.records`), stored as JSON in a
    `data_index` dataset at the root when there are any. `.hforge` is
    added to `path` when it is not there, and a `.vdyn` path raises
    ValueError. `compress` writes the large arrays with gzip
    (`compressed`), for archiving.

    `references` are the objects written as references to other files
    rather than copied, by name (`Reference`): a name in `objects` is
    written as its reference in its place, and any other is written
    after the objects, as a reference kept for an object that is not
    loaded. A file with any reference is stamped schema 2, and one
    without, schema 1.
    """
    import h5py

    with (_replaced(hforge_path(path)) as partial,
          h5py.File(partial, 'w') as f, compressed(compress)):
        save_test_into(f, name, objects, active_geometry, project_type,
                       object_groups, provenance, data_index, references)


def save_test_into(f: h5py.File, name: str, objects: Mapping[str, Any],
                   active_geometry: str | None = None,
                   project_type: str | None = None,
                   object_groups: Sequence[Mapping[str, Any]] | None = None,
                   provenance: Mapping[str, Any] | None = None,
                   data_index: Sequence[Mapping[str, Any]] | None = None,
                   references: Mapping[str, Reference] | None = None
                   ) -> None:
    """`save_test` into an open, empty HDF5 file (see `save_into`).
    References are written relative to the folder the file is in."""
    references = dict(references or {})
    f.attrs[STAMP] = REFERENCE_SCHEMA if references else BASE_SCHEMA
    f.attrs['test_name'] = name
    f.attrs['active_geometry'] = active_geometry or ''
    f.attrs['project_type'] = project_type or ''
    import json
    f.attrs['object_groups'] = json.dumps(object_groups or [])
    # how each derived object was computed, for the staleness
    # badges — settings fingerprints, so they survive the file
    f.attrs['provenance'] = json.dumps(provenance or {})
    if data_index:
        # the files the project rests on, by name, size and SHA-256. A
        # dataset, not an attribute: an attached settings list rides in
        # it, and an HDF5 attribute holds 64 KiB at most. Written only
        # when there is one, so a project with none is stored as it
        # always was; a reader that predates it opens the file and
        # leaves the dataset alone
        import h5py

        f.create_dataset('data_index', data=json.dumps(list(data_index)),
                         dtype=h5py.string_dtype('utf-8'))
    container = f.create_group('objects')
    folder = (os.path.dirname(os.path.abspath(f.filename))
              if f.driver != 'core' else '')
    names = [*objects, *(name for name in references if name not in objects)]
    for i, obj_name in enumerate(names):
        group = container.create_group(f'{i:04d}')
        group.attrs['name'] = obj_name
        if obj_name in references:
            group.attrs['kind'] = references[obj_name].kind
            _write_reference(group, references[obj_name], folder)
            continue
        kind, saver = _saver_for(objects[obj_name])
        group.attrs['kind'] = kind
        saver(objects[obj_name], group)


def load(path: str | os.PathLike,
         progress: Callable[[int, int], None] | None = None) -> Any:
    """Load a .hforge or .vdyn file: the object it holds, or a whole test.

    The stamp inside the file decides how it is read, not the suffix.

    For a whole test, `progress` is called as (objects loaded, objects
    in the file), once at the start with 0 and once per object, because
    a large project can take minutes to load and only the reader knows
    how far along it is. For a single-object file it is not called,
    since loading one object is a single step.

    Parameters
    ----------
    path : str or os.PathLike
        The `.hforge` or `.vdyn` file.
    progress : callable, optional
        Called as ``progress(loaded, total)``, once up front and once per
        object.

    Returns
    -------
    object
        The object the file holds, or a `Project` for a whole test.

    Raises
    ------
    ValueError
        When the file has neither stamp (it was not written by
        Hertzforge or Visual Dynamics), was written with a newer schema,
        or holds nothing this reader recognizes.
    """
    import h5py

    with h5py.File(path, 'r') as f:
        return load_from(f, progress, str(path))


def load_from(f: h5py.File, progress: Callable[[int, int], None] | None = None,
              path: str = '') -> Any:
    """`load` from an open HDF5 file (see `save_into`). `path` names the
    file in error messages."""
    from ..core.data_index import NOT_FOUND

    # A newer stamp means a newer Hertzforge wrote fields this
    # reader has no idea exist, and half-loading someone's project
    # quietly is worse than telling them to update. A missing stamp
    # means the file is not ours at all — every writer stamps.
    if STAMP in f.attrs:
        written, newest, writer = int(f.attrs[STAMP]), SCHEMA_VERSION, 'Hertzforge'
    elif VDYN_STAMP in f.attrs:
        written, newest, writer = (int(f.attrs[VDYN_STAMP]), VDYN_SCHEMA,
                                   'Visual Dynamics')
    else:
        raise ValueError(f'{path} is not a Hertzforge or Visual Dynamics '
                         'file (no schema stamp)')
    if written > newest:
        raise ValueError(
            f'{path} was written by a newer {writer} '
            f'(schema {written}; this build reads up to '
            f'{newest}). Update Hertzforge to open it.')
    if 'objects' in f:
        objects = {}
        followed: dict[str, Reference] = {}
        unresolved: dict[str, Reference] = {}
        folder = os.path.dirname(os.path.abspath(path)) if path else ''
        keys = sorted(f['objects'])
        if progress is not None:
            progress(0, len(keys))
        for done, key in enumerate(keys, start=1):
            group = f['objects'][key]
            name = group.attrs['name']
            if 'reference_sha256' in group.attrs:
                obj, reference = _follow(group, folder)
                if obj is None:
                    unresolved[name] = reference
                else:
                    objects[name] = obj
                    followed[name] = reference
            else:
                objects[name] = _LOADERS[group.attrs['kind']](group)
            if progress is not None:
                progress(done, len(keys))
        import json
        indexed = []
        if 'data_index' in f:
            raw = f['data_index'][()]
            indexed = json.loads(raw.decode('utf-8') if isinstance(raw, bytes)
                                 else raw)
        project = Project(f.attrs['test_name'], objects,
                          f.attrs['active_geometry'] or None,
                          f.attrs['project_type'] or None,
                          json.loads(f.attrs['links' if VDYN_STAMP in f.attrs
                                             else 'object_groups']),
                          provenance=json.loads(f.attrs['provenance']),
                          data_index=indexed)
        for name, reference in followed.items():
            project.hold_by_reference(name, reference)
        project.unresolved = unresolved
        for name, reference in unresolved.items():
            said = ('was not found' if reference.reason == NOT_FOUND
                    else 'has changed since the project referred to it')
            warnings.warn(
                f'{name} is held in {reference.path}, which {said}, so the '
                'project opened without it. Its reference is kept: put the '
                'file back, or beside the project, and open the project again.',
                stacklevel=3)
        return project
    for group_name, loader in _LOADERS.items():
        if group_name in f:
            return loader(f[group_name])
    raise ValueError(f"No recognized content in {path}")
