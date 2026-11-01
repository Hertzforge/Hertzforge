"""The `.hforge` file: Hertzforge's projects and data, in HDF5.

The layout is Visual Dynamics' `.vdyn` schema 3 under the names the code
uses today (`docs/hforge-format.md`). Each object is written and read by
Visual Dynamics' own writer and reader for its kind, so what an object
carries is decided in one place while Visual Dynamics is a dependency.
The file differs from `.vdyn` in three ways: the root stamp
(`hertzforge_schema`), the object groups' attribute (`object_groups`,
which `.vdyn` writes as `links`), and a geometry's element groups, which
`.vdyn` writes under their older names (`GEOMETRY_NAMES`).

`load` also opens every `.vdyn` file a Visual Dynamics release has
written, through Visual Dynamics' reader, so a project moves across by
opening it and saving it.
"""

from __future__ import annotations

import json
import os
from collections.abc import Mapping
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:                                    # pragma: no cover
    import h5py

SUFFIX = '.hforge'

#: the newest schema this build reads and the one it writes
SCHEMA_VERSION = 1

#: the root attribute every `.hforge` file is stamped with
STAMP = 'hertzforge_schema'

#: the stamp of a `.vdyn` file
VDYN_STAMP = 'visualdynamics_schema'

#: a geometry's fields that `.vdyn` writes under their older names:
#: {name Visual Dynamics' writer uses: name in a `.hforge` file}
GEOMETRY_NAMES = {'elem_block': 'elem_group',
                  'block_id': 'group_id',
                  'block_name': 'group_name',
                  'block_properties': 'group_properties'}


class _Renamed:
    """An HDF5 group seen through `names`, so Visual Dynamics' geometry
    writer and reader find each field under the name they use while the
    file holds it under the name in `GEOMETRY_NAMES`."""

    def __init__(self, group: h5py.Group, names: Mapping[str, str]) -> None:
        self._group = group
        self._names = names

    @property
    def attrs(self) -> Any:
        return self._group.attrs

    def __contains__(self, name: str) -> bool:
        return self._names.get(name, name) in self._group

    def __getitem__(self, name: str) -> Any:
        return self._group[self._names.get(name, name)]

    def create_dataset(self, name: str, **options: Any) -> Any:
        return self._group.create_dataset(self._names.get(name, name),
                                          **options)

    def create_group(self, name: str) -> Any:
        return self._group.create_group(self._names.get(name, name))


def _save_geometry(geometry: Any, group: h5py.Group) -> None:
    from visualdynamics.io import native

    native.save_geometry(geometry, _Renamed(group, GEOMETRY_NAMES))


def _load_geometry(group: h5py.Group) -> Any:
    from visualdynamics.io import native

    return native.load_geometry(_Renamed(group, GEOMETRY_NAMES))


def _kinds() -> list[tuple[type, str, Any, Any]]:
    """(class, kind, writer, reader) for every kind of object a file
    holds, most specific class first."""
    from visualdynamics.core.channel_table import ChannelTable
    from visualdynamics.core.data import DataArray
    from visualdynamics.core.geometry import Geometry
    from visualdynamics.core.matches import MatchedModes
    from visualdynamics.core.photos import Photos
    from visualdynamics.core.prediction import Predictions
    from visualdynamics.core.report import Report
    from visualdynamics.core.run_log_extract import RunLogExtract
    from visualdynamics.core.shapes import ShapeSet
    from visualdynamics.core.sine import SineLevelSet, SineSweepSpecification
    from visualdynamics.io import native as n

    return [
        (Geometry, 'geometry', _save_geometry, _load_geometry),
        (SineSweepSpecification, 'sine_specification',
         n.save_sine_specification, n.load_sine_specification),
        (SineLevelSet, 'sine_levels', n.save_sine_levels, n.load_sine_levels),
        (DataArray, 'data', n.save_data, n.load_data),
        (ShapeSet, 'shapes', n.save_shapes, n.load_shapes),
        (Report, 'report', n.save_report, n.load_report),
        (Photos, 'photos', n.save_photos, n.load_photos),
        (MatchedModes, 'matches', n.save_matches, n.load_matches),
        (ChannelTable, 'channel_table', n.save_channel_table,
         n.load_channel_table),
        (Predictions, 'predictions', n.save_predictions, n.load_predictions),
        (RunLogExtract, 'run_log_extract', n.save_run_log_extract,
         n.load_run_log_extract),
    ]


def _writer_for(obj: Any) -> tuple[str, Any]:
    for cls, kind, writer, _reader in _kinds():
        if isinstance(obj, cls):
            return kind, writer
    raise TypeError(f'A .hforge file cannot hold a {type(obj).__name__}')


