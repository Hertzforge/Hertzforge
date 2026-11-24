"""Frozen .hforge files open forever, and every .vdyn moves across.

The files under `testdata/hforge_corpus/` are written by
`testdata/generate_hforge_corpus.py`, one per schema, and pinned here.
Whatever the format grows into, they must keep opening with their
values intact. A frozen file is written once and never refreshed: the
moment one fails, a change has orphaned every project saved before it.

An older project moves to Hertzforge by opening its `.vdyn`
and saving it, so each file in `testdata/vdyn_corpus/` is saved as
`.hforge` here and opened again, and the last `.vdyn` schema is compared
with its `.hforge` tree name by name.
"""

import glob
import os
import shutil

import h5py
import numpy as np
import pytest
from conftest import fixture_path

from hertzforge import Project
from hertzforge.io import native

CORPUS = str(fixture_path('hforge_corpus'))
VDYN_CORPUS = str(fixture_path('vdyn_corpus'))


def file_tree(path):
    """{path: value} for every attribute (`path@name`) and dataset in an
    HDF5 file."""
    out = {}
    with h5py.File(path, 'r') as f:
        def visit(name, obj):
            for key, value in obj.attrs.items():
                out[f'{name}@{key}'] = value
            if isinstance(obj, h5py.Dataset):
                out[name] = obj[()]
        f.visititems(visit)
        for key, value in f.attrs.items():
            out[f'@{key}'] = value
    return out


def same(a, b):
    """Equal values, NaN equal to NaN, strings compared as text."""
    a, b = np.asarray(a), np.asarray(b)
    if a.shape != b.shape:
        return False
    if a.dtype.kind in 'OSU' or b.dtype.kind in 'OSU':
        def text(x):
            return x.decode() if isinstance(x, bytes) else str(x)
        return [text(x) for x in a.ravel()] == [text(x) for x in b.ravel()]
    return a.dtype == b.dtype and np.array_equal(a, b, equal_nan=a.dtype.kind in 'fc')


def as_hforge(key):
    """A `.vdyn` tree path under the names `.hforge` gives it."""
    renames = {old: new for new, old in native.VDYN_NAMES.items()}
    renames['links'] = 'object_groups'
    renames[native.VDYN_STAMP] = native.STAMP
    head, at, attribute = key.partition('@')
    parts = [renames.get(part, part) for part in head.split('/')]
    return '/'.join(parts) + at + renames.get(attribute, attribute)


@pytest.mark.parametrize('name', sorted(
    os.path.basename(p) for p in glob.glob(CORPUS + '/*.hforge')))
def test_every_frozen_schema_still_opens(name):
    project = native.load(os.path.join(CORPUS, name))
    assert isinstance(project, Project)
    assert len(project) > 0


def test_schema1_values_are_intact():
    """The schema 1 file holds the project the last `.vdyn` corpus file
    holds, plus element-group properties, so every value pinned there
    is checked here by comparing the two."""
    held = native.load(os.path.join(CORPUS, 'schema1.hforge'))
    vdyn = native.load(os.path.join(VDYN_CORPUS, 'schema3.vdyn'))
    assert list(held) == list(vdyn)
    assert (held.name, held.project_type, held.active_geometry) == (
        vdyn.name, vdyn.project_type, vdyn.active_geometry)
    assert held.object_groups == vdyn.object_groups
    assert held.provenance == vdyn.provenance
    for name in ('Geometry', 'Time History', 'FRF', 'Shapes', 'Channel Table',
                 'Sine Specification', 'Sine Levels', 'Time History PSDs',
                 'Time History SRS'):
        assert held[name] == vdyn[name], name
    assert held['Report'].blocks == vdyn['Report'].blocks
    assert held['Matches'].pairs == vdyn['Matches'].pairs
    assert held['Photos'].images == vdyn['Photos'].images
    assert ([p.as_record() for p in held['Predictions']]
            == [p.as_record() for p in vdyn['Predictions']])
    assert held['Run Log, Test 1'].text == vdyn['Run Log, Test 1'].text

    geometry = held['Geometry']
    (properties,) = geometry.group_properties.values()
    assert properties.material.name == '6061-T6'
    assert properties.material.youngs_modulus == 68.9e9
    assert properties.section.name == 'tube'
    assert properties.section.area == 1.2e-4
    assert geometry.group_name == vdyn['Geometry'].group_name


