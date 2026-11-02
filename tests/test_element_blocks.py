"""Element groups: which region of a mesh each element belongs to.

Exodus has always had them — a mesh is written as named element groups, and the
element group is where the part identity lives — and until this they were read
and thrown away, so a file that went `wing`/`tail` in came back one
undifferentiated soup. That is a lossy round trip in a package whose rule
is that anything it reads, it writes.

They earn their place beyond the round trip: `fem.Model.from_geometry`
reads the section for a member off the element group of the element it came from,
so a geometry is a complete description of a structure and a saved file
rebuilds it with nothing passed alongside. `tests/test_demo_drone.py`
pins that end of it.
"""

from __future__ import annotations

import pytest

from hertzforge.core.geometry import Geometry
from hertzforge.io import export_file, import_file, load, save


@pytest.fixture
def two_blocks():
    """Four quads over six nodes, split into a named pair of element groups."""
    return Geometry(
        node_id=[1, 2, 3, 4, 5, 6],
        node_xyz=[[0, 0, 0], [1, 0, 0], [2, 0, 0],
                  [0, 1, 0], [1, 1, 0], [2, 1, 0]],
        elem_id=[10, 11],
        elem_conn=[[1, 2, 5, 4], [2, 3, 6, 5]],
        elem_type=[44, 44],
        elem_group=[7, 9],
        group_id=[7, 9],
        group_name=['wing', 'tail'],
        length_unit='m')


def test_an_element_says_which_block_it_is_in(two_blocks):
    assert two_blocks.group_of(10) == 'wing'
    assert two_blocks.group_of(11) == 'tail'
    assert two_blocks.elements_in('wing') == [10]
    assert two_blocks.elements_in(9) == [11], 'by id as well as by name'


def test_a_mesh_with_no_blocks_is_one_block(two_blocks):
    """Most formats do not record the question, and nothing may invent an
    answer: everything lands in one element group with no name."""
    plain = Geometry(node_id=[1, 2, 3],
                     node_xyz=[[0, 0, 0], [1, 0, 0], [0, 1, 0]],
                     elem_conn=[[1, 2, 3]], elem_type=[41])
    assert list(plain.group_id) == [1]
    assert plain.group_name == ['']
    assert plain.elements_in(1) == list(plain.elem_id)


def test_a_block_no_element_belongs_to_is_refused(two_blocks):
    """An element in element group 3 with no element group 3 declared is a file saying
    two things that cannot both be true, and it is refused where it is
    written rather than found later by whatever reads it."""
    with pytest.raises(ValueError, match='element groups the geometry does not have'):
        Geometry(node_id=[1, 2, 3],
                 node_xyz=[[0, 0, 0], [1, 0, 0], [0, 1, 0]],
                 elem_conn=[[1, 2, 3]], elem_type=[41],
                 elem_group=[3], group_id=[1], group_name=[''])


def test_a_new_element_joins_a_block(two_blocks):
    two_blocks.add_element([1, 2, 5, 4], group=9)
    assert two_blocks.group_of(two_blocks.elem_id[-1]) == 'tail'
    two_blocks.add_element([2, 3, 5], group=12)
    assert 12 in two_blocks.group_id.tolist(), 'a new element group is declared'
    assert two_blocks.group_of(two_blocks.elem_id[-1]) == ''
    # one element group, one family (2026-09-30): a triangle is refused an element group
    # of quads by name, and with no element group named finds its own family
    import pytest

    with pytest.raises(ValueError, match='element group 9 holds quads'):
        two_blocks.add_element([1, 2, 4], group=9)
    two_blocks.add_element([1, 2, 4])
    assert int(two_blocks.elem_group[-1]) == 12, 'the triangles already there'
    two_blocks.add_element([1, 2])
    assert int(two_blocks.elem_group[-1]) not in (7, 9, 12), 'a beam element group of its own'


def test_deleting_an_element_takes_its_block_entry_with_it(two_blocks):
    two_blocks.delete_elements([10])
    assert len(two_blocks.elem_group) == len(two_blocks.elem_id) == 1
    assert two_blocks.group_of(11) == 'tail'


def test_blocks_ride_the_native_file(two_blocks, tmp_path):
    path = tmp_path / 'element groups.vdyn'
    save(two_blocks, path)
    back = load(path)
    assert list(back.elem_group) == [7, 9]
    assert list(back.group_id) == [7, 9]
    assert back.group_name == ['wing', 'tail']
    assert back == two_blocks


def test_blocks_survive_an_exodus_round_trip(two_blocks, tmp_path):
    """The format they come from, and the reason for the whole feature."""
    path = tmp_path / 'element groups.exo'
    export_file(two_blocks, path)
    back = import_file(path)
    assert back.group_name == ['wing', 'tail']
    assert list(back.group_id) == [7, 9]
    assert back.group_of(back.elem_id[0]) == 'wing'
    assert back.group_of(back.elem_id[1]) == 'tail'


def test_blocks_are_a_view_like_the_other_four(two_blocks):
    """Same shape as `nodes` and friends: plural on the view, singular on
    a row, and a row written through writes the geometry."""
    groups = two_blocks.groups
    assert len(groups) == 2
    assert list(groups.ids) == [7, 9]
    assert groups.names == ['wing', 'tail']
    assert groups[1].id == 9 and groups[1].name == 'tail'
    groups[1].name = 'empennage'
    assert two_blocks.group_name == ['wing', 'empennage'], 'not a copy'


