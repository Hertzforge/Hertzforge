"""The iterative modal solve: LOBPCG preconditioned by smoothed-aggregation
multigrid, for a model whose factor will not fit in memory.

Each test makes the factor "too large" by having `sparse_solve.factor_memory`
report a petabyte, so the direct solve refuses it before factoring and the
model goes to the iterative solve, which checks its own memory against the
machine's."""

import inspect

import numpy as np
import pytest

from hertzforge.core import fem, iterative_modes, sparse_solve
from hertzforge.core.iterative_modes import (
    ITERATIVE_ACCEPT,
    ITERATIVE_RESIDUAL,
    ROUND_ITERATIONS,
    STALL_ROUNDS,
    STILL_NOTE,
    IterativeStalled,
    IterativeTooLarge,
    iterative_residual,
)
from hertzforge.core.progress import Cancelled
from hertzforge.project import Project

ALUMINUM = fem.material('6061-T6')
PETABYTE = 10**15


def _block(nx=20, ny=12, nz=8):
    """A free block of nx x ny x nz aluminum bricks 10 mm on a side."""
    model = fem.Model('block')

    def node(i, j, k):
        return 1 + i + (nx + 1) * (j + (ny + 1) * k)

    for k in range(nz + 1):
        for j in range(ny + 1):
            for i in range(nx + 1):
                model.add_node(node(i, j, k), 0.01 * i, 0.01 * j, 0.01 * k)
    for k in range(nz):
        for j in range(ny):
            for i in range(nx):
                model.add_solid([node(i, j, k), node(i + 1, j, k), node(i + 1, j + 1, k),
                                 node(i, j + 1, k), node(i, j, k + 1), node(i + 1, j, k + 1),
                                 node(i + 1, j + 1, k + 1), node(i, j + 1, k + 1)], ALUMINUM)
    return model


def _plate(nx, ny, thickness):
    """A free flat plate of nx x ny quads 10 mm on a side."""
    model = fem.Model('plate')

    def node(i, j):
        return 1 + i + (nx + 1) * j

    for j in range(ny + 1):
        for i in range(nx + 1):
            model.add_node(node(i, j), 0.01 * i, 0.01 * j, 0.0)
    for j in range(ny):
        for i in range(nx):
            model.add_plate([node(i, j), node(i + 1, j), node(i + 1, j + 1), node(i, j + 1)],
                            ALUMINUM, thickness)
    return model


@pytest.fixture
def unfactorable(monkeypatch):
    """A switch: once called, every factor is reported a petabyte, and the
    factorizations still made are recorded in the list it returns."""
    factored = []

    def refuse():
        monkeypatch.setattr(sparse_solve, 'factor_memory',
                            lambda matrix: (PETABYTE, 'accelerate'))
        for name in ('AccelerateFactor', 'PardisoFactor', 'SuperLUFactor'):
            monkeypatch.setattr(sparse_solve, name, lambda matrix: factored.append(matrix))
        return factored

    return refuse


def test_a_block_too_large_to_factor_solves_to_the_direct_frequencies(unfactorable):
    """Under the default settings, the same modes as the direct solve:
    six rigid ones at exactly 0 Hz, the elastic frequencies well within
    the iterative tolerance, mass-normal shapes, and each mode's residual
    in its comment, where the direct solve's say nothing of the kind."""
    model = _block()
    assert model.num_dof > fem.SPARSE_ABOVE
    direct = model.eigensolution(num_modes=20)
    factored = unfactorable()
    shapes = model.eigensolution(num_modes=20)
    assert factored == [], 'nothing factored'
    assert shapes.num_shapes == 20
    assert np.all(shapes.frequency[:6] == 0.0) and np.all(shapes.frequency[6:] > 1000.0)
    elastic = direct.frequency[6:]
    assert np.max(np.abs(shapes.frequency[6:] - elastic) / elastic) < 1e-9
    mass, _stiffness = model.sparse_matrices()
    phi = shapes.shape_matrix.T
    assert np.max(np.abs(phi.T @ (mass @ phi) - np.eye(20))) < 1e-8

    read = [iterative_residual(comment) for comment in shapes.comment]
    assert read[:6] == ['strains nothing'] * 6
    assert all(shapes.comment[k].endswith(STILL_NOTE) for k in range(6))
    assert all(isinstance(value, float) and 0.0 < value < ITERATIVE_RESIDUAL
               for value in read[6:]), read
    assert [iterative_residual(comment) for comment in direct.comment] == [None] * 20