def test_schema3_holds_a_converted_run():
    """The schema 3 file is the schema 1 project plus a converted control
    run: its channel table, streams, specification, results and the run
    record, grouped as the conversion grouped them."""
    from hertzforge.core.run_record import RunRecord

    held = native.load(os.path.join(CORPUS, 'schema3.hforge'))
    plain = native.load(os.path.join(CORPUS, 'schema1.hforge'))
    with h5py.File(os.path.join(CORPUS, 'schema3.hforge')) as f:
        assert f.attrs[native.STAMP] == native.RUN_RECORD_SCHEMA
    assert list(held)[:len(plain)] == list(plain)
    for name in ('Geometry', 'Time History', 'FRF', 'Shapes', 'Channel Table'):
        assert held[name] == plain[name], name
    record = held['Run Record']
    assert isinstance(record, RunRecord)
    assert (record.test, record.outcome, record.source) == ('random', 'completed', 'SIM')
    assert [entry['kind'] for entry in record.log] == [
        'state', 'station', 'level', 'alarm', 'level', 'state']
    assert record.law_file[0] == 'diagonal.py'
    inputs = held['inputs']
    assert inputs.ordinate.shape == (2, 256) and inputs.source == 'SIM'
    assert inputs.response_dof == ['101Z+', '2']
    assert held['Channel Table (2)'].has_controller_settings
    assert held['Specification'].limits['abort_upper'].shape == (1, 4)
    (group,) = [g for g in held.object_groups if g.get('name') == 'golden_random']
    assert 'Run Record' in group['members'] and 'Channel Table (2)' in group['members']


@pytest.mark.parametrize('name', sorted(
    os.path.basename(p) for p in glob.glob(VDYN_CORPUS + '/*.vdyn')))
def test_a_vdyn_project_moves_across_unchanged(tmp_path, name):
    """Opened and saved, a `.vdyn` from any schema becomes a `.hforge`
    that reopens to the same project: saved again, it writes the same
    tree, attribute by attribute and dataset by dataset."""
    moved = Project.open(os.path.join(VDYN_CORPUS, name)).save(tmp_path / 'moved')
    again = Project.open(moved).save(tmp_path / 'again.hforge')
    first, second = file_tree(moved), file_tree(again)
    assert first['@' + native.STAMP] == native.BASE_SCHEMA
    assert set(first) == set(second), set(first) ^ set(second)
    assert not [key for key in first if not same(first[key], second[key])]


def test_the_last_vdyn_schema_differs_only_by_the_renames(tmp_path):
    """`.vdyn` schema 3 is `.hforge` schema 1 under other names, so the
    two trees match once the names are mapped. The older schemas are
    upgraded as they are read (tracelines become element groups, a
    channel table's `range` becomes two columns), which the test above
    covers. A run log extract stores its log's path relative to the
    project file, so that one attribute follows where the file is saved."""
    source = os.path.join(VDYN_CORPUS, 'schema3.vdyn')
    moved = Project.open(source).save(tmp_path / 'moved')
    before = {as_hforge(key): value for key, value in file_tree(source).items()}
    after = file_tree(moved)
    assert set(before) == set(after), set(before) ^ set(after)
    differing = [key for key in after if not same(before[key], after[key])]
    assert sorted(differing) == ['@' + native.STAMP, 'objects/0011@path'], differing