def test_a_block_can_be_added_empty(two_blocks):
    """It has to exist before an element can be put in it, and exodus
    files carry empty ones anyway."""
    group_id = two_blocks.groups.add('fin')
    assert group_id == 10 and two_blocks.elements_in(10) == []
    two_blocks.validate()
    with pytest.raises(ValueError, match='already exists'):
        two_blocks.add_group('again', group_id=10)


def test_renumbering_a_block_carries_its_elements(two_blocks):
    """An element names its element group by id, so leaving them behind would put
    them in an element group that is not there — which `validate` refuses."""
    two_blocks.renumber_group(0, 4)
    assert list(two_blocks.elem_group) == [4, 9]
    two_blocks.validate()
    with pytest.raises(ValueError, match='already exists'):
        two_blocks.renumber_group(0, 9)


def test_deleting_a_block_deletes_its_elements_and_its_own_nodes(two_blocks):
    """An element group deleted is the part deleted (Brandon, 2026-09-27): its
    elements, and the nodes no other element group's element uses. The nodes it
    shares with the tail stay, so the tail is not cut into."""
    report = two_blocks.delete_groups([7])
    assert report == {'groups': 1, 'elements': 1, 'nodes': 2}
    assert list(two_blocks.group_id) == [9]
    assert two_blocks.elements_in(9) == [11], 'the tail, whole'
    assert list(two_blocks.node_id) == [2, 3, 5, 6], 'the shared edge kept'
    two_blocks.validate()


def test_deleting_every_block_leaves_no_mesh(two_blocks):
    two_blocks.delete_groups([7, 9])
    assert list(two_blocks.group_id) == [] and two_blocks.elem_conn == []
    assert two_blocks.num_nodes == 0


def test_blocks_of_one_kind_merge_into_the_first(two_blocks):
    """Merge Element Groups: the first keeps its id and name, the rest's elements
    move into it."""
    report = two_blocks.merge_groups([9, 7])
    assert report == {'into': 9, 'groups': 1, 'elements': 1}
    assert list(two_blocks.group_id) == [9]
    assert two_blocks.group_name == ['tail']
    assert two_blocks.elements_in(9) == [10, 11]
    two_blocks.validate()


def test_blocks_that_differ_are_not_merged(two_blocks, mixed_block):
    """Different element types, or different properties, are different
    parts: one material and thickness for both would change the model."""
    from hertzforge import fem

    aluminum = fem.material('6061-T6')
    two_blocks.group_properties = {7: fem.GroupProperties(aluminum, 0.01),
                                   9: fem.GroupProperties(aluminum, 0.02)}
    assert 'thickness' in two_blocks.merge_refusal([7, 9])
    with pytest.raises(ValueError, match='differ in material'):
        two_blocks.merge_groups([7, 9])
    two_blocks.group_properties[9] = fem.GroupProperties(aluminum, 0.01)
    assert two_blocks.merge_refusal([7, 9]) is None
    tri = two_blocks.add_group('fin')
    two_blocks.add_element([3, 6, 5], group=tri)
    assert two_blocks.merge_refusal([7, tri]) == \
        'the element groups hold different element types'
    assert two_blocks.merge_refusal([7]) == 'select two element groups or more to merge'


@pytest.fixture
def mixed_block():
    """One named element group holding a quad and a triangle — what the drone's
    'canopy' is, and what exodus cannot write as one element group."""
    return Geometry(
        node_id=[1, 2, 3, 4, 5],
        node_xyz=[[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [2, 0.5, 0]],
        elem_id=[10, 11],
        elem_conn=[[1, 2, 3, 4], [2, 5, 3]],
        elem_type=[44, 41],
        elem_group=[7, 7],
        group_id=[7],
        group_name=['canopy'],
        length_unit='m')


def test_a_block_of_two_element_types_splits_into_distinct_blocks(
        mixed_block, tmp_path):
    """Exodus holds one element type per element group, so an element group with a quad and
    a tri in it goes out as two — and they may share neither the id nor
    the name it declared. The drone went out as 34 element groups under 22 ids and
    read back nowhere: this package refuses the duplicate ids outright,
    and ParaView's IOSS reader fails the file at REQUEST_INFORMATION
    without saying why."""
    import netCDF4

    path = tmp_path / 'mixed.exo'
    export_file(mixed_block, path)
    with netCDF4.Dataset(path) as ds:
        ids = [int(i) for i in ds.variables['eb_prop1'][:]]
    assert len(set(ids)) == len(ids) == 2, 'two element groups, two ids'
    assert 7 in ids, 'the declared id is kept where it can be'

    back = import_file(path)
    assert back.group_name == ['canopy', 'canopy triangles'], (
        'the second piece says which type it is, since names must differ')
    assert len(back.elem_id) == 2
    assert back.group_of(back.elem_id[0]) == 'canopy'


def _category(item, label):
    return next(item.child(i) for i in range(item.childCount())
                if item.child(i).text(0).startswith(label))


def _refusal(model):
    """A refused edit says why on `edit_rejected` — the cell reverts,
    and the window puts the reason in the status bar."""
    said = []
    model.edit_rejected.connect(said.append)
    return said