def test_modes_found_a_batch_at_a_time_are_the_direct_modes(unfactorable, monkeypatch):
    """Fourteen elastic modes of a block of square section, five at a
    time: each solve is kept orthogonal to the modes before it, so none is
    found twice and none skipped, the bending pair split between the first
    two solves included."""
    model = _block(12, 12, 10)
    direct = model.eigensolution(num_modes=20)
    elastic = direct.frequency[6:]
    assert elastic[5] - elastic[4] < 1e-9 * elastic[4], 'elastic modes 5 and 6 are a pair'
    unfactorable()
    monkeypatch.setattr(iterative_modes.IterativeSolver, 'check_memory', lambda self, count: 5)
    shapes = model.eigensolution(num_modes=20)
    assert np.max(np.abs(shapes.frequency[6:] - elastic) / elastic) < 1e-9
    mass, _stiffness = model.sparse_matrices()
    phi = shapes.shape_matrix.T
    assert np.max(np.abs(phi.T @ (mass @ phi) - np.eye(20))) < 1e-8


def test_the_widest_batch_that_fits_is_taken(unfactorable, monkeypatch):
    """Twenty-four elastic modes in one solve where they fit, else in
    batches of 16, else of 8, as the memory estimate allows. On a model
    this small the work after the solve outweighs any batch, so it is set
    aside here and the batches are what the estimate weighs."""
    unfactorable()
    monkeypatch.setattr(iterative_modes, 'ELEMENT_WORK', 0)
    monkeypatch.setattr(iterative_modes, 'FULL_COPIES', 0)
    chosen = []
    real = iterative_modes.IterativeSolver.check_memory

    def spy(self, count):
        need = {batch: iterative_modes.iterative_memory(self.held, self.shifted, count, 6,
                                                        self.full_dof, batch)
                for batch in (24, 16, 8)}
        assert need[24] > need[16] > need[8]
        for room, batch in ((need[24], 24), (need[16], 16), (need[8], 8)):
            monkeypatch.setattr(sparse_solve, 'physical_memory',
                                lambda room=room: room / sparse_solve.MEMORY_SHARE)
            chosen.append((real(self, count), batch))
        return real(self, count)

    monkeypatch.setattr(iterative_modes.IterativeSolver, 'check_memory', spy)
    assert _block().eigensolution(num_modes=30).num_shapes == 30
    assert [found for found, _batch in chosen] == [batch for _found, batch in chosen]


def test_every_mode_up_to_a_frequency_is_found_past_the_first_block(unfactorable):
    """More modes below the frequency than the first block of 24: the
    solve grows the block from the modes it found until a converged one
    clears the frequency, and finds the direct solve's set."""
    model = _block()
    direct = model.eigensolution(maximum_frequency=26000.0)
    assert direct.num_shapes > 24, 'the first block falls short'
    factored = unfactorable()
    shapes = model.eigensolution(maximum_frequency=26000.0)
    assert factored == []
    assert shapes.num_shapes == direct.num_shapes
    assert np.max(np.abs(shapes.frequency - direct.frequency)) < 1e-6 * direct.frequency[-1]