def test_saving_never_writes_a_vdyn(tmp_path):
    project = native.load(os.path.join(VDYN_CORPUS, 'schema1.vdyn'))
    with pytest.raises(ValueError, match=r'saves \.hforge files'):
        project.save(tmp_path / 'back.vdyn')
    with pytest.raises(ValueError, match=r'saves \.hforge files'):
        native.save(project['Geometry'], tmp_path / 'geometry.vdyn')
    assert not list(tmp_path.iterdir())
    assert project.save(tmp_path / 'plain') == str(tmp_path / 'plain.hforge')


def test_a_newer_schema_is_refused_not_half_loaded(tmp_path):
    newer = tmp_path / 'from_the_future.hforge'
    shutil.copy(os.path.join(CORPUS, 'schema1.hforge'), newer)
    with h5py.File(newer, 'r+') as f:
        f.attrs[native.STAMP] = native.SCHEMA_VERSION + 1
    with pytest.raises(ValueError, match=(
            rf'newer Hertzforge \(schema {native.SCHEMA_VERSION + 1}; this build '
            rf'reads up to {native.SCHEMA_VERSION}\)')):
        native.load(newer)


def test_the_stamp_decides_not_the_suffix(tmp_path):
    """A `.vdyn` renamed `.hforge` still reads by its own stamp and its
    own names, and a file with neither stamp is refused by name."""
    renamed = tmp_path / 'renamed.hforge'
    shutil.copy(os.path.join(VDYN_CORPUS, 'schema1.vdyn'), renamed)
    assert native.load(renamed)['Geometry'] == native.load(
        os.path.join(VDYN_CORPUS, 'schema1.vdyn'))['Geometry']
    with h5py.File(renamed, 'r+') as f:
        del f.attrs[native.VDYN_STAMP]
    with pytest.raises(ValueError, match=r'not a \.hforge or \.vdyn file'):
        native.load(renamed)


def test_an_older_escdf_file_keeps_its_whole_objects(tmp_path):
    """An older file stored each object whole in an ESCDF attachment named
    `escdf_objects.VDYN_WHOLE`, in the `.vdyn` layout. Read from the
    standard's fields alone, a geometry loses its element-group
    properties, so finding that attachment is what keeps them."""
    import io

    from hertzforge.io import escdf, escdf_objects

    geometry = native.load(os.path.join(CORPUS, 'schema1.hforge'))['Geometry']
    project = Project('Older')
    project.add('Geometry', geometry)
    path = tmp_path / 'older.h5'
    project.save(path)

    file = escdf.read(path)
    (dataset,) = [d for d in file.metadata.values()
                  if escdf_objects.WHOLE in list(d.values.get('attachment_names', []))]
    names = list(dataset.values['attachment_names'])
    k = names.index(escdf_objects.WHOLE)
    buffer = io.BytesIO(bytes(np.asarray(dataset.values['attachments'][k], dtype=np.uint8)))
    with h5py.File(buffer, 'r+') as blob:
        del blob.attrs[native.STAMP]
        blob.attrs[native.VDYN_STAMP] = native.VDYN_SCHEMA
        for new, old in native.VDYN_NAMES.items():
            blob.move(f'geometry/{new}', f'geometry/{old}')
    names[k] = escdf_objects.VDYN_WHOLE
    dataset.values['attachment_names'] = np.array(names, dtype=object)
    dataset.values['attachments'][k] = np.frombuffer(buffer.getvalue(), dtype=np.uint8)
    escdf.write(file, path)

    back = Project.open(path)['Geometry']
    assert back.group_properties.keys() == geometry.group_properties.keys()
    assert back == geometry


