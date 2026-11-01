"""The BARC, built from planes (Brandon, 2026-09-26;
`hertzforge.demo.barc`, from https://wiki.sem.org/wiki/BARC).

What is pinned: the geometry is the solid model's — its mass within a
percent of the solids' volumes, its parts where the drawing puts them,
one connected structure; the bolts are the patches the documentation
tells a person to pick; and the modes stay where they were when the
model was checked against the finite element models shared on the wiki
(2026-09-27: the check was kept, and the wiki's modes left out of the
project — Brandon).
"""

from __future__ import annotations

from collections import Counter

import numpy as np
import pytest

from hertzforge.core.fem import RIGID
from hertzforge.demo import barc

INCH = 0.0254

INCH3 = INCH ** 3


@pytest.fixture(scope='module')
def solved():
    model = barc.build()
    return model, model.eigensolution(maximum_frequency=2000.0)


def test_the_geometry_is_the_solid_model():
    geometry = barc.geometry()
    assert list(geometry.group_name) == ['box', 'right channel',
                                         'left channel', 'beam', 'bolts']
    bolts = int(geometry.group_id[list(geometry.group_name).index('bolts')])
    assert geometry.group_properties[bolts].material is RIGID
    box = int(geometry.group_id[0])
    assert geometry.group_properties[box].thickness == pytest.approx(0.25 * 0.0254)
    inch = geometry.node_xyz / 0.0254
    assert inch[:, 0].min() == pytest.approx(-2.875)            # mid-surface
    assert inch[:, 1].max() == pytest.approx(barc.BEAM_Y)
    # the slot: nothing on the top wall between -0.25 and 0.25
    top = np.abs(inch[:, 1] - barc.BOX_MID) < 1e-6
    assert not np.any(top & (np.abs(inch[:, 0]) < barc.SLOT_HALF - 1e-6))


def test_it_weighs_what_the_solid_model_does(solved):
    """The STEP's solids: box 16.83 in³, channels 0.453 in³ each, beam
    0.615 in³, of 6061-T6. The plates carry no bolt holes, and weigh
    half a percent more."""
    model, _shapes = solved
    solid = (16.8299 + 2 * 0.4527 + 0.6154) * INCH3 * barc.MATERIAL.density
    assert model.structural_mass == pytest.approx(solid, rel=0.01)
    assert len(model.pieces()) == 1


#: the first ten elastic modes of the model as built at `barc.SIZE`, Hz —
#: four bolts at each end of the beam since 2026-10-11, each tying the
#: element it passes through, within 0.2 % (MAC 0.999 and up) of the
#: build with one bolt there that was checked against the wiki's models
#: (194.5, 212.5, 265.3, 461.8, 494.9, 581.0, 593.5, 681.4, 1128.1,
#: 1170.9)
CHECKED = (194.4, 212.4, 264.8, 461.6, 494.4, 580.3, 593.4, 681.1,
           1125.6, 1168.8)


def test_the_modes_stay_where_they_were_checked(solved):
    """Free-free, and the first ten elastic modes within half a percent
    of the build that was checked against the wiki's models: a change to
    the planes, the joints or the solver that moves them is a change to
    look at, not a drift to accept."""
    _model, shapes = solved
    assert int(np.sum(shapes.frequency == 0.0)) == 6, 'free-free'
    elastic = [f for f in shapes.frequency if f > 1.0][:10]
    assert np.allclose(elastic, CHECKED, rtol=0.005), np.round(elastic, 1)


def test_each_bolt_ties_the_elements_it_clamps():
    """The rule the docs give a person to click: under a foot bolt the
    element it passes through and the eight around it, the washer's
    area, and through the beam the element it passes through alone (four
    bolts at each end since 2026-10-11) — each tied to the part below,
    every patch node that part does not share linked once."""
    from hertzforge import mesh

    planes = mesh.assemble(*barc._planes(barc.SIZE))
    patches = [barc.bolt_patch(planes, bolt) for bolt in barc.BOLTS]
    assert [len(patch) for patch, _below in patches] == [9] * 8 + [1] * 8
    assert [below for _patch, below in patches] == (
        ['box'] * 8 + ['left channel'] * 4 + ['right channel'] * 4)
    # the beam bolt's element is the one it passes through
    xyz = planes.node_xyz / 0.0254
    for (patch, _below), bolt in zip(patches[8:], barc.BOLTS[8:]):
        row = int(np.flatnonzero(planes.elem_id == patch[0])[0])
        corners = xyz[planes.node_index(planes.elem_conn[row])]
        assert np.allclose(corners.mean(axis=0)[[0, 2]], bolt[0])
    geometry = barc.geometry()
    links = geometry.elements_in('bolts')
    assert len(links) == 8 * 16 + 8 * 4