def test_more_modes_than_the_iterative_limit_are_refused_by_name(unfactorable, monkeypatch):
    unfactorable()
    monkeypatch.setattr(iterative_modes, 'ITERATIVE_MODE_LIMIT', 12)
    model = _block()
    with pytest.raises(ValueError, match='13 modes were asked for') as asked:
        model.eigensolution(num_modes=13)
    assert 'finds at most 12: ask for 12 or fewer' in str(asked.value)
    with pytest.raises(ValueError, match='is more than the 12 modes the iterative solve finds'):
        model.eigensolution(maximum_frequency=26000.0)
    assert model.eigensolution(num_modes=12).num_shapes == 12


def test_a_thin_plate_stalls_and_is_refused_with_its_cause(unfactorable):
    """Multigrid built for elasticity does not move the residuals of a
    free plate 0.2 mm thick on 10 mm elements off one: the stall is caught
    after `STALL_ROUNDS` rounds, not run out to the iteration limit, and
    refused with the residual, the reason and the shell elements named."""
    unfactorable()
    model = _plate(30, 20, 0.0002)
    with pytest.raises(IterativeStalled) as stalled:
        model.eigensolution(num_modes=10)
    said = str(stalled.value)
    assert stalled.value.worst > ITERATIVE_ACCEPT
    # six rounds: the first sets the best, five make no progress on it; each
    # applies the preconditioner once more than its iterations
    assert stalled.value.iterations == (STALL_ROUNDS + 1) * (ROUND_ITERATIONS + 1)
    assert 'its residuals stopped falling' in said
    assert f'at a residual of {stalled.value.worst:.1e}' in said
    assert f'short of the {ITERATIVE_ACCEPT:.0e} a mode must reach' in said
    assert 'this model has 600 shell elements' in said
    assert 'factoring this model of' in said, 'the reason it was solved this way'


def test_a_solve_that_will_not_fit_either_is_refused_with_both_sizes(unfactorable, monkeypatch):
    """With too little memory for the iterative solve as well, the refusal
    names both, before the multigrid hierarchy is built."""
    import pyamg

    factored = unfactorable()
    built = []
    monkeypatch.setattr(pyamg, 'smoothed_aggregation_solver',
                        lambda *args, **kwargs: built.append(1))
    monkeypatch.setattr(sparse_solve, 'physical_memory', lambda: 32 * 2**20)
    model = _block()
    with pytest.raises(IterativeTooLarge) as refused:
        model.eigensolution(num_modes=20)
    assert isinstance(refused.value, sparse_solve.FactorTooLarge)
    assert refused.value.needed > sparse_solve.MEMORY_SHARE * 32 * 2**20
    said = str(refused.value)
    assert f'factoring it would need about {sparse_solve._gigabytes(PETABYTE)} of memory' in said
    assert f'about {sparse_solve._gigabytes(refused.value.needed)} for its matrices' in said
    assert 'its 0.031 GB of physical memory' in said
    assert built == [] and factored == []


def test_a_cancel_stops_the_iterative_solve_between_iterations(unfactorable):
    """The progress callback is told of every iteration from inside LOBPCG,
    so a cancel there stops the solve while it iterates."""
    unfactorable()
    seen = []

    def progress(done, total):
        if any(frame.function == 'lobpcg' for frame in inspect.stack()):
            seen.append(done)
            if len(seen) == 3:
                raise Cancelled

    with pytest.raises(Cancelled):
        _block().eigensolution(num_modes=20, progress=progress)
    assert len(seen) == 3