def test_a_compressed_save_holds_the_same_project(tmp_path):
    """gzip on request: the large arrays are chunked along time and
    compressed, the small ones are left as they are, and the file reads
    back to the same tree as an uncompressed save. A plain save
    compresses nothing."""
    from hertzforge.core.data import TimeHistory

    t = np.arange(65536) / 8192.0
    quiet = np.where(t < 1.0, np.sin(2 * np.pi * 40 * t) * np.exp(-8 * t), 0.0)
    project = native.load(os.path.join(CORPUS, 'schema1.hforge'))
    project.add('Hits', TimeHistory(t, [quiet, 0.5 * quiet], response_dof=['1X+', '2Y+']))
    plain = project.save(tmp_path / 'plain')
    packed = project.save(tmp_path / 'packed', compress=True)

    with h5py.File(plain) as f:
        filters = []
        f.visititems(lambda name, obj: filters.append(name) if isinstance(
            obj, h5py.Dataset) and obj.compression else None)
        assert filters == []
    with h5py.File(packed) as f:
        hits = next(f['objects'][k] for k in f['objects']
                    if f['objects'][k].attrs['name'] == 'Hits')
        ordinate = hits['ordinate']
        assert (ordinate.compression, ordinate.shuffle) == ('gzip', True)
        assert ordinate.chunks == (1, 65536)
        assert hits['abscissa'].compression == 'gzip'
        assert hits['response_dof'].compression is None
        geometry = f['objects/0000/node_xyz']
        assert geometry.nbytes < native.COMPRESS_FROM_BYTES
        assert geometry.compression is None
    assert os.path.getsize(packed) < 0.7 * os.path.getsize(plain)
    a, b = file_tree(plain), file_tree(packed)
    assert set(a) == set(b)
    assert not [key for key in a if not same(a[key], b[key])]


def test_compress_is_refused_where_it_does_nothing(tmp_path):
    project = native.load(os.path.join(CORPUS, 'schema1.hforge'))
    for name in ('p.mat', 'p.h5'):
        with pytest.raises(ValueError, match='compress applies to a .hforge file'):
            project.save(tmp_path / name, compress=True)
    assert not list(tmp_path.iterdir())


def test_schema4_holds_test_setups():
    """The schema 4 file is the schema 3 project plus two test setups:
    one typed, and one taken from the converted run's record, which
    holds that run's configuration exactly."""
    from hertzforge.core.test_setup import TestSetup

    held = native.load(os.path.join(CORPUS, 'schema4.hforge'))
    older = native.load(os.path.join(CORPUS, 'schema3.hforge'))
    with h5py.File(os.path.join(CORPUS, 'schema4.hforge')) as f:
        assert f.attrs[native.STAMP] == native.TEST_SETUP_SCHEMA
    assert list(held)[:len(older)] == list(older)
    typed, taken = held['Recorder Setup'], held['Random Setup']
    assert isinstance(typed, TestSetup) and isinstance(taken, TestSetup)
    assert (typed.module, typed.origin, typed.table) == ('recorder', {'kind': 'typed'}, {})
    assert typed.notes == 'typed on the Recorder screen'
    record = held['Run Record']
    assert taken.module == 'random' and taken.config == record.config
    assert taken.origin == {'kind': 'run', 'run_uid': record.run_uid,
                            'run_id': record.run_id, 'name': 'Run Record'}
    assert taken.table['rows'] == record.table['rows']


def test_schema5_scans_open_as_geometries():
    """The schema 5 file is the schema 4 project plus two `scan/` groups,
    which nothing writes any more. They open as geometries: the probed
    points as nodes in meters with a feature an element group, and the
    bare cloud as nodes with no unit."""
    from hertzforge.core.geometry import Geometry

    held = native.load(os.path.join(CORPUS, 'schema5.hforge'))
    older = native.load(os.path.join(CORPUS, 'schema4.hforge'))
    with h5py.File(os.path.join(CORPUS, 'schema5.hforge')) as f:
        assert f.attrs[native.STAMP] == native.SCAN_SCHEMA
    assert list(held)[:len(older)] == list(older)
    probed, cloud = held['Probed Scan'], held['Cloud Scan']
    assert isinstance(probed, Geometry) and isinstance(cloud, Geometry)
    assert probed.length_unit == 'mm' and list(probed.group_name) == ['31_1', '31_2']
    assert probed.node_id.tolist() == [1, 2, 3, 4, 5, 6, 7]
    assert [int(c[0]) for c in probed.elem_conn] == [1, 2, 3, 4, 5, 6]
    assert probed.elem_group.tolist() == [1, 1, 1, 2, 2, 2]
    assert set(probed.elem_type.tolist()) == {203}
    np.testing.assert_allclose(probed.node_xyz[2], [0.010, 0.002, 0.009])
    assert cloud.length_unit is None and not len(cloud.elem_conn) and cloud.num_nodes == 8
    np.testing.assert_allclose(cloud.node_xyz[1], [3.0, 2.0, 1.25])


