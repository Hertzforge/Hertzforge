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
    project.save(tmp_path / 'barc.hforge')
    project = hertzforge.load(tmp_path / 'barc.hforge')
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




#: the published calibration of the BARC model shared on the wiki (`barc.WIKI`, its
#: Hardware and Models folder under FEM, Calibration Results.xlsx, sheet
#: Frequencies, read 2026-11-10): its 55 elastic
#: modes below 9.84 kHz and the measured modes it paired them with, Hz, in the
#: model's order. The test's own order differs where two modes cross
CALIBRATED = [
    (185.46, 187.88), (205.82, 207.56), (263.29, 263.08), (445.00, 441.14),
    (475.17, 475.90), (561.08, 564.73), (580.46, 585.94), (669.49, 666.41),
    (1103.28, 1112.16), (1168.73, 1204.45), (1521.98, 1508.59), (1634.00, 1628.91),
    (1779.33, 1765.27), (1813.39, 1729.69), (1991.64, 1963.86), (2095.37, 2064.84),
    (2312.36, 2271.09), (2428.33, 2366.60), (2667.41, 2685.69), (2732.71, 2643.22),
    (2930.50, 2980.47), (3081.59, 3060.16), (3510.78, 3476.56), (4038.78, 4028.91),
    (4155.30, 4157.76), (4423.07, 4432.03), (4858.73, 4867.19), (5258.47, 5300.50),
    (5365.74, 5260.16), (5377.88, 5376.56), (5718.68, 5575.78), (5812.88, 5682.81),
    (6562.47, 6403.34), (6611.12, 6455.52), (6624.43, 6475.78), (6712.68, 6628.91),
    (6815.28, 6749.22), (6872.91, 6793.75), (6915.92, 6845.01), (7072.53, 6827.63),
    (7248.74, 7120.31), (7532.86, 7568.75), (7711.39, 7626.22), (7805.66, 7754.31),
    (7966.37, 7915.63), (8017.78, 7970.31), (8082.97, 8043.80), (8471.71, 8415.63),
    (8658.23, 8487.50), (8840.98, 8755.47), (9305.83, 9299.22), (9544.98, 9480.47),
    (9561.05, 9567.19), (9740.24, 9646.09), (9840.21, 9819.53)]

#: the first ten elastic modes of the assembly of bricks at `barc.SOLID_SIZE`,
#: Hz, 2026-11-10: within 1.2 % of the calibrated model's (`CALIBRATED`)
SOLID_CHECKED = (185.3, 205.1, 264.1, 439.8, 472.3, 561.1, 579.7, 670.0,
                 1109.3, 1180.2)

#: the removable component's first seven at `barc.SOLID_SIZE`, 2026-11-10,
#: with the calibrated model's sizes and joints. The wiki's folder has no
#: frequencies for it alone, only its shapes (rcomp_shp.tam)
PART_CHECKED = (287.7, 434.2, 586.3, 1277.7, 1554.3, 1870.7, 1932.2)


def test_the_bricks_are_the_solid_model_and_the_bolts_weigh_in():
    """The bricks' volume is the calibrated model's parts without their bolt
    holes (the box a tube 5.98 in square with a 0.238 in wall and its slot,
    the channels' webs 0.12 in, the beam 0.128 in); the bolts are point
    masses at their heads, eight #8 at the feet and eight at the beam
    carrying the shared model's two 1/4 in screws between them, and
    nothing is rigid: the parts are joined by mated surfaces. Built at
    `barc.SIZE`, which gives the same volume quicker."""
    from hertzforge.core.fem import Model

    geometry = barc.solid_geometry(size=barc.SIZE)
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
    bricks = (5.98 ** 2 - 5.504 ** 2 - 0.5 * 0.238) * 3.0 + 2 * (
        2.0 - 0.88 * 1.75) + 5 * 0.128
    assert model.structural_mass == pytest.approx(
        bricks * INCH3 * barc.SOLID_MATERIAL.density)
    assert len(model.pieces()) == 1


def _shared(geometry, a, b):
    ids = dict(zip(geometry.group_name, geometry.group_id.tolist()))
    nodes = [set(np.concatenate([geometry.elem_conn[i] for i in np.flatnonzero(
        geometry.elem_group == ids[name])]).tolist()) for name in (a, b)]
    return nodes[0] & nodes[1]