def _reader_for(kind: str, path: str) -> Any:
    for _cls, known, _writer, reader in _kinds():
        if known == kind:
            return reader
    raise ValueError(f'{path} holds an object of kind {kind!r}, which this '
                     'build of Hertzforge does not know')


def hforge_path(path: str | os.PathLike) -> str:
    """`path` with `.hforge` added when it does not end in it."""
    path = str(path)
    return path if path.endswith(SUFFIX) else path + SUFFIX


def save(obj: Any, path: str | os.PathLike) -> str:
    """Write a project, or one object, to a `.hforge` file.

    Parameters
    ----------
    obj : visualdynamics.Project or object
        A whole project, or one object of a kind the format holds.
    path : str or os.PathLike
        Where; `.hforge` is added when the suffix is not there.

    Returns
    -------
    str
        The path written.

    Raises
    ------
    TypeError
        For an object of a kind the format does not hold.
    """
    import h5py
    from visualdynamics import Project

    written = hforge_path(path)
    if not isinstance(obj, Project):
        kind, writer = _writer_for(obj)
        with h5py.File(written, 'w') as f:
            f.attrs[STAMP] = SCHEMA_VERSION
            writer(obj, f.create_group(kind))
        return written
    # every object's writer is found before the file is opened, so an
    # object the format cannot hold leaves no half-written file behind
    writers = [(name, obj_, *_writer_for(obj_)) for name, obj_ in obj.items()]
    with h5py.File(written, 'w') as f:
        f.attrs[STAMP] = SCHEMA_VERSION
        f.attrs['test_name'] = obj.name
        f.attrs['active_geometry'] = obj.active_geometry or ''
        f.attrs['project_type'] = obj.project_type or ''
        f.attrs['object_groups'] = json.dumps(obj.object_groups or [])
        f.attrs['provenance'] = json.dumps(obj.provenance or {})
        records = obj.data_index.records()
        if records:
            # a dataset, not an attribute: an attached file's text rides
            # in it, and an HDF5 attribute holds 64 KiB at most
            f.create_dataset('data_index', data=json.dumps(records),
                             dtype=h5py.string_dtype('utf-8'))
        container = f.create_group('objects')
        for i, (name, obj_, kind, writer) in enumerate(writers):
            group = container.create_group(f'{i:04d}')
            group.attrs['name'] = name
            group.attrs['kind'] = kind
            writer(obj_, group)
    obj.file = os.path.abspath(os.path.expanduser(written))
    return written


def load(path: str | os.PathLike) -> Any:
    """Read a `.hforge` file, or a `.vdyn` file from Visual Dynamics.

    Parameters
    ----------
    path : str or os.PathLike
        The file. Which format it is comes from its stamp, not its
        suffix.

    Returns
    -------
    visualdynamics.Project or object
        The project, or the one object the file holds.

    Raises
    ------
    ValueError
        When the file carries neither stamp, was written by a newer
        Hertzforge, or holds nothing this reader recognizes.
    """
    import h5py

    path = str(path)
    with h5py.File(path, 'r') as f:
        if STAMP not in f.attrs:
            if VDYN_STAMP in f.attrs:
                from visualdynamics.io import native

                return native.load_from(f, path=path)
            raise ValueError(f'{path} is not a Hertzforge or Visual Dynamics '
                             'file (it has no schema stamp)')
        written = int(f.attrs[STAMP])
        if written > SCHEMA_VERSION:
            raise ValueError(
                f'{path} was written by a newer Hertzforge (schema '
                f'{written}; this build reads up to {SCHEMA_VERSION}). '
                'Update Hertzforge to open it.')
        if 'objects' in f:
            return _load_project(f, path)
        for _cls, kind, _writer, reader in _kinds():
            if kind in f:
                return reader(f[kind])
    raise ValueError(f'{path} holds nothing a .hforge file can hold')


def _load_project(f: h5py.File, path: str) -> Any:
    from visualdynamics import Project

    objects = {}
    for key in sorted(f['objects']):
        group = f['objects'][key]
        objects[group.attrs['name']] = _reader_for(group.attrs['kind'],
                                                   path)(group)
    indexed = []
    if 'data_index' in f:
        raw = f['data_index'][()]
        indexed = json.loads(raw.decode('utf-8') if isinstance(raw, bytes)
                             else raw)
    project = Project(f.attrs['test_name'], objects,
                      f.attrs['active_geometry'] or None,
                      f.attrs['project_type'] or None,
                      json.loads(f.attrs['object_groups']),
                      provenance=json.loads(f.attrs['provenance']),
                      data_index=indexed)
    project.file = os.path.abspath(path)
    return project
