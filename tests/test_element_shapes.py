"""Element shape sets (`core.element_shapes`): a solved model's modes read
as strain and stress at each solid element's centroid and rule points.

What is pinned: a uniform strain comes back exactly on distorted bricks,
wedges and tetrahedra, as strain and as the stress D gives it; the
stress and strain at the rule's points give back u^T K u for every solid
kind, the brick's incompatible modes included; summed over a whole
model, a mass-normalized mode's strain energy is omega^2 / 2, with a
turned displacement frame and with a mated surface's moved nodes; a
cantilever of bricks bends as beam theory says; and what cannot be read
one way is refused by its cause.
"""

from __future__ import annotations

import copy

import numpy as np
import pytest
from test_fem import ALUMINUM

from hertzforge.core import element_shapes as es
from hertzforge.core import fem
from hertzforge.core.fem import GroupProperties, Model
from hertzforge.core.shapes import ShapeSet

TETRA = np.array([[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1]], float)
TET10 = np.vstack([TETRA] + [(TETRA[i] + TETRA[j]) / 2
                             for i, j in ((0, 1), (1, 2), (0, 2), (0, 3), (1, 3), (2, 3))])
WEDGE = np.array([[0, 0, 0], [1, 0, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [0, 1, 1]], float)
BRICK = (fem._HEX_CORNERS + 1.0) / 2.0
SOLIDS = {8: BRICK, 6: WEDGE, 4: TETRA, 10: TET10}


def _distorted(nodes, count=6, seed=0):
    rng = np.random.default_rng(seed)
    # midside nodes of the ten-node tetrahedron moved less, so its sides
    # stay near straight
    spread = 0.02 if nodes == 10 else 0.08
    return SOLIDS[nodes][None] * 0.01 + rng.normal(0.0, spread * 0.01, (count, nodes, 3))


def _voigt(gradient):
    g = gradient
    return np.array([g[0, 0], g[1, 1], g[2, 2], g[0, 1] + g[1, 0],
                     g[1, 2] + g[2, 1], g[0, 2] + g[2, 0]])


@pytest.mark.parametrize('nodes', [8, 6, 4, 10])
def test_a_uniform_strain_comes_back_exactly_on_distorted_solids(nodes):
    """The patch test, read back: u = G x on elements that are not
    parallelepipeds gives the strain sym(G) at the centroid and at every
    rule point, and the stress D sym(G). The brick passes it because its
    incompatible modes' strains are taken through the centroid's
    Jacobian; a uniform field excites none of them."""
    xyz = _distorted(nodes)
    rng = np.random.default_rng(3)
    gradient = rng.normal(0.0, 1e-3, (3, 3))
    motion = np.einsum('ij,enj->eni', gradient, xyz).reshape(len(xyz), -1, 1)
    strain, stress, positions, weights = fem._solid_strains_many(ALUMINUM, xyz, motion)
    want = _voigt(gradient)
    assert strain.shape[1] == {8: 9, 6: 7, 4: 2, 10: 28}[nodes]
    assert np.abs(strain[..., 0] - want).max() < 1e-12 * np.abs(want).max()
    assert np.allclose(stress[..., 0], fem._isotropic(ALUMINUM) @ want, rtol=1e-11, atol=0)
    assert np.all(weights[:, 0] == 0.0), 'the centroid weighs nothing'
    assert weights.sum(axis=1) == pytest.approx(
        np.abs(fem._solid_volumes(xyz)), rel=1e-12)
    if nodes != 10:
        # a linear solid's centroid is the mean of its nodes
        assert np.allclose(positions[:, 0], xyz.mean(axis=1), rtol=0, atol=1e-15)


@pytest.mark.parametrize('nodes', [8, 6, 4, 10])
def test_the_rule_points_give_back_each_elements_strain_energy(nodes):
    """Sum over the rule's points of weight times stress dot strain is
    u^T K u for any motion, the stiffness the solve used: the brick's
    condensed one only when its incompatible modes are recovered first."""
    xyz = _distorted(nodes, seed=4)
    rng = np.random.default_rng(5)
    motion = rng.normal(size=(len(xyz), 3 * nodes, 3))
    stiffness, _mass = fem._solid_matrices_many(ALUMINUM, xyz)
    strain, stress, _positions, weights = fem._solid_strains_many(ALUMINUM, xyz, motion)
    energy = np.einsum('ep,epcm,epcm->em', weights, stress, strain)
    want = np.einsum('eim,eij,ejm->em', motion, stiffness, motion)
    assert energy == pytest.approx(want, rel=1e-12)


def _cells(along=8, seed=7):
    """A free bar of distorted cells along x, 0.04 x 0.01 x 0.01 m a
    cell each: bricks, then a cell split into two wedges, then one split
    into six tetrahedra, then bricks again; one element group each kind."""
    rng = np.random.default_rng(seed)
    model = Model('cells')
    nid = {}
    for i in range(along + 1):
        for j in range(2):
            for k in range(2):
                jitter = rng.normal(0.0, 0.0006, 3) if 0 < i < along else np.zeros(3)
                nid[i, j, k] = model.add_node(len(nid) + 1, *(np.array(
                    [0.04 * i, 0.01 * j, 0.01 * k]) + jitter))
    for i in range(along):
        c = [nid[i + a, b, z] for z in (0, 1)
             for a, b in ((0, 0), (1, 0), (1, 1), (0, 1))]
        if i == 3:
            model.add_solid([c[0], c[1], c[2], c[4], c[5], c[6]], ALUMINUM, group='wedges')
            model.add_solid([c[0], c[2], c[3], c[4], c[6], c[7]], ALUMINUM, group='wedges')
        elif i == 5:
            for t in ((0, 1, 2, 6), (0, 2, 3, 6), (0, 3, 7, 6), (0, 7, 4, 6),
                      (0, 4, 5, 6), (0, 5, 1, 6)):
                model.add_solid([c[q] for q in t], ALUMINUM, group='tetrahedra')
        else:
            model.add_solid(c, ALUMINUM, group='bricks')
    geometry = model.geometry()
    geometry.group_properties = {int(g): GroupProperties(ALUMINUM) for g in geometry.group_id}
    return geometry


def test_a_mass_normalized_modes_strain_energy_is_half_omega_squared():
    """Over every element of a model of bricks, wedges and tetrahedra,
    half the weighted stress dot strain of each mass-normalized mode is
    omega^2 / 2, and the rigid-body modes strain nothing."""
    geometry = _cells()
    shapes = Model.from_geometry(geometry).eigensolution(num_modes=14)
    read = es.element_shapes(geometry, shapes)
    omega = 2.0 * np.pi * shapes.frequency
    assert read.strain_energy()[6:] == pytest.approx(omega[6:] ** 2 / 2.0, rel=1e-9)
    assert np.abs(read.stress[:6]).max() < 1e-6 * np.abs(read.stress[6:]).max()
    assert read.num_points == 6 * 9 + 2 * 7 + 6 * 2
    assert len(read.elements) == 14
    assert list(read.point[:9]) == list(range(9))
    # a group alone is its elements alone, and its energy a share of the whole
    wedges = int(geometry.group_id[list(geometry.group_name).index('wedges')])
    part = es.element_shapes(geometry, shapes, groups=[wedges], modes=[6, 7])
    assert part.num_points == 14 and part.num_shapes == 2
    assert np.all(part.strain_energy() < read.strain_energy()[6:8])
    rows = np.isin(read.element, part.elements)
    assert np.allclose(part.stress, read.stress[6:8][:, rows])


def test_a_turned_displacement_frame_is_read_back_to_the_global_axes():
    """A node measured in a turned frame has its mode written in that
    frame (`Model.eigensolution`); the strain is read in the global
    axes all the same, so the set is the one the plain geometry gives."""
    plain = _cells()
    turned = copy.deepcopy(plain)
    frame = turned.add_coordinate_system(rotation=[[0, 1, 0], [-1, 0, 0], [0, 0, 1]])
    turned.node_disp_cs[9] = frame
    turned.node_disp_cs[14] = frame
    a = es.element_shapes(plain, Model.from_geometry(plain).eigensolution(num_modes=10))
    b = es.element_shapes(turned, Model.from_geometry(turned).eigensolution(num_modes=10))
    sign = np.sign(np.sum(a.stress[6:] * b.stress[6:], axis=(1, 2)))
    assert np.allclose(b.stress[6:] * sign[:, None, None], a.stress[6:],
                       atol=1e-9 * np.abs(a.stress).max())


def test_a_mated_surfaces_moved_nodes_are_where_the_strain_is_read():
    """A rod tied rigidly in its sleeve: the bore's nodes are moved onto
    the rod's facets in the model the modes come from, a chord's sag, so
    the strain is read there too. Read where the geometry has them, the
    energy is out by far more than round-off."""
    from test_mate import INCH, _mated, _rod_in_sleeve

    geometry = _mated(_rod_in_sleeve(), tolerance=0.02 * INCH)
    shapes = Model.from_geometry(geometry).eigensolution(num_modes=9)
    omega = 2.0 * np.pi * shapes.frequency[6:]
    read = es.element_shapes(geometry, shapes, modes=[6, 7, 8])
    assert read.strain_energy() == pytest.approx(omega ** 2 / 2.0, rel=1e-9)
    unmoved = copy.deepcopy(geometry)
    unmoved.mates = []
    off = es.element_shapes(unmoved, shapes, modes=[6, 7, 8]).strain_energy()
    assert np.abs(off / (omega ** 2 / 2.0) - 1.0).max() > 1e-6


def _cantilever(length=0.5, width=0.03, depth=0.02, along=50):
    """A bar of bricks along x, `width` along y in three bricks and
    `depth` along z in two, clamped at x = 0 by its end nodes. Wider than
    it is deep, so its first mode bends in z alone, where a square
    section's two bending modes share a frequency and the solver returns
    any mix of them."""
    model = Model('cantilever')
    nid = {}
    wide, deep = 3, 2
    for i in range(along + 1):
        for j in range(wide + 1):
            for k in range(deep + 1):
                nid[i, j, k] = model.add_node(
                    len(nid) + 1, length * i / along, width * (j / wide - 0.5),
                    depth * (k / deep - 0.5))
    for i in range(along):
        for j in range(wide):
            for k in range(deep):
                model.add_solid([nid[i + a, j + b, k + z] for z in (0, 1)
                                 for a, b in ((0, 0), (1, 0), (1, 1), (0, 1))],
                                ALUMINUM, group='bar')
    clamped = [str(nid[0, j, k]) for j in range(wide + 1) for k in range(deep + 1)]
    geometry = model.geometry()
    geometry.group_properties = {int(geometry.group_id[0]): GroupProperties(ALUMINUM)}
    return geometry, clamped


def test_a_cantilever_of_bricks_bends_as_beam_theory_says():
    """The first bending mode of a clamped bar 25 to 1 deep, mass-normalized,
    against the Euler-Bernoulli cantilever's: the axial stress at each
    brick's centroid is -E z phi''(x), read past the first tenth of the span.

    Tolerance: 1 % of the span's largest stress; it reads 0.5 %. The
    beam omits shear and rotary inertia, a (depth / length)^2 effect of
    0.2 % here, and the frequencies agree to 0.3 %, which is checked
    first; fifty bricks along the bar read the curvature at their
    centroids to a few tenths of a percent, and a stress linear through
    the depth is read exactly there. The clamp holds the end's section
    against the Poisson contraction beam theory lets it have, which puts
    the stress out by 2.6 % within a depth or two of the root, so the
    first tenth of the span is left out.
    """
    length, width, depth = 0.5, 0.03, 0.02
    geometry, clamped = _cantilever(length, width, depth)
    model = Model.from_geometry(geometry)
    shapes = model.eigensolution(num_modes=1, fixed=clamped)
    E, rho = ALUMINUM.youngs_modulus, ALUMINUM.density
    area, inertia = width * depth, width * depth ** 3 / 12.0
    beta = 1.875104068711961 / length
    omega = beta ** 2 * np.sqrt(E * inertia / (rho * area))
    assert 2.0 * np.pi * shapes.frequency[0] == pytest.approx(omega, rel=0.005)
    sigma = (np.cosh(beta * length) + np.cos(beta * length)) / (
        np.sinh(beta * length) + np.sin(beta * length))
    read = es.element_shapes(geometry, shapes)
    xyz = read.xyz[read.point == 0]
    x, _y, z = xyz.T
    stress = read.values('stress', 'xx', 0)

    def curvature(s):
        b = beta * s
        return beta ** 2 * (np.cosh(b) + np.cos(b) - sigma * (np.sinh(b) + np.sin(b)))

    # unit modal mass: phi = c (cosh - cos - sigma (sinh - sin)), and the
    # mode's integral of rho A phi^2 over the length is rho A L c^2
    scale = 1.0 / np.sqrt(rho * area * length)
    theory = -E * z * scale * curvature(x)
    middle = x > 0.1 * length
    sign = np.sign(stress[middle] @ theory[middle])
    worst = np.abs(sign * stress[middle] - theory[middle]).max()
    assert worst < 0.01 * np.abs(theory[middle]).max(), worst / np.abs(theory).max()
    # and the bending carries no axial force: the stress is odd in z
    assert abs(stress[middle].sum()) < 1e-6 * np.abs(stress).sum()


def test_one_value_per_element_or_per_point():
    geometry = _cells(along=4)
    shapes = Model.from_geometry(geometry).eigensolution(num_modes=8)
    read = es.element_shapes(geometry, shapes)
    assert read.values('strain', 'xy', 7).shape == (len(read.elements),)
    assert read.values('stress', 'zz', 7, at='points').shape == (read.num_points,)
    first = int(read.elements[0])
    assert list(read.rows_of(first)) == list(range(9))
    with pytest.raises(KeyError, match='element 999 is not in this set'):
        read.rows_of(999)
    with pytest.raises(ValueError, match="'shear' is not a component: xx, yy"):
        read.values('stress', 'shear', 7)
    with pytest.raises(ValueError, match="'energy' is not strain or stress"):
        read.values('energy', 'xx', 7)
    with pytest.raises(IndexError, match='mode 9 of 8'):
        read.values('stress', 'xx', 8)
    kept = read.kept([7, 6])
    assert list(kept.frequency) == [shapes.frequency[7], shapes.frequency[6]]
    assert np.array_equal(kept.stress[0], read.stress[7])
    assert kept == read.kept([7, 6]) and kept != read


def test_what_cannot_be_read_is_refused_by_its_cause():
    geometry = _cells(along=4)
    shapes = Model.from_geometry(geometry).eigensolution(num_modes=8)
    with pytest.raises(ValueError, match='no element group 77'):
        es.element_shapes(geometry, shapes, groups=[77])
    with pytest.raises(ValueError, match='no element groups were chosen'):
        es.element_shapes(geometry, shapes, groups=[])
    with pytest.raises(ValueError, match='no modes were chosen'):
        es.element_shapes(geometry, shapes, modes=[])
    with pytest.raises(ValueError, match='mode 9 is not in the shape set, which has 8'):
        es.element_shapes(geometry, shapes, modes=[8])
    # a set at a test's DOFs does not reach the model's nodes
    some = [dof for dof in shapes.coordinate if not dof.startswith('5')]
    partial = ShapeSet(shapes.frequency, shapes.damping, some,
                       shapes.shape_matrix[:, [shapes.coordinate.index(d) for d in some]])
    with pytest.raises(ValueError, match=r'the shape set has no 5X\+: .*Choose the solved'):
        es.element_shapes(geometry, partial)
    holed = copy.deepcopy(shapes)
    holed.shape_matrix[7, holed.coordinate.index('3Y+')] = np.nan
    with pytest.raises(ValueError, match=r'mode 8 has no value at 3Y\+'):
        es.element_shapes(geometry, holed)
    empty = ShapeSet(np.zeros(0), np.zeros(0), shapes.coordinate,
                     np.zeros((0, len(shapes.coordinate))))
    with pytest.raises(ValueError, match='the shape set has no modes'):
        es.element_shapes(geometry, empty)
    bare = copy.deepcopy(geometry)
    bare.group_properties = {}
    with pytest.raises(ValueError, match=r'element group \d+ \(bricks\) has no material'):
        es.element_shapes(bare, shapes)


def test_a_plate_or_a_beam_is_refused_by_name_and_a_mass_passed_over():
    from test_fem import square_plate

    plate = square_plate(3).geometry()
    plate.group_properties = {int(plate.group_id[0]): GroupProperties(ALUMINUM, 0.01)}
    shapes = Model.from_geometry(plate).eigensolution(num_modes=8)
    with pytest.raises(ValueError, match=r'holds quad4.*\(type 44\), which have no strain '
                                         r'and stress recovery: bricks, wedges'):
        es.element_shapes(plate, shapes)
    # a point mass on the bar holds no strain field: passed over unless named
    geometry = _cells(along=4)
    group = geometry.add_group('weight')
    geometry.add_element([int(geometry.node_id[-1])], 161, group=group)
    geometry.group_properties[group] = GroupProperties(mass=0.01)
    solved = Model.from_geometry(geometry).eigensolution(num_modes=8)
    assert len(es.element_shapes(geometry, solved).elements) == 5
    with pytest.raises(ValueError, match=r'element group \d+ \(weight\) holds '
                                         r'\w+ elements \(type 161\)'):
        es.element_shapes(geometry, solved, groups=[group])


def test_a_set_too_large_is_refused_with_its_size(monkeypatch):
    geometry = _cells(along=4)
    shapes = Model.from_geometry(geometry).eigensolution(num_modes=8)
    monkeypatch.setattr(es, 'LIMIT', 10_000)
    with pytest.raises(ValueError, match=r'5 elements at 41 stress points for 8 modes is '
                                         r'0\.0 GB .*choose fewer element groups or fewer'):
        es.element_shapes(geometry, shapes)
    assert es.element_shapes(geometry, shapes, modes=[6]).num_shapes == 1
