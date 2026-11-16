"""Modal FRFs moving a geometry: the operating deflection shape of a
force at one DOF, from each mode's modal FRF and its shape.

The 0 Hz shape of a free-free finite element model under a force at one
point, with the rigid-body modes left out, used to need a channel table
listing every DOF of the model. Modal FRFs, their shape set and a drive
DOF are everything it needs (Brandon, 2026-11-16). Checked against
the package's own physical synthesis at every DOF, which is checked
against the direct inverse of the dynamic stiffness
(`test_modal_frf.py`).
"""

from __future__ import annotations

import numpy as np
import pytest

import hertzforge
from hertzforge.core.modal_frf import driven_modes, synthesize_modal_frfs
from hertzforge.core.shapes import ShapeSet
from hertzforge.deform import (
    ModalOdsDeflection,
    OdsDeflection,
    modal_ods_deflection,
)
from hertzforge.demo import plate

DRIVE = '101Z+'
POWER = {'displacement': 0, 'velocity': 1, 'acceleration': 2}


@pytest.fixture(scope='module')
def model():
    """A free plate: its geometry and its first twenty modes, six of
    them rigid, damped so the response has phase."""
    built = plate.build(4)
    solved = built.eigensolution(num_modes=20)
    shapes = ShapeSet(solved.frequency, np.full(solved.num_shapes, 0.02),
                      solved.coordinate, solved.shape_matrix,
                      modal_mass=solved.modal_mass, mass_unit='kg')
    return built.geometry(), shapes


def _elastic(shapes):
    return [k for k in range(shapes.num_shapes) if shapes.frequency[k] > 1.0]


def _translations(shapes):
    return [dof for dof in shapes.coordinate if 'R' not in dof]


def _motion(shapes, driven):
    """Φq at every DOF of the set: DOFs × lines."""
    return np.asarray(shapes.shape_matrix)[driven.modes].T @ driven.coordinates


# ---- the calculation ----------------------------------------------------------


@pytest.mark.parametrize('quantity', list(POWER))
def test_through_the_shapes_it_is_the_frf_at_every_dof(model, quantity):
    """The modal coordinates per unit force, times the shapes, are the
    FRFs from the drive to every DOF the set covers."""
    _geometry, shapes = model
    modal = synthesize_modal_frfs(shapes, 4096.0, 256, quantity)
    driven = driven_modes(modal, shapes, DRIVE)
    assert driven.drive == DRIVE and driven.modes == list(range(20))
    assert driven.coordinates.shape == (20, 129)
    assert driven.quantity == ('length' if quantity == 'displacement'
                               else quantity)
    dofs = list(shapes.coordinate)
    truth = shapes.synthesize_frf(modal.abscissa[1:], dofs,
                                  [DRIVE] * len(dofs), power=POWER[quantity])
    # the in-plane DOFs answer a force through the thickness with
    # rounding alone, so the floor is set against the largest response
    assert np.allclose(_motion(shapes, driven)[:, 1:], truth,
                       rtol=1e-9, atol=1e-9 * np.abs(truth).max())


def test_a_force_the_other_way_moves_it_the_other_way(model):
    _geometry, shapes = model
    modal = synthesize_modal_frfs(shapes, 4096.0, 64)
    forward = driven_modes(modal, shapes, '101Z+').coordinates
    backward = driven_modes(modal, shapes, '101Z-').coordinates
    assert np.abs(forward).max() > 0
    assert np.array_equal(backward, -forward)


def test_a_modes_sign_and_scale_do_not_change_the_motion(model):
    """A shape turned over, or scaled with its modal mass, is the same
    mode, and the motion it gives a force is the same."""
    _geometry, shapes = model
    factors = np.linspace(-3.0, 2.0, shapes.num_shapes)
    assert not np.any(factors == 0.0)
    other = ShapeSet(shapes.frequency, shapes.damping, shapes.coordinate,
                     np.asarray(shapes.shape_matrix) * factors[:, None],
                     modal_mass=np.asarray(shapes.modal_mass) * factors ** 2,
                     mass_unit='kg')
    ours = _motion(shapes, driven_modes(
        synthesize_modal_frfs(shapes, 4096.0, 64), shapes, DRIVE))
    theirs = _motion(other, driven_modes(
        synthesize_modal_frfs(other, 4096.0, 64), other, DRIVE))
    finite = np.isfinite(ours)
    assert np.array_equal(finite, np.isfinite(theirs))
    assert np.allclose(theirs[finite], ours[finite], rtol=1e-9,
                       atol=1e-9 * np.abs(ours[finite]).max())