def test_each_joint_ties_where_the_calibrated_model_does():
    """Meshed apart, each channel's foot is 17 x 17 places coincident with
    the box and each channel's top 17 x 17 with the beam, none shared.
    Each joint is a set of mated surfaces, rigid, as the calibrated model
    joins them (its deck ties each foot's face to the box, and its mesh
    bonds the beam to each channel over a half-inch square around the
    bolts): a foot tied over its whole face, the beam over the square's
    places and clicked off everywhere else. The patch is as Mate
    Surfaces finds it, the part below the first side."""
    geometry = barc.solid_geometry()
    ids = dict(zip(geometry.group_name, geometry.group_id.tolist()))
    sets = {m.name: m for m in geometry.mates}
    assert sorted(sets) == ['left foot bolts', 'left top bolts', 'right foot bolts',
                            'right top bolts']
    assert all(m.describe() == 'rigid' for m in sets.values())
    inch = geometry.node_xyz / INCH
    for side, center in (('right', 2.0), ('left', -2.0)):
        channel = f'{side} channel'
        for kind, below, above in (('foot', 'box', channel), ('top', channel, 'beam')):
            mated = sets[f'{side} {kind} bolts']
            assert mated.groups == (ids[below], ids[above])
            assert len(mated.nodes_b) == 289, (side, kind)
            assert not _shared(geometry, below, above), 'joined by ties, not merged'
        assert len(sets[f'{side} foot bolts'].nodes_a) == 289
        # at the beam, the channel's top inside the square and nothing else
        tied = inch[geometry.node_index(list(sets[f'{side} top bolts'].nodes_a))]
        assert np.all(np.abs(tied[:, 0] - center) <= barc.BEAM_BOND / 2 + 1e-6)
        assert np.all(np.abs(tied[:, 2] - 1.5) <= barc.BEAM_BOND / 2 + 1e-6)
        top = inch[geometry.node_index(sorted(_shared_places(geometry, channel)))]
        inside = ((np.abs(top[:, 0] - center) <= barc.BEAM_BOND / 2 + 1e-6)
                  & (np.abs(top[:, 2] - 1.5) <= barc.BEAM_BOND / 2 + 1e-6))
        assert len(tied) == int(inside.sum())


def _shared_places(geometry, channel):
    """The channel's nodes on its top face, where the beam sits."""
    ids = dict(zip(geometry.group_name, geometry.group_id.tolist()))
    nodes = set(np.concatenate([geometry.elem_conn[i] for i in np.flatnonzero(
        geometry.elem_group == ids[channel])]).tolist())
    inch = geometry.node_xyz / INCH
    return {n for n in nodes if abs(inch[geometry.node_index([n])[0], 1] - 5.0) < 1e-6}


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
def test_the_bricks_match_the_calibrated_model():
    """About a minute and 14 GB: every mode to 10 kHz. The bricks have the
    the calibrated model's 55 elastic modes below 9.84 kHz, each within 2 % of its
    in order and 0.4 % on average (MAC unchecked; the published table
    pairs its modes with the test's by MAC)."""
    shapes = barc.solid_build().eigensolution(maximum_frequency=10000.0)
    assert int(np.sum(shapes.frequency == 0.0)) == 6, 'free-free'
    elastic = np.array([f for f in shapes.frequency if f > 1.0])
    assert np.allclose(elastic[:10], SOLID_CHECKED, rtol=0.005), np.round(elastic[:10], 1)
    model = np.array([fe for fe, _test in CALIBRATED])
    assert int(np.sum(elastic <= model[-1] + 1.0)) == len(model) == 55
    error = np.abs(elastic[:55] / model - 1)
    assert error.max() < 0.02 and error.mean() < 0.005, np.round(100 * error, 1)


def test_a_coarser_mesh_of_bricks_still_holds_together():
    """Every box is cut at every other's faces before it is meshed: at
    a quarter inch a channel's eighth-inch web met a flange with no line
    of nodes there, hung from one edge, and the BARC read 76 Hz for
    192 (2026-10-07). At a quarter inch it reads within 1 % of the
    checked mesh's first eight (the website's figure is drawn at it)."""
    shapes = barc.solid_build('BARC', 0.25).eigensolution(
        maximum_frequency=1300.0)
    elastic = [f for f in shapes.frequency if f > 1.0][:8]
    assert np.allclose(elastic, SOLID_CHECKED[:8], rtol=0.02), \
        np.round(elastic, 1)


def test_the_example_project_is_the_barc_of_bricks():
    """The downloads page's BARC is the brick model, as the RC plate's is."""
    built = barc.solid_project()
    assert [name for name, _obj in built.items()] == ['BARC']
    assert 'box' in list(built['BARC'].group_name) and 'foot bolts' in list(built['BARC'].group_name)
    # a mated set for each side's foot bolts and top bolts
    assert len(built['BARC'].mates) == 4
