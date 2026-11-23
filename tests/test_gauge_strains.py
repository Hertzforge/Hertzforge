"""Strain at a gauge (`core.element_shapes.gauge_strains`): the strain a gauge
on a solid's surface reads in each mode, at a node and along a direction in
the surface, so a strain channel sits in a channel table beside accelerations.

What is pinned: a uniform strain field is read exactly at every surface node
and in every direction in the surface, distorted bricks, wedges and
tetrahedra alike; a gauge on a cantilever reads the beam's surface strain; a
gauge turned end for end reads the same; what a gauge cannot read is refused
by its cause; a channel table's strain channels are read on the model the
modes were solved on, matched by the nearest node when the table is on a
test geometry; and Expand at a channel table answers a strain channel with
its strain per force, beside an acceleration at the same node and direction.
"""

from __future__ import annotations

import numpy as np
import pytest
from test_element_shapes import ALUMINUM, _cantilever, _cells

import hertzforge
from hertzforge.core import element_shapes as es
from hertzforge.core.shapes import ShapeSet


def _uniform(geometry, gradient):
    """A one-mode set moving every node by u = G x."""
    dofs, values = [], []
    for node, x in zip(geometry.node_id, geometry.node_xyz):
        moved = gradient @ x
        for k, axis in enumerate('XYZ'):
            dofs.append(f'{int(node)}{axis}+')
            values.append(moved[k])
    return ShapeSet([10.0], [0.0], dofs, [values])


def test_a_uniform_strain_is_read_exactly_at_every_surface_node():
    """Distorted bricks, two wedges and six tetrahedra in a row; each node is
    on the surface, and each axis lies in one of its faces there."""
    geometry = _cells()
    gradient = np.random.default_rng(2).normal(0.0, 1e-3, (3, 3))
    tensor = (gradient + gradient.T) / 2.0
    shapes = _uniform(geometry, gradient)
    read = 0
    for node in geometry.node_id:
        for d in ([1, 0, 0], [0, 1, 0], [0, 0, 1], [1, 1, 0], [1, 0, -1]):
            d = np.asarray(d, float) / np.linalg.norm(d)
            try:
                value = es.gauge_strains(geometry, shapes, [(int(node), d)])[0, 0]
            except ValueError as error:
                assert 'out of the surface' in str(error)
                continue
            assert value == pytest.approx(d @ tensor @ d, abs=1e-15)
            read += 1
    assert read >= 3 * len(geometry.node_id), 'each node read along the axes at least'


def test_a_gauge_turned_end_for_end_reads_the_same():
    geometry = _cells(along=4)
    shapes = _uniform(geometry, np.random.default_rng(3).normal(0.0, 1e-3, (3, 3)))
    node = int(geometry.node_id[5])
    plus = es.gauge_strains(geometry, shapes, [(node, [1.0, 0.0, 0.0])])
    minus = es.gauge_strains(geometry, shapes, [(node, [-2.0, 0.0, 0.0])])
    assert np.allclose(plus, minus, rtol=1e-14, atol=0)


def test_a_gauge_on_a_cantilever_reads_the_beams_surface_strain():
    """The first bending mode of the clamped bar of bricks: a gauge on the
    top face along the bar reads -h/2 phi''(x), mass-normalized as the
    Euler-Bernoulli cantilever is.

    Tolerance: 1 % of the largest strain read; it reads 0.5 %, as the
    stress at the bricks' centroids does (`test_element_shapes`). A node's
    strain is each brick's own field at that corner, its incompatible modes
    included, averaged over the bricks there. The first tenth of the span is
    left out: the clamp holds the end's section against the Poisson
    contraction the beam lets it have, and at the clamped face itself the
    gauge reads 19 % off the beam.
    """
    length, width, depth = 0.5, 0.03, 0.02
    geometry, clamped = _cantilever(length, width, depth)
    shapes = hertzforge.core.fem.Model.from_geometry(geometry).eigensolution(
        num_modes=1, fixed=clamped)
    rho = ALUMINUM.density
    area = width * depth
    beta = 1.875104068711961 / length
    sigma = (np.cosh(beta * length) + np.cos(beta * length)) / (
        np.sinh(beta * length) + np.sin(beta * length))
    xyz = np.asarray(geometry.node_xyz)
    # the top face's middle line: away from the side faces, along the bar
    top = np.flatnonzero((np.abs(xyz[:, 2] - depth / 2) < 1e-9)
                         & (np.abs(xyz[:, 1] - width / 6) < 1e-9)
                         & (xyz[:, 0] > 0.1 * length))
    gauges = [(int(geometry.node_id[k]), [1.0, 0.0, 0.0]) for k in top]
    read = es.gauge_strains(geometry, shapes, gauges)[0]
    x = xyz[top, 0]
    b = beta * x
    curvature = beta ** 2 * (np.cosh(b) + np.cos(b) - sigma * (np.sinh(b) + np.sin(b)))
    theory = -(depth / 2) * curvature / np.sqrt(rho * area * length)
    sign = np.sign(read @ theory)
    worst = np.abs(sign * read - theory).max()
    assert worst < 0.01 * np.abs(theory).max(), worst / np.abs(theory).max()