def test_a_project_saved_now_holds_measured_points_as_a_geometry(tmp_path):
    """Measured points are a geometry, of an element type a build from
    before them does not know, so the file is stamped for them and that
    build says to update."""
    from hertzforge.io.point_files import points_geometry

    project = Project()
    project.add('Probed', points_geometry([[1.0, 2.0, 3.0]], 'mm', ['a'], [0]))
    path = tmp_path / 'probed.hforge'
    project.save(path)
    with h5py.File(path) as f:
        assert f.attrs[native.STAMP] == native.MEASURED_POINT_SCHEMA
        assert 'scan' not in f
    np.testing.assert_allclose(Project.open(path)['Probed'].node_xyz, [[0.001, 0.002, 0.003]])


def test_schema6_holds_an_element_shape_set_and_measured_points():
    """The schema 6 file is the schema 4 project plus measured points and a
    free brick's two lowest elastic modes read as strain and stress. The
    set reads back whole: nine stress points, the centroid weighing
    nothing, and each mode's strain energy half its omega squared."""
    from hertzforge.core.element_shapes import ElementShapeSet
    from hertzforge.core.geometry import Geometry

    held = native.load(os.path.join(CORPUS, 'schema6.hforge'))
    older = native.load(os.path.join(CORPUS, 'schema4.hforge'))
    with h5py.File(os.path.join(CORPUS, 'schema6.hforge')) as f:
        assert f.attrs[native.STAMP] == native.ELEMENT_SHAPES_SCHEMA
    assert list(held)[:len(older)] == list(older)
    assert isinstance(held['Probed Points'], Geometry)
    read = held['Brick Modes Strain and Stress']
    assert isinstance(read, ElementShapeSet)
    assert read.strain.shape == read.stress.shape == (2, 9, 6)
    assert read.element.tolist() == [1] * 9 and read.point.tolist() == list(range(9))
    assert read.weight[0] == 0.0 and read.weight.sum() == pytest.approx(2e-6, rel=1e-12)
    assert read.mass_unit == 'kg' and read.length_unit == 'm'
    np.testing.assert_allclose(read.frequency, [85382.26462276, 135195.50996603], rtol=1e-9)
    np.testing.assert_allclose(read.strain_energy(), (2 * np.pi * read.frequency) ** 2 / 2,
                               rtol=1e-9)


def test_schema7_holds_a_time_comparison():
    """The schema 7 file is the schema 6 project plus a time comparison:
    a model half the test's size, one station reversed, scored over 0.1
    to 0.4 s on the test's 1 kHz clock."""
    from hertzforge.core.data import TimeComparison
    from hertzforge.core.time_compare import scores

    held = native.load(os.path.join(CORPUS, 'schema7.hforge'))
    older = native.load(os.path.join(CORPUS, 'schema6.hforge'))
    with h5py.File(os.path.join(CORPUS, 'schema7.hforge')) as f:
        assert f.attrs[native.STAMP] == native.TIME_COMPARISON_SCHEMA
    assert list(held)[:len(older)] == list(older)
    read = held['Time Response Comparison']
    assert isinstance(read, TimeComparison)
    assert read.pairs == [['Test Responses', 0, 'Model Responses', 0],
                          ['Test Responses', 1, 'Model Responses', 1]]
    assert read.sources == ['Test Responses', 'Model Responses']
    assert (read.start, read.stop) == (0.1, 0.4)
    assert len(read.abscissa) == 500
    first, second = scores(read)
    assert first['strac'] == pytest.approx(2.0) and second['strac'] == pytest.approx(-2.0)
    assert first['samples'] == 301