def test_without_the_rigid_modes_zero_hertz_is_the_elastic_deflection(model):
    """The case it was asked for: displacement at 0 Hz is unbounded with the
    rigid-body modes in and the elastic static flexibility without."""
    _geometry, shapes = model
    modal = synthesize_modal_frfs(shapes, 4096.0, 64, 'displacement')
    whole = driven_modes(modal, shapes, DRIVE)
    assert np.isnan(whole.coordinates[:6, 0]).all()
    elastic = _elastic(shapes)
    assert elastic == list(range(6, 20))
    driven = driven_modes(modal, shapes, DRIVE, records=elastic)
    assert driven.modes == elastic
    phi = np.asarray(shapes.shape_matrix)[elastic]
    at_drive = phi[:, shapes.coordinate.index(DRIVE)]
    stiffness = (np.asarray(shapes.modal_mass)[elastic]
                 * (2 * np.pi * shapes.frequency[elastic]) ** 2)
    static = phi.T @ (at_drive / stiffness)
    assert np.abs(static).max() > 0
    assert np.allclose(_motion(shapes, driven)[:, 0], static,
                       rtol=1e-10, atol=1e-10 * np.abs(static).max())


def test_cross_terms_between_modes_are_carried():
    """Modal FRFs holding every mode against every mode, as a transform
    of a measured FRF gives: each coordinate answers every mode's modal
    force, not only its own."""
    rng = np.random.default_rng(7)
    shapes = ShapeSet([10.0, 20.0], [0.01, 0.01], ['1X+', '2X+'],
                      [[1.0, 2.0], [3.0, -1.0]], mass_unit='kg')
    matrix = rng.normal(size=(5, 2, 2)) + 1j * rng.normal(size=(5, 2, 2))
    pairs = [(r, s) for r in range(2) for s in range(2)]
    modal = hertzforge.Frf(
        np.arange(5.0), [matrix[:, r, s] for r, s in pairs],
        response_dof=[f'M{r + 1}' for r, _s in pairs],
        reference_dof=[f'M{s + 1}' for _r, s in pairs],
        ordinate_dim=['modal_acceleration/modal_force'] * 4)
    driven = driven_modes(modal, shapes, '2X+')
    expected = matrix @ np.array([2.0, -1.0])            # lines × modes
    assert np.abs(matrix[:, 0, 1]).min() > 0
    assert np.allclose(driven.coordinates, expected.T, rtol=1e-12, atol=0)


def test_what_cannot_be_driven_is_refused(model):
    _geometry, shapes = model
    modal = synthesize_modal_frfs(shapes, 4096.0, 64)
    with pytest.raises(ValueError, match='no DOF 999Z'):
        driven_modes(modal, shapes, '999Z+')
    few = ShapeSet(shapes.frequency[:3], shapes.damping[:3], shapes.coordinate,
                   np.asarray(shapes.shape_matrix)[:3], mass_unit='kg')
    with pytest.raises(ValueError, match='reach M20'):
        driven_modes(modal, few, DRIVE)
    short = ShapeSet(shapes.frequency[:19], shapes.damping[:19],
                     shapes.coordinate, np.asarray(shapes.shape_matrix)[:19],
                     mass_unit='kg')
    with pytest.raises(ValueError, match='the set has 19 modes, and the '
                                         'modal FRFs reach M20'):
        driven_modes(modal, short, DRIVE)
    with pytest.raises(ValueError, match='the set has 19 modes, and the '
                                         'modal FRFs reach M20'):
        modal_ods_deflection(_geometry, short,
                             driven_modes(modal, shapes, DRIVE))
    twisted = ShapeSet(shapes.frequency, shapes.damping, shapes.coordinate,
                       np.asarray(shapes.shape_matrix) * (1 + 1j),
                       mass_unit='kg')
    with pytest.raises(ValueError, match='complex'):
        driven_modes(modal, twisted, DRIVE)


# ---- the deflection -----------------------------------------------------------


def _both(model, quantity='acceleration', records=None):
    geometry, shapes = model
    modal = synthesize_modal_frfs(shapes, 4096.0, 256, quantity)
    driven = driven_modes(modal, shapes, DRIVE, records)
    lazy = modal_ods_deflection(geometry, shapes, driven)
    dofs = _translations(shapes)
    full = OdsDeflection(geometry, dofs, np.nan_to_num(shapes.synthesize_frf(
        modal.abscissa, dofs, [DRIVE] * len(dofs), modes=driven.modes,
        power=POWER[quantity])))
    return lazy, full