def test_the_project_is_the_app_workflow(tmp_path):
    """What the docs page walks through, as the project verbs the app
    calls: the saved project opened, and Solve Modes on it."""
    import hertzforge

    project = barc.project()
    assert list(project.keys()) == ['BARC'], 'the model alone'
    project.save(tmp_path / 'barc.vdyn')
    project = hertzforge.load(tmp_path / 'barc.vdyn')
    solved = project.solve_modes('BARC', maximum_frequency=1200.0)
    elastic = [f for f in project[solved].frequency if f > 1.0][:3]
    assert np.allclose(elastic, CHECKED[:3], rtol=0.005)


def _quads(geometry):
    """Each element as its group's name and its corners' places, on a
    64th of an inch, so two meshes numbered differently compare."""
    corners = np.round(geometry.node_xyz / INCH * 64).astype(int)
    names = dict(zip(geometry.group_id.tolist(), geometry.group_name))
    return Counter((names[int(group)], frozenset(map(tuple, corners[geometry.node_index(conn)])))
                   for conn, group in zip(geometry.elem_conn, geometry.elem_group))




#: the first ten elastic modes of the assembly of bricks at `barc.SIZE`,
#: Hz — its bolts merged nodes since 2026-10-11, 0.3 to 3.4 % under the
#: build checked against the wiki's model and test (MAC 0.97 and up, in
#: order), which joined each foot over its whole footprint and read 1 to
#: 4 % above the measured modes: (191.8, 213.1, 271.6, 457.9, 490.7,
#: 581.5, 603.1, 696.7, 1150.0, 1226.0)
SOLID_CHECKED = (191.2, 210.7, 263.3, 442.4, 485.8, 570.1, 598.5, 689.6,
                 1124.3, 1197.6)

#: the removable component's first seven, 0.3 to 1.2 % under the build
#: checked against the shared model of it (MAC 0.999 and up), which was
#: 6 % under it for the first two and within 2 % above after: (280.0,
#: 435.6, 596.3, 1320.8, 1633.8, 1954.8, 2025.9)
PART_CHECKED = (278.6, 432.9, 592.2, 1305.0, 1627.8, 1949.3, 2014.7)


def test_the_bricks_are_the_solid_model_and_the_bolts_weigh_in():
    """The bricks' volume is the solid model's without its bolt holes
    (the box 16.9 in³ as a tube with its slot, the channels and beam
    as drawn); the bolts are point masses at their heads, eight #8 at
    the feet and eight at the beam carrying the shared model's two 1/4
    in screws between them, and nothing is rigid: the parts are joined
    by the nodes their bolts merged."""
    from hertzforge.core.fem import Model

    geometry = barc.solid_geometry()
    assert list(geometry.group_name) == ['box', 'right channel',
                                         'left channel', 'beam',
                                         'foot bolts', 'top bolts']
    assert len(geometry.elements_in('foot bolts')) == 8
    assert len(geometry.elements_in('top bolts')) == 8
    model = Model.from_geometry(geometry)
    assert not model.rigid_links
    lb = 0.45359237
    assert sum(m.mass for m in model.masses) == pytest.approx(
        (8 * 0.00744 + 2 * 0.0125) * lb)
    bricks = (6 * 6 - 5.5 * 5.5 - 0.5 * 0.25) * 3.0 + 2 * (
        2 * 0.125 + 1.75 * 0.125) + 5 * 0.125
    assert model.structural_mass == pytest.approx(
        bricks * INCH3 * barc.MATERIAL.density)
    assert len(model.pieces()) == 1


def _shared(geometry, a, b):
    ids = dict(zip(geometry.group_name, geometry.group_id.tolist()))
    nodes = [set(np.concatenate([geometry.elem_conn[i] for i in np.flatnonzero(
        geometry.elem_group == ids[name])]).tolist()) for name in (a, b)]
    return nodes[0] & nodes[1]