def test_the_bar_counts_the_modes_through_every_batch(unfactorable, monkeypatch):
    """The bar moved to three quarters in the first round of iterations
    and stood there for the rest of a solve of 30 modes of 2.5 million
    DOF, which read as frozen: the stage's estimate was one round, and
    the iterative solve, which has no polish, left the polish's quarter
    of the bar to the end. Now each batch starts where the modes found
    before it put the bar, moves on every iteration, and the solve's
    share runs from the assembly's end to the end of the bar."""
    unfactorable()
    monkeypatch.setattr(iterative_modes.IterativeSolver, 'check_memory', lambda self, count: 5)
    heard = []
    starts = []
    rounds = iterative_modes.IterativeSolver._rounds

    def spy(self, *args):
        # the bar's own count: what it reports is held to one report each 0.05 s
        bar = self.ticker.parent
        starts.append(bar.done / bar.total)
        return rounds(self, *args)

    monkeypatch.setattr(iterative_modes.IterativeSolver, '_rounds', spy)

    def progress(done, total):
        if total > 0:
            heard.append(done / total)

    _block().eigensolution(num_modes=21, progress=progress)
    assert heard == sorted(heard), 'the bar never goes back'
    assert len(starts) == 3, '15 elastic modes in batches of 5'
    assembly = fem.SOLVE_BAR['assembly'] / sum(fem.SOLVE_BAR.values())
    expected = [assembly + (1 - assembly) * k / 3 for k in range(3)]
    assert np.allclose(starts, expected, atol=0.005), (starts, expected)
    moving = {f for f in heard if starts[-1] < f < 1.0}
    assert len(moving) > 3, 'it moves through the last batch'


def test_solve_modes_reads_out_an_iterative_solve_and_its_residuals(unfactorable):
    """The app's Solve Modes says the modes came from the iterative solve
    and how far each converged, and the mode table carries a Residual
    column; a direct solve has neither."""
    from test_solve_modes import _plate_geometry

    from hertzforge.service.ops_project import OPERATIONS
    from hertzforge.service.ops_tables import _table

    unfactorable()
    project = Project()
    project.add('Skin', _plate_geometry(mesh=24))
    op = next(o for o in OPERATIONS if o.id == 'project.solve_modes')
    values = op.resolve({'source': 'Skin', 'num_modes': 10}, objects={'Skin': 'geometry'},
                        verbs={'Skin': ['solve_modes']})
    views = op.run(project, values)
    assert [view['view'] for view in views] == ['shape3d', 'reading']
    reading = views[1]
    assert reading['title'] == 'Skin Modes: solved iteratively'
    assert 'come from the iterative solve' in reading['rows'][0]['value']
    residuals = [row for row in reading['rows'] if row['label'].endswith('residual')]
    assert [row['label'] for row in residuals] == [f'Mode {k} residual' for k in range(1, 11)]
    assert [row['value'] for row in residuals[:6]] == ['none: a motion that strains nothing'] * 6
    shapes = project['Skin Modes']
    assert [row['value'] for row in residuals[6:]] == [
        iterative_residual(comment) for comment in shapes.comment[6:]]
    assert all(row['status'] == 'pass' for row in residuals[6:])
    worst = max(row['value'] for row in residuals[6:])
    assert reading['rows'][0]['value'].endswith(f'The worst of these is at {worst:.1e}')
    assert f'to a relative residual of {ITERATIVE_RESIDUAL:.0e}' in reading['rows'][0]['value']
    grid = _table(project, 'Skin Modes')
    assert grid['columns'][-1] == 'Residual'
    assert [row[-1] for row in grid['rows']] == (
        ['strains nothing'] * 6 + [f'{iterative_residual(c):.1e}' for c in shapes.comment[6:]])
    assert grid['edit']['columns'] == [None, None, None, 'description', None]



def test_a_direct_solve_has_no_iterative_reading_and_no_residual_column():
    from test_solve_modes import _plate_geometry

    from hertzforge.service.ops_project import OPERATIONS
    from hertzforge.service.ops_tables import _table

    project = Project()
    project.add('Skin', _plate_geometry(mesh=24))
    op = next(o for o in OPERATIONS if o.id == 'project.solve_modes')
    values = op.resolve({'source': 'Skin', 'num_modes': 10}, objects={'Skin': 'geometry'},
                        verbs={'Skin': ['solve_modes']})
    assert [view['view'] for view in op.run(project, values)] == ['shape3d']
    grid = _table(project, 'Skin Modes')
    assert grid['columns'] == ['Mode', 'Frequency [Hz]', 'Damping [%]', 'Description']
    assert grid['edit']['columns'] == [None, None, None, 'description']