def test_it_draws_what_the_frfs_at_every_dof_would(model):
    lazy, full = _both(model)
    assert isinstance(lazy, ModalOdsDeflection)
    assert lazy.num_lines == full.num_lines == 129
    assert lazy.line == 0, 'a new one is on its first line'
    assert np.array_equal(lazy.rows, full.rows)
    moved = 0.0
    for line in (1, 17, 64, 128):
        lazy.line = full.line = line
        assert lazy.line_defined
        for phase in (0.0, 0.7, 2.0):
            assert np.allclose(lazy.offsets(phase), full.offsets(phase),
                               rtol=1e-9, atol=1e-12)
            moved = max(moved, float(np.abs(lazy.offsets(phase)).max()))
    assert moved > 0.5, 'each line is drawn at full scale'
    assert lazy.peak == 1.0
    assert lazy.peak_magnitude == pytest.approx(full.peak_magnitude,
                                                rel=1e-9)


def test_the_cursor_starts_where_the_modes_reach_furthest(model):
    lazy, full = _both(model)
    assert abs(lazy.strongest_line - full.strongest_line) <= 1
    assert lazy.strongest_line > 0


def test_a_line_with_no_shape_is_drawn_at_rest(model):
    """Displacement at 0 Hz with the rigid-body modes in is not a
    number: nothing is drawn moved, and the deflection says so."""
    lazy, _full = _both(model, 'displacement')
    lazy.line = 0
    assert not lazy.line_defined
    assert not np.any(lazy.offsets(0.0))
    lazy.line = 5
    assert lazy.line_defined and np.any(lazy.offsets(0.0))
    assert lazy.strongest_line != 0
    assert np.isfinite(lazy.peak_magnitude) and lazy.peak_magnitude > 0


def test_the_elastic_static_shape_is_drawn_at_zero_hertz(model):
    _geometry, shapes = model
    lazy, full = _both(model, 'displacement', records=_elastic(shapes))
    lazy.line = full.line = 0
    assert lazy.line_defined
    assert np.abs(lazy.offsets(0.0)).max() > 0.5
    assert np.allclose(lazy.offsets(0.0), full.offsets(0.0),
                       rtol=1e-9, atol=1e-12)


def test_past_the_budget_the_strongest_lines_are_read(model, monkeypatch):
    """A large model does not make every line's pattern to find how far
    it moves: the strongest lines and each mode's own are enough."""
    lazy, _full = _both(model)
    exact = lazy.peak_magnitude
    monkeypatch.setattr(ModalOdsDeflection, 'EXACT_BUDGET', 1)
    monkeypatch.setattr(ModalOdsDeflection, 'STRONGEST_LINES', 4)
    surveyed = lazy._survey_lines()
    assert 4 <= len(surveyed) < lazy.num_lines
    assert set(np.argsort(lazy._reach())[-4:]) <= set(surveyed), \
        'the four strongest lines'
    own = np.abs(lazy._coordinates).argmax(axis=1)
    assert set(own) <= set(surveyed), "and each mode's own"
    assert len(surveyed) == len(set(np.argsort(lazy._reach())[-4:])
                                | set(own))
    # the budget counts DOFs × lines, and is met exactly
    values = lazy._phi.shape[0] * lazy.num_lines
    assert lazy._phi.shape[0] != lazy._phi.shape[1]
    monkeypatch.setattr(ModalOdsDeflection, 'EXACT_BUDGET', values)
    assert len(lazy._survey_lines()) == lazy.num_lines
    monkeypatch.setattr(ModalOdsDeflection, 'EXACT_BUDGET', values - 1)
    assert len(lazy._survey_lines()) < lazy.num_lines
    assert lazy.peak_magnitude == pytest.approx(exact, rel=0.05)


def test_a_set_that_is_nowhere_on_the_geometry_moves_nothing(model):
    geometry, shapes = model
    elsewhere = ShapeSet(shapes.frequency, shapes.damping,
                         [f'9{dof}' for dof in shapes.coordinate],
                         shapes.shape_matrix, mass_unit='kg')
    modal = synthesize_modal_frfs(elsewhere, 4096.0, 64)
    lazy = modal_ods_deflection(geometry, elsewhere,
                                driven_modes(modal, elsewhere, '9101Z+'))
    assert len(lazy.rows) == 0 and lazy.num_lines == 0 and lazy.peak == 0.0
    assert lazy.strongest_line == 0 and not lazy.line_defined


# ---- the project --------------------------------------------------------------