def test_each_joint_ties_the_places_its_bolts_clamp_and_no_other():
    """Meshed apart, each channel's foot is 9 x 9 places coincident with
    the box and each channel's top 9 x 9 with the beam, none shared. Each
    joint is a set of mated surfaces, rigid (Brandon, 2026-10-20: the
    examples remade with the new joints): a foot's four bolts tie the
    washer's 4 x 4 places each, which share their middle rows into one
    7 x 7 square; a beam end's four bolts tie the four corners of the
    brick each passes through. The patch is as Mate Surfaces finds it,
    the part below the first side (both sides hold 81, and the first
    group picked follows), kept at the bolts' places and clicked off
    everywhere else; the second side stays the patch's whole 81."""
    geometry = barc.solid_geometry()
    ids = dict(zip(geometry.group_name, geometry.group_id.tolist()))
    sets = {m.name: m for m in geometry.mates}
    assert sorted(sets) == ['left foot bolts', 'left top bolts', 'right foot bolts',
                            'right top bolts']
    assert all(m.describe() == 'rigid' for m in sets.values())
    for side in ('right', 'left'):
        channel = f'{side} channel'
        for kind, below, above, places in (('foot', 'box', channel, 49),
                                           ('top', channel, 'beam', 16)):
            mated = sets[f'{side} {kind} bolts']
            assert mated.groups == (ids[below], ids[above])
            assert (len(mated.nodes_a), len(mated.nodes_b)) == (places, 81), (side, kind)
            assert not _shared(geometry, below, above), 'joined by ties, not merged'
    inch = geometry.node_xyz / 0.0254
    for (bx, bz), kind in barc.SOLID_BOLTS[8:]:
        side = 'right' if bx > 0 else 'left'
        places = barc.bolt_places(geometry, (bx, bz), kind)
        at = inch[geometry.node_index(sorted(places))]
        assert np.allclose(at.mean(axis=0)[[0, 2]], (bx, bz))
        assert np.allclose(np.ptp(at, axis=0), [0.125, 0.0, 0.125])
        # the joint's first side, the channel, is tied at the places'
        # partners: the four corners of the brick the bolt passes through
        tied = inch[geometry.node_index(list(sets[f'{side} top bolts'].nodes_a))]
        near = tied[np.hypot(tied[:, 0] - bx, tied[:, 2] - bz) < 0.1]
        assert len(near) == 4


def test_the_removable_component_stays_where_it_was_checked():
    geometry = barc.solid_geometry('removable component')
    assert 'box' not in list(geometry.group_name)
    assert 'foot bolts' not in list(geometry.group_name), \
        'its foot bolts stay with the box'
    shapes = barc.solid_build('removable component').eigensolution(
        maximum_frequency=barc.PART_SOLVE_TO)
    assert int(np.sum(shapes.frequency == 0.0)) == 6, 'free-free'
    elastic = [f for f in shapes.frequency if f > 1.0][:7]
    assert np.allclose(elastic, PART_CHECKED, rtol=0.005), np.round(elastic, 1)
    with pytest.raises(ValueError, match='not one of'):
        barc.solid_geometry('bench')


@pytest.mark.slow
def test_the_bricks_stay_where_they_were_checked():
    shapes = barc.solid_build().eigensolution(maximum_frequency=barc.SOLVE_TO)
    assert int(np.sum(shapes.frequency == 0.0)) == 6, 'free-free'
    elastic = [f for f in shapes.frequency if f > 1.0][:10]
    assert np.allclose(elastic, SOLID_CHECKED, rtol=0.005), np.round(elastic, 1)


def test_a_coarser_mesh_of_bricks_still_holds_together():
    """Every box is cut at every other's faces before it is meshed: at
    a quarter inch a channel's eighth-inch web met a flange with no line
    of nodes there, hung from one edge, and the BARC read 76 Hz for
    192 (2026-10-07). Coarse now reads 1 to 5 % above the checked mesh:
    each foot bolt clamps three bricks across, which at a quarter inch
    is the whole of a one-inch foot (the website's figure is drawn at
    it)."""
    shapes = barc.solid_build('BARC', 0.25).eigensolution(
        maximum_frequency=1300.0)
    elastic = [f for f in shapes.frequency if f > 1.0][:8]
    assert np.allclose(elastic, SOLID_CHECKED[:8], rtol=0.06), \
        np.round(elastic, 1)