def test_what_a_gauge_cannot_read_is_refused_by_its_cause():
    geometry, _clamped = _cantilever(along=4)
    shapes = _uniform(geometry, np.eye(3) * 1e-3)
    xyz = np.asarray(geometry.node_xyz)
    inside = int(geometry.node_id[np.flatnonzero((np.abs(xyz[:, 0] - 0.25) < 1e-9)
                                                 & (np.abs(xyz[:, 2]) < 1e-9)
                                                 & (np.abs(xyz[:, 1] - 0.005) < 1e-9))[0]])
    with pytest.raises(ValueError, match=f'node {inside} is inside the solid'):
        es.gauge_strains(geometry, shapes, [(inside, [1, 0, 0])])
    top = int(geometry.node_id[np.flatnonzero((np.abs(xyz[:, 0] - 0.25) < 1e-9)
                                              & (np.abs(xyz[:, 2] - 0.01) < 1e-9)
                                              & (np.abs(xyz[:, 1] - 0.005) < 1e-9))[0]])
    with pytest.raises(ValueError, match=f'the gauge at node {top} points 90 degrees out '
                                         'of the surface'):
        es.gauge_strains(geometry, shapes, [(top, [0, 0, 1])])
    with pytest.raises(ValueError, match=f'the gauge at node {top} has no direction'):
        es.gauge_strains(geometry, shapes, [(top, [0, 0, 0])])
    with pytest.raises(ValueError, match='the geometry has no node 99999'):
        es.gauge_strains(geometry, shapes, [(99999, [1, 0, 0])])
    loose = int(geometry.add_node([1.0, 1.0, 1.0]))
    with pytest.raises(ValueError, match=f'node {loose} is on no solid element'):
        es.gauge_strains(geometry, shapes, [(loose, [1, 0, 0])])
    with pytest.raises(ValueError, match='no gauges were given'):
        es.gauge_strains(geometry, shapes, [])
    with pytest.raises(ValueError, match='mode 2 is not in the shape set, which has 1'):
        es.gauge_strains(geometry, shapes, [(top, [1, 0, 0])], modes=[1])
    holed = ShapeSet(shapes.frequency, shapes.damping, shapes.coordinate,
                     shapes.shape_matrix.copy())
    holed.shape_matrix[0, :] = np.nan
    with pytest.raises(ValueError, match='mode 1 has no value at'):
        es.gauge_strains(geometry, holed, [(top, [1, 0, 0])])


def _project():
    project = hertzforge.Project('p')
    project.add('Cells', _cells(along=4))
    modes = project.solve_modes('Cells', num_modes=10)
    return project, modes


def test_a_tables_strain_channels_are_read_on_the_model():
    """Same geometry: each strain channel at its own node; the acceleration
    beside it is no strain channel and is not read."""
    project, modes = _project()
    geometry = project['Cells']
    project.add('Channels', hertzforge.ChannelTable({
        'channel': [1, 2, 3], 'node': ['3', '3', '6'], 'direction': ['X+', 'X+', 'Y-'],
        'role': ['response'] * 3, 'unit': ['', 'm/s**2', ''],
        'channel_type': ['strain', 'acceleration', 'strain']}))
    project.link('Cells', 'Channels')
    values, read, report = project.gauge_strains(modes, 'Channels', modes=[6, 7])
    assert read == ['3X+', '6Y-'] and report == {'nodes': [3, 6], 'dropped': []}
    direct = es.gauge_strains(geometry, project[modes], [(3, [1, 0, 0]), (6, [0, -1, 0])],
                              modes=[6, 7])
    assert np.allclose(values, direct, rtol=1e-14, atol=0)
    with pytest.raises(ValueError, match='none of the picked channels measures strain'):
        project.gauge_strains(modes, 'Channels', channels=[1])
    assert project.gauge_strains(modes, 'Channels', channels=[2])[1] == ['6Y-']


