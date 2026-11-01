"""The .hforge file: every object survives it, it uses its own names, and
every .vdyn file Visual Dynamics wrote opens and moves across.

`tests/data/vdyn_corpus/` is Visual Dynamics' frozen corpus, one file per
.vdyn schema, copied as Visual Dynamics froze it. Those files are never
regenerated here: the day one fails to open, a change has stranded every
project saved in that era.
"""

from pathlib import Path

import h5py
import numpy as np
import pytest
from visualdynamics import Project
from visualdynamics.io import native

from hertzforge.io import hforge

CORPUS = Path(__file__).parent / 'data' / 'vdyn_corpus'
ERAS = sorted(p.name for p in CORPUS.glob('*.vdyn'))


def _contents(path):
    """Every attribute and dataset in an HDF5 file, by path, with the
    schema stamp left out: what a file says, independent of how h5py
    orders it."""
    out = {}

    def note(name, item):
        for key, value in item.attrs.items():
            out[f'{name}@{key}'] = value
        if isinstance(item, h5py.Dataset):
            out[name] = item[()]

    with h5py.File(path, 'r') as f:
        for key, value in f.attrs.items():
            if key != hforge.VDYN_STAMP:
                out[f'@{key}'] = value
        f.visititems(note)
    return out


def _same(a, b):
    if isinstance(a, np.ndarray) or isinstance(b, np.ndarray):
        a, b = np.asarray(a), np.asarray(b)
        if a.dtype.kind in 'fc':
            return a.shape == b.shape and np.array_equal(a, b, equal_nan=True)
        return a.shape == b.shape and bool(np.all(a == b))
    return a == b


def _differences(first, second):
    a, b = _contents(first), _contents(second)
    missing = sorted(set(a) ^ set(b))
    changed = sorted(k for k in set(a) & set(b) if not _same(a[k], b[k]))
    return missing, changed


def test_the_corpus_has_every_vdyn_era():
    assert ERAS == [f'schema{n}.vdyn' for n in range(1, native.SCHEMA_VERSION + 1)]


@pytest.mark.parametrize('era', ERAS)
def test_a_vdyn_project_opens(era):
    project = hforge.load(CORPUS / era)
    assert isinstance(project, Project)
    assert len(project) > 0


@pytest.mark.parametrize('era', ERAS)
def test_a_vdyn_project_moves_across_unchanged(era, tmp_path):
    """Opened from .vdyn, saved as .hforge and opened again, the project
    is the one Visual Dynamics read: written back out by Visual Dynamics'
    own writer, the two files hold the same values under the same names."""
    original = hforge.load(CORPUS / era)
    moved = hforge.load(hforge.save(original, tmp_path / 'moved'))
    native.save_test(tmp_path / 'before.vdyn', original.name, dict(original),
                     original.active_geometry, original.project_type,
                     original.object_groups, original.provenance,
                     original.data_index.records())
    native.save_test(tmp_path / 'after.vdyn', moved.name, dict(moved),
                     moved.active_geometry, moved.project_type,
                     moved.object_groups, moved.provenance,
                     moved.data_index.records())
    missing, changed = _differences(tmp_path / 'before.vdyn',
                                    tmp_path / 'after.vdyn')
    assert (missing, changed) == ([], [])
    assert list(moved) == list(original)


def test_a_file_uses_hertzforge_names(tmp_path):
    project = hforge.load(CORPUS / 'schema1.vdyn')
    path = hforge.save(project, tmp_path / 'named')
    with h5py.File(path, 'r') as f:
        assert f.attrs[hforge.STAMP] == hforge.SCHEMA_VERSION
        assert hforge.VDYN_STAMP not in f.attrs
        assert 'object_groups' in f.attrs and 'links' not in f.attrs
        geometry = next(f['objects'][k] for k in f['objects']
                        if f['objects'][k].attrs['kind'] == 'geometry')
        held = set(geometry)
    assert {'elem_group', 'group_id', 'group_name'} <= held
    assert not held & {'elem_block', 'block_id', 'block_name'}


def test_element_group_properties_survive_under_their_name(tmp_path):
    from visualdynamics.core.fem import GroupProperties
    from visualdynamics.demo import plate

    geometry = plate.build(4).geometry()
    group = int(geometry.group_id[0])
    geometry.group_properties = {
        group: GroupProperties(plate.ALUMINUM, thickness=plate.THICKNESS)}
    path = hforge.save(geometry, tmp_path / 'plate')
    with h5py.File(path, 'r') as f:
        assert list(f['geometry/group_properties']) == [str(group)]
        assert 'block_properties' not in f['geometry']
    read = hforge.load(path).group_properties[group]
    assert read.material.youngs_modulus == plate.ALUMINUM.youngs_modulus
    assert read.thickness == plate.THICKNESS


def test_one_object_saves_and_opens_alone(tmp_path):
    shapes = hforge.load(CORPUS / 'schema1.vdyn')['Shapes']
    path = hforge.save(shapes, tmp_path / 'shapes.hforge')
    assert path == str(tmp_path / 'shapes.hforge')
    read = hforge.load(path)
    assert read.frequency.tolist() == shapes.frequency.tolist()
    assert np.array_equal(read.shape_matrix, shapes.shape_matrix)


def test_the_suffix_is_added_once(tmp_path):
    project = hforge.load(CORPUS / 'schema1.vdyn')
    assert hforge.save(project, tmp_path / 'a') == str(tmp_path / 'a.hforge')
    assert hforge.save(project, tmp_path / 'b.hforge') == \
        str(tmp_path / 'b.hforge')
    assert project.file == str(tmp_path / 'b.hforge')


def test_a_project_remembers_where_it_was_opened_from(tmp_path):
    path = hforge.save(hforge.load(CORPUS / 'schema1.vdyn'), tmp_path / 'p')
    assert hforge.load(path).file == path


def test_a_newer_schema_is_refused(tmp_path):
    path = hforge.save(hforge.load(CORPUS / 'schema1.vdyn'), tmp_path / 'new')
    with h5py.File(path, 'a') as f:
        f.attrs[hforge.STAMP] = hforge.SCHEMA_VERSION + 1
    with pytest.raises(ValueError, match='newer Hertzforge'):
        hforge.load(path)


def test_a_file_with_no_stamp_is_refused(tmp_path):
    path = tmp_path / 'plain.h5'
    with h5py.File(path, 'w') as f:
        f.create_dataset('x', data=[1.0])
    with pytest.raises(ValueError, match='no schema stamp'):
        hforge.load(path)


def test_an_unknown_kind_is_refused_by_name(tmp_path):
    path = hforge.save(hforge.load(CORPUS / 'schema1.vdyn'), tmp_path / 'k')
    with h5py.File(path, 'a') as f:
        f['objects/0000'].attrs['kind'] = 'hologram'
    with pytest.raises(ValueError, match="'hologram'"):
        hforge.load(path)


def test_an_object_it_cannot_hold_leaves_no_file(tmp_path):
    project = hforge.load(CORPUS / 'schema1.vdyn')
    project['Note'] = object()
    with pytest.raises(TypeError, match='cannot hold a object'):
        hforge.save(project, tmp_path / 'broken')
    assert not (tmp_path / 'broken.hforge').exists()


def test_every_kind_visual_dynamics_saves_is_held():
    """A kind Visual Dynamics' native format gains must be added here
    too, or a project holding one could be opened but not saved."""
    assert {kind for _c, kind, _w, _r in hforge._kinds()} == \
        set(native._LOADERS)