def _table(nodes=('101',), roles=('reference',), kinds=('force',)):
    count = len(nodes)
    return hertzforge.ChannelTable({
        'channel': list(range(1, count + 1)), 'node': list(nodes),
        'direction': ['Z+'] * count, 'role': list(roles),
        'unit': ['N' if kind == 'force' else 'g' for kind in kinds],
        'channel_type': list(kinds)})


def _project(model):
    geometry, shapes = model
    project = hertzforge.Project()
    project.add('Plate', geometry)
    project.add('Modes', shapes)
    project.link('Plate', 'Modes')
    modal = project.synthesize_modal_frfs('Modes', sample_rate=4096.0,
                                          frame_length=64,
                                          quantity='displacement')
    return project, modal


def test_a_tables_force_reference_is_the_drive(model):
    _geometry, shapes = model
    project, modal = _project(model)
    project.add('Force', _table())
    project.add('Mixed', _table(('102', '103', '103', '104'),
                                ('response', 'reference', 'reference',
                                 'reference'),
                                ('acceleration', 'force', 'force',
                                 'acceleration')))
    assert project.drive_points('Force') == ['101Z+']
    assert project.drive_points('Mixed') == ['103Z+'], \
        'a force, a reference, and each DOF once'
    with pytest.raises(TypeError, match='not a channel table'):
        project.drive_points('Modes')
    basis, driven = project.driven_modes(modal, table='Force')
    assert basis == 'Modes' and driven.drive == '101Z+'
    assert np.array_equal(
        driven.coordinates, driven_modes(project[modal], shapes,
                                         '101Z+').coordinates, equal_nan=True)
    _basis, named = project.driven_modes(modal, '103Z+',
                                         records=_elastic(shapes))
    assert named.drive == '103Z+' and named.modes == _elastic(shapes)


def test_the_project_refuses_what_has_no_drive(model):
    project, modal = _project(model)
    project.add('Quiet', _table(roles=('response',), kinds=('acceleration',)))
    project.add('Two', _table(('101', '102'), ('reference',) * 2,
                              ('force',) * 2))
    with pytest.raises(ValueError, match='no reference channel'):
        project.driven_modes(modal, table='Quiet')
    with pytest.raises(ValueError, match=r'2 force references \(101Z\+, 102Z\+\)'):
        project.driven_modes(modal, table='Two')
    _basis, picked = project.driven_modes(modal, '102Z+', table='Two')
    assert picked.drive == '102Z+', 'naming one settles it'
    with pytest.raises(ValueError, match='drive='):
        project.driven_modes(modal)
    with pytest.raises(TypeError, match='not an FRF'):
        project.driven_modes('Modes', '101Z+')
    orphan = project[modal]
    project.add('Orphan', hertzforge.Frf(
        orphan.abscissa, orphan.ordinate, response_dof=orphan.response_dof,
        reference_dof=orphan.reference_dof, ordinate_dim=orphan.ordinate_dim))
    with pytest.raises(ValueError, match='nothing says which shape set'):
        project.driven_modes('Orphan', '101Z+')


def test_a_force_on_a_test_geometry_drives_the_model_at_its_nearest_node():
    """FE modes on their own geometry and a force named in a test
    geometry's table: the shapes at the drive are the model's at the
    nearest node, as Expand to Physical Responses reads them."""
    from test_correlate import _channels, _fem, _test_model

    fem_geometry, fem_shapes = _fem()
    fem_shapes.define_units('kg')
    test_geometry, _shapes = _test_model()
    project = hertzforge.Project()
    project.add('FEM Geometry', fem_geometry)
    project.add('FEM Shapes', fem_shapes)
    project.add('Test Geometry', test_geometry)
    project.add('Channels', _channels())
    project.link('FEM Geometry', 'FEM Shapes')
    project.link('Test Geometry', 'Channels')
    modal = project.synthesize_modal_frfs('FEM Shapes', sample_rate=128.0,
                                          frame_length=128)
    assert project.drive_points('Channels') == ['102X-']
    basis, driven = project.driven_modes(modal, table='Channels')
    assert basis == 'FEM Shapes' and driven.drive == '102X-'
    projected = project[project.project_onto_basis('FEM Shapes',
                                                   onto='Channels')]
    truth = projected.synthesize_frf(driven.lines, ['101X+'], ['102X-'],
                                     power=2)[0]
    at_response = np.asarray(projected.shape_matrix)[
        :, projected.coordinate.index('101X+')]
    assert np.allclose(at_response[driven.modes] @ driven.coordinates, truth,
                       rtol=1e-12, atol=0)
    with pytest.raises(ValueError, match='within the match tolerance'):
        project.driven_modes(modal, table='Channels', tolerance=1e-6)