def test_on_a_test_geometry_a_channel_takes_the_models_nearest_node():
    project, modes = _project()
    model = project['Cells']
    test = hertzforge.Geometry(node_id=[101, 102],
                               node_xyz=[model.node_xyz[2] + [0, 0, 1e-5], [5.0, 5.0, 5.0]])
    test.length_unit = 'm'
    project.add('Test', test)
    project.add('Channels', hertzforge.ChannelTable({
        'channel': [1, 2], 'node': ['101', '102'], 'direction': ['X+', 'X+'],
        'role': ['response'] * 2, 'unit': ['', ''], 'channel_type': ['strain', 'strain']}))
    project.link('Test', 'Channels')
    values, read, report = project.gauge_strains(modes, 'Channels', modes=[6])
    assert read == ['101X+'] and report['dropped'] == ['102X+']
    assert report['nodes'] == [int(model.node_id[2])]
    direct = es.gauge_strains(model, project[modes], [(int(model.node_id[2]), [1, 0, 0])],
                              modes=[6])
    assert np.allclose(values, direct, rtol=1e-14, atol=0)


def test_expand_answers_a_strain_channel_with_its_strain_per_force():
    """Modal FRFs at a table with a force, an acceleration and a strain at
    one node and direction: the strain row is the gauge's strain in each mode
    times the drive point's shape and the mode's FRF, a displacement's power
    of i omega, and the acceleration row is unchanged by its being there."""
    project, modes = _project()
    shapes = project[modes]
    project.add('Channels', hertzforge.ChannelTable({
        'channel': [1, 2, 3], 'node': ['8', '3', '3'], 'direction': ['Z+', 'X+', 'X+'],
        'role': ['reference', 'response', 'response'], 'unit': ['N', 'm/s**2', ''],
        'channel_type': ['force', 'acceleration', 'strain']}))
    project.link('Cells', 'Channels')
    modal = project.synthesize_modal_frfs(modes, sample_rate=40000.0, frame_length=256,
                                          quantity='acceleration')
    frfs = project[project.expand(modal, 'Channels')]
    assert frfs.response_dof == ['3X+', '3X+']
    assert frfs.ordinate_dim == ['acceleration/force', 'dimensionless/force']
    f = frfs.abscissa[1:]
    assert np.allclose(frfs.ordinate[0, 1:], shapes.synthesize_frf(
        f, ['3X+'], ['8Z+'], power=2)[0], rtol=1e-10, atol=0)
    strain = es.gauge_strains(project['Cells'], shapes, [(3, [1, 0, 0])])[:, 0]
    # the same modal sum, the gauge's strains standing where a shape's row would
    gauge = ShapeSet(shapes.frequency, shapes.damping, ['3X+', '8Z+'],
                     np.column_stack([strain, shapes.shape_matrix[
                         :, shapes.coordinate.index('8Z+')]]))
    want = gauge.synthesize_frf(f, ['3X+'], ['8Z+'], power=0)[0]
    assert np.allclose(frfs.ordinate[1, 1:], want, rtol=1e-10, atol=0)


def test_project_onto_a_table_leaves_a_strain_channel_out_and_says_so():
    """A shape set holds displacements: a strain channel's node and direction
    given a displacement would read as a gauge that is not one. One sharing
    its node and direction with an acceleration leaves that DOF in."""
    project, modes = _project()
    project.add('Channels', hertzforge.ChannelTable({
        'channel': [1, 2, 3, 4], 'node': ['3', '3', '6', '7'],
        'direction': ['X+', 'X+', 'Y+', 'Z+'], 'role': ['response'] * 4,
        'unit': ['m/s**2', '', '', 'm/s**2'],
        'channel_type': ['acceleration', 'strain', 'strain', 'acceleration']}))
    project.link('Cells', 'Channels')
    added = project.project_onto_basis(modes, onto='Channels')
    assert project[added].coordinate == ['3X+', '7Z+']
    assert project[added].projection_report['strain'] == ['6Y+']
    with pytest.raises(ValueError, match='the channels of Channels picked all measure strain'):
        project.project_onto_basis(modes, onto='Channels', channels=[2])
