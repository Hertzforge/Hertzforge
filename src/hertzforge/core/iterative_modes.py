"""The lowest modes of a model too large to factor, by LOBPCG preconditioned
with smoothed-aggregation algebraic multigrid.

The sparse modal solve factors (K - sigma M) once (`sparse_solve`), and a
factor grows faster than the model: a solid block of 807,003 degrees of
freedom needs about 53 GB for Accelerate's. A model whose factor will not
fit is solved here instead, with nothing larger than the matrices
themselves: LOBPCG (`scipy.sparse.linalg.lobpcg`, the locally optimal
block preconditioned conjugate gradient) iterates a block of vectors
toward the lowest modes of (K - sigma M) x = mu M x, and each iteration
applies a preconditioner, an approximate inverse of (K - sigma M), to
its residuals. The preconditioner is one V-cycle of smoothed-aggregation
algebraic multigrid (pyamg), whose coarse levels are built from the
model's rigid-body motions, the near-null space of elasticity.

The direct solve stays the default: its convergence does not depend on
what the elements are. Multigrid built for elasticity preconditions solid
elements well and thin shells, beams with rotations, rigid links and
springs poorly, so this one is used only when the factor will not fit.
It runs each mode to a relative residual (`ITERATIVE_RESIDUAL`), and a
solve that stalls short of `ITERATIVE_ACCEPT` is refused with the cause
named (`IterativeStalled`), never returned with modes that did not
converge. Each mode's residual is kept in its comment
(`iterative_comment`), where the mode table and Solve Modes read it.
"""

from __future__ import annotations

import itertools
import math
import os
import warnings
from collections.abc import Iterator
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from typing import Any

import numpy as np

from . import sparse_solve
from .sparse_solve import MEMORY_SHARE, FactorTooLarge, _gigabytes

#: the relative residual |K v - lambda M v| / |(K - sigma M) v| the
#: iterations run to, the one `fem.polish_modes` takes the direct solve's
#: modes to
ITERATIVE_RESIDUAL = 1e-8
#: the residual every mode asked for must be under for the solve to be
#: returned at all, when the iterations stall short of
#: `ITERATIVE_RESIDUAL`
ITERATIVE_ACCEPT = 1e-6
#: LOBPCG iterations in one round; between rounds the residuals are
#: measured, the stall test made and the tolerance set afresh
ROUND_ITERATIONS = 20
#: the iterations a solve may take in all
ITERATIVE_ITERATIONS = 1000
#: a round that does not cut the worst residual of the modes asked for
#: to this share of the best it had been is a round without progress,
#: and `STALL_ROUNDS` of them in a row are a stall. Five, because a free
#: plate 0.5 mm thick on 10 mm elements sat near a residual of one for
#: four rounds and then converged; one 0.2 mm thick never moved off it
STALL_SHARE = 0.5
STALL_ROUNDS = 5
#: modes converged by one LOBPCG solve, the widest that fits in memory:
#: every mode asked for at once, else each of these in turn. The next
#: solve is kept orthogonal to those found (LOBPCG's constraints), so its
#: blocks are a batch wide whatever the count; batches of 8 fit fifty
#: modes of a block of 2.5 million degrees of freedom in 20 GB, and cost
#: time where memory is not short (232 s against 148 s for fifty modes
#: of 807,003 degrees of freedom in one solve)
BATCHES = (16, 8)
#: vectors iterated beyond those a solve converges: the rate a mode
#: converges at is set by the gap from it to the first one outside the
#: block
ITERATIVE_GUARD = 10
#: the most modes the iterative solve finds, rigid ones included
ITERATIVE_MODE_LIMIT = 200
#: the multigrid hierarchy's bytes over those of (K - sigma M), held
#: beside it: 1.6 measured on a block of bricks
AMG_OVER_MATRIX = 2.0
#: blocks of a solve's width held at once: LOBPCG's X, R and P and their
#: products with K - sigma M and M, and the V-cycle's work in each thread
BLOCKS_HELD = 24
#: copies of the modes found held while a solve runs: the modes
#: themselves, their product with M that LOBPCG keeps for its
#: constraints, and the vectors returned
CONSTRAINT_COPIES = 3
#: copies of the modes at every degree of freedom the caller makes
#: after the solve (back to every DOF, turned into displacement frames,
#: signed, picked): 1.3 per mode measured, the strain energy summed a few
#: modes at a time
FULL_COPIES = 6
#: bytes of the element matrices the caller builds a few chunks at a time
#: after the solve, to tell which modes strain no element
ELEMENT_WORK = 2**29


class IterativeTooLarge(FactorTooLarge):
    """A model refused because neither its factor nor the iterative solve
    would fit in the machine's memory.

    Attributes
    ----------
    needed : int
        The bytes the iterative solve was estimated to need.
    factor : int
        The bytes the factor would have needed (`FactorTooLarge.needed`
        of the refusal that sent the model here).
    physical : int
        The machine's physical memory, in bytes.
    backend : str
        'lobpcg'.
    """

    def __init__(self, needed: int, factor: FactorTooLarge, size: int) -> None:
        self.needed, self.factor = needed, factor.needed
        self.physical, self.backend = factor.physical, 'lobpcg'
        ValueError.__init__(
            self,
            f'this model of {size:,} degrees of freedom does not fit in memory: '
            f'factoring it would need about {_gigabytes(factor.needed)} of memory, and the '
            f'iterative solve, which factors nothing, about {_gigabytes(needed)} for '
            'its matrices, multigrid hierarchy and blocks of vectors, both more than '
            f'the {_gigabytes(MEMORY_SHARE * factor.physical)} a solve may take on this '
            f'machine ({MEMORY_SHARE:.0%} of its {_gigabytes(factor.physical)} of '
            'physical memory); ask for fewer modes, mesh it coarser, or solve it on a '
            'machine with more memory')


class IterativeStalled(ValueError):
    """The iterative solve refused because its modes did not converge.

    Attributes
    ----------
    iterations : int
        The LOBPCG iterations taken.
    worst : float
        The largest residual among the modes asked for when it stopped.
    """

    def __init__(self, message: str, iterations: int, worst: float) -> None:
        super().__init__(message)
        self.iterations, self.worst = iterations, worst


#: what a mode's comment says after the model's name when the mode came
#: from the iterative solve, followed by its residual or `STILL_NOTE`
ITERATIVE_NOTE = 'iterative solve, relative residual '
#: what a mode's comment says in place of a residual when the mode is a
#: motion that strains nothing: its residual is round-off over a
#: denominator of nearly nothing, and says nothing about it
STILL_NOTE = 'iterative solve, a motion that strains nothing'


def iterative_comment(name: str, residual: float | None) -> str:
    """The comment a mode from the iterative solve carries: the model's
    name, then its relative residual, or `STILL_NOTE` for a mode that
    strains nothing (residual None)."""
    said = STILL_NOTE if residual is None else f'{ITERATIVE_NOTE}{residual:.1e}'
    return f'{name}; {said}' if name else said


def iterative_residual(comment: str) -> float | str | None:
    """What `iterative_comment` wrote in a mode's comment: its residual,
    'strains nothing', or None for a mode that did not come from the
    iterative solve."""
    if comment.endswith(STILL_NOTE):
        return 'strains nothing'
    at = comment.rfind(ITERATIVE_NOTE)
    if at < 0:
        return None
    try:
        return float(comment[at + len(ITERATIVE_NOTE):])
    except ValueError:
        return None


@contextmanager
def _all_blas_threads() -> Iterator[None]:
    """Every core for BLAS while LOBPCG runs: its dense steps are products
    of blocks with the model's whole size, where `fem.blas_threads` holds
    the sparse solve to a quarter of the cores for small ones."""
    from threadpoolctl import threadpool_limits

    with threadpool_limits(limits=os.cpu_count() or 1, user_api='blas'):
        yield


def _nbytes(matrix: Any) -> int:
    return int(sum(getattr(matrix, part).nbytes
                   for part in ('data', 'indices', 'indptr')))


def iterative_memory(held: int, shifted: Any, count: int, rigid: int, full_dof: int,
                     batch: int) -> int:
    """The bytes the iterative solve of `count` modes, `rigid` of them
    deflated, `batch` converged at a time, will take at its peak: `held`,
    the matrices the caller holds already, and the larger of two stages.
    While LOBPCG runs: the multigrid hierarchy of `shifted`, (K - sigma
    M), a batch's blocks and the modes found. After it: the copies of the
    modes at all `full_dof` degrees of freedom the caller makes, the
    element matrices it builds, and the absolute value of the stiffness,
    counted as large as what is held."""
    size = shifted.shape[0]
    width = min(batch, max(count - rigid, 1)) + ITERATIVE_GUARD
    solving = (AMG_OVER_MATRIX * _nbytes(shifted)
               + 8 * size * (BLOCKS_HELD * width + CONSTRAINT_COPIES * count))
    after = held + ELEMENT_WORK + 8 * count * (FULL_COPIES * full_dof + size)
    return int(held + max(solving, after))


def relative_residuals(shifted: Any, mass: Any, sigma: float, vectors: np.ndarray,
                       columns: Any = None) -> tuple[np.ndarray, np.ndarray]:
    """Each column's Rayleigh quotient and its relative residual
    |K v - lambda M v| / |(K - sigma M) v|, the measure `fem.polish_modes`
    converges the direct solve's modes to, from `shifted`, K - sigma M,
    and M; a slice of columns per thread when `columns` (`_Columns`) is
    given, so no product of the whole block is held."""
    def measured(part: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
        a_v = shifted @ part
        m_v = mass @ part
        # K v = (K - sigma M) v + sigma M v: K itself is not kept, being as
        # large as K - sigma M
        k_v = a_v + sigma * m_v
        values = np.einsum('ij,ij->j', part, k_v) / np.einsum('ij,ij->j', part, m_v)
        return values, (np.linalg.norm(k_v - m_v * values, axis=0)
                        / np.linalg.norm(a_v, axis=0))

    if columns is None:
        return measured(vectors)
    parts = columns.each(measured, vectors)
    return (np.concatenate([values for values, _r in parts]),
            np.concatenate([residuals for _v, residuals in parts]))


#: weighted Jacobi sweeps before and after each coarse correction of the
#: V-cycle
SMOOTHING_SWEEPS = 2
#: the largest coarsest level, solved by a dense Cholesky factor
COARSEST = 500


class _Columns:
    """A product taken a slice of columns per thread. scipy's sparse
    products and numpy's arithmetic on large arrays let go of the
    interpreter, so the slices run side by side; pyamg's Gauss-Seidel
    does not, and its V-cycles in threads ran one at a time (1.2 of 12
    cores busy on a block of 807,003 degrees of freedom)."""

    def __init__(self, pool: ThreadPoolExecutor, threads: int) -> None:
        self.pool, self.threads = pool, max(1, int(threads))

    def _slices(self, width: int) -> list[slice]:
        bounds = np.linspace(0, width, min(self.threads, width) + 1)
        return [slice(int(a), int(b)) for a, b in itertools.pairwise(bounds) if b > a]

    def apply(self, function: Any, block: np.ndarray) -> np.ndarray:
        """`function` of the block, a block of the same shape."""
        block = np.asarray(block)
        if block.ndim == 1:
            return function(block[:, None])[:, 0]
        slices = self._slices(block.shape[1])
        out = np.empty_like(block, dtype=float)
        for part, done in zip(slices, self.pool.map(lambda part: function(block[:, part]),
                                                     slices)):
            out[:, part] = done
        return out

    def each(self, function: Any, block: np.ndarray) -> list[Any]:
        """`function` of each slice of the block's columns, in order."""
        return list(self.pool.map(lambda part: function(block[:, part]),
                                  self._slices(block.shape[1])))


class _BlockCycle:
    """One V-cycle of a smoothed-aggregation hierarchy, applied to a block
    of columns at once: weighted Jacobi smoothing (`SMOOTHING_SWEEPS`
    before and after, so the cycle is symmetric, as LOBPCG wants of its
    preconditioner), the hierarchy's own restriction and prolongation,
    and a dense Cholesky factor on the coarsest level. Jacobi in place of
    pyamg's Gauss-Seidel because it is products alone, which run a block
    at a time and in threads (`_Columns`)."""

    def __init__(self, hierarchy: Any, sweeps: int = SMOOTHING_SWEEPS) -> None:
        import scipy.linalg
        from pyamg.relaxation.smoothing import rho_D_inv_A

        self.sweeps: int = int(sweeps)
        #: each level but the coarsest: its matrix, prolongation, restriction
        #: and Jacobi weight over its diagonal
        self.levels: list[tuple[Any, Any, Any, np.ndarray]] = []
        for level in hierarchy.levels[:-1]:
            # 4/3 over the spectral radius of D^-1 A, the weight that damps
            # the upper part of the spectrum best; pyamg has it cached from
            # smoothing the prolongation
            weight = (4.0 / 3.0) / rho_D_inv_A(level.A)
            scaled = (weight / level.A.diagonal())[:, None]
            self.levels.append((level.A, level.P, level.R, scaled))
        self.coarse: tuple[np.ndarray, bool] = scipy.linalg.cho_factor(hierarchy.levels[-1].A.toarray())

    def __call__(self, rhs: np.ndarray) -> np.ndarray:
        return self._cycle(0, rhs)

    def _cycle(self, depth: int, rhs: np.ndarray) -> np.ndarray:
        import scipy.linalg

        if depth == len(self.levels):
            return scipy.linalg.cho_solve(self.coarse, rhs)
        matrix, prolong, restrict, scaled = self.levels[depth]
        x = scaled * rhs
        for _sweep in range(self.sweeps - 1):
            x += scaled * (rhs - matrix @ x)
        x += prolong @ self._cycle(depth + 1, restrict @ (rhs - matrix @ x))
        for _sweep in range(self.sweeps):
            x += scaled * (rhs - matrix @ x)
        return x


class IterativeSolver:
    """LOBPCG on (K - sigma M) x = mu M x, preconditioned by one V-cycle of
    smoothed-aggregation multigrid, for a model too large to factor.

    The hierarchy is built once, here, and `modes` may be asked again for
    more modes, starting from the vectors found before.

    Parameters
    ----------
    shifted, mass : scipy.sparse matrix
        K - sigma M and M, constrained and symmetrically scaled, as
        `fem.Model.scaled_system` gives K and M; K itself is not needed.
    sigma : float
        The shift; K - sigma M is positive definite.
    near_null : numpy.ndarray
        (n, 6), the rigid-body motions in the same coordinates, which the
        multigrid's coarse levels are built to represent.
    deflated : numpy.ndarray
        (n, r), M-orthonormal motions that strain nothing (the rigid-body
        modes of a free model), kept out of the iterations and returned
        as modes of their own; (n, 0) when there are none.
    blocksize : int or None
        Degrees of freedom per node when every node has the same number,
        so the multigrid aggregates nodes rather than single degrees of
        freedom.
    held : int
        Bytes of the matrices the caller holds, for the memory check.
    full_dof : int
        The model's degrees of freedom before constraints, for the memory
        check of the copies the caller makes of the modes.
    refused : FactorTooLarge
        The refusal of the factor that sent the model here, quoted when
        this solve will not fit either.
    weak : str
        What in the model the multigrid preconditions poorly, in words
        ('412 plates and 6 springs'), for the message of a stall; empty
        when there is nothing of the kind.
    ticker : Ticker or Share, optional
        Told one tick per iteration; a callback that raises (a cancel)
        stops the solve between iterations.
    """

    def __init__(self, shifted: Any, mass: Any, sigma: float, near_null: np.ndarray,
                 deflated: np.ndarray, blocksize: int | None, held: int, full_dof: int,
                 refused: FactorTooLarge, weak: str = '', ticker: Any = None) -> None:
        self.shifted: Any = shifted.tocsr()
        self.mass, self.sigma = mass, float(sigma)
        self.deflated: np.ndarray = deflated
        self.blocksize, self.held, self.full_dof = blocksize, int(held), int(full_dof)
        self.refused, self.weak, self.ticker = refused, weak, ticker
        # LOBPCG's orthonormalization works in M's own scale, and the
        # scaled M of a stiff structure has a diagonal near 1e-12: it is
        # given M over its mean diagonal, which has the same modes with mu
        # scaled, applied as M and a division rather than held as a copy
        self.mass_scale: float = float(np.mean(mass.diagonal()))
        self.near_null: np.ndarray = near_null
        self.hierarchy: Any = None
        self.iterations: int = 0
        #: (iterations, worst residual of the modes asked for) after each round
        self.history: list[tuple[int, float]] = []
        # the modes converged so far, in LOBPCG's scale (normal in the
        # scaled M), the deflated ones first, column-major so the leading
        # columns are the next solve's constraints without a copy; their
        # eigenvalues and residuals; and the guard vectors of the last
        # solve, which the next starts from
        self.found: np.ndarray = np.asfortranarray(deflated * math.sqrt(self.mass_scale))
        self.values: list[float] = []
        self.residuals: list[float] = []
        self.guard: np.ndarray | None = None
        #: products taken a slice of columns per thread, while a solve runs
        self.columns: Any = None

    def check_memory(self, count: int) -> int:
        """The widest batch (`BATCHES`) a solve for `count` modes fits in
        beside what the caller holds; `IterativeTooLarge` when not even
        the narrowest does."""
        rigid = self.deflated.shape[1]
        wanted = max(int(count) - rigid, 1)
        # read through the module, so a test that sets the machine's
        # memory sets it for both solves
        room = MEMORY_SHARE * sparse_solve.physical_memory()
        for batch in dict.fromkeys(min(width, wanted) for width in (wanted, *BATCHES)):
            needed = iterative_memory(self.held, self.shifted, count, rigid, self.full_dof,
                                      batch)
            if needed <= room:
                return batch
        raise IterativeTooLarge(needed, self.refused, self.shifted.shape[0])

    def _build(self) -> None:
        import pyamg

        if self.hierarchy is not None:
            return
        if self.ticker is not None:
            self.ticker.busy()
        shifted = self.shifted
        b = self.blocksize
        if b and shifted.shape[0] % b == 0:
            shifted = shifted.tobsr(blocksize=(b, b))
        self.hierarchy = pyamg.smoothed_aggregation_solver(shifted, B=self.near_null,
                                                           max_coarse=COARSEST)
        self.cycle: _BlockCycle = _BlockCycle(self.hierarchy)
        # the finest level is K - sigma M, by nodes where it was given so:
        # the solve's products take it, and the caller's copy is let go
        self.shifted = self.hierarchy.levels[0].A

    def modes(self, count: int) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        """The lowest `count` modes, the deflated ones among them, found
        a batch at a time (`check_memory`), each solve kept orthogonal to the modes before
        it. A call for more modes than the last keeps those and finds the
        rest.

        Parameters
        ----------
        count : int
            How many modes must converge, rigid ones included.

        Returns
        -------
        (numpy.ndarray, numpy.ndarray, numpy.ndarray)
            The eigenvalues, the M-normal vectors and each one's relative
            residual (`relative_residuals`), `count` of them in ascending
            order.

        Raises
        ------
        IterativeTooLarge
            Before anything is built, when the solve would not fit.
        IterativeStalled
            When a mode asked for did not reach `ITERATIVE_ACCEPT`.
        """
        from scipy.sparse.linalg import LinearOperator, lobpcg

        batch = self.check_memory(count)
        size = self.shifted.shape[0]
        rigid = self.deflated.shape[1]
        wanted = max(int(count) - rigid, 0)
        if wanted and size - rigid - wanted < 5 * (min(batch, wanted) + ITERATIVE_GUARD):
            # scipy turns to a dense solve below this, and refuses one with
            # constraints; a model this small has a factor that fits
            raise ValueError(f'{count} modes of a model of {size:,} degrees of freedom '
                             'are too many for the iterative solve')
        threads = os.cpu_count() or 1
        with ThreadPoolExecutor(max_workers=threads) as pool, _all_blas_threads():
            self.columns = columns = _Columns(pool, threads)
            if len(self.values) < wanted:
                self._build()
                grown = np.empty((size, rigid + wanted), order='F')
                grown[:, :self.found.shape[1]] = self.found
                self.found = grown
            ticker = self.ticker
            cycle = self.cycle if wanted else None

            def precondition(residual: np.ndarray) -> np.ndarray:
                # one call per iteration, from LOBPCG's own thread: the
                # tick is where a cancel stops the solve
                self.iterations += 1
                if ticker is not None:
                    ticker.tick()
                return columns.apply(cycle, residual)

            def operator(function: Any) -> Any:
                return LinearOperator((size, size), matvec=function, matmat=function,
                                      dtype=float)

            shifted = operator(lambda block: columns.apply(lambda part: self.shifted @ part,
                                                           block))
            scaled_mass = operator(lambda block: columns.apply(
                lambda part: (self.mass @ part) / self.mass_scale, block))
            rng = np.random.default_rng(len(self.values))
            while len(self.values) < wanted:
                done = len(self.values)
                take = min(batch, wanted - done)
                block = rng.standard_normal((size, take + ITERATIVE_GUARD))
                if self.guard is not None:
                    kept = min(self.guard.shape[1], block.shape[1])
                    block[:, :kept] = self.guard[:, :kept]
                constraints = self.found[:, :rigid + done] if rigid + done else None
                values, residuals, block = self._rounds(
                    lobpcg, shifted, scaled_mass, operator(precondition), block,
                    constraints, take, rigid + done)
                self.found[:, rigid + done:rigid + done + take] = block[:, :take]
                self.values.extend(float(value) for value in values[:take])
                self.residuals.extend(float(value) for value in residuals[:take])
                self.guard = block[:, take:]
            # LOBPCG's vectors are normal in the scaled M; these are in M
            vectors = self.found[:, :rigid + wanted] / math.sqrt(self.mass_scale)
            fixed_values, fixed_residuals = relative_residuals(
                self.shifted, self.mass, self.sigma, vectors[:, :rigid], columns)
            self.columns = None
        values = np.concatenate([fixed_values, self.values[:wanted]])
        residuals = np.concatenate([fixed_residuals, self.residuals[:wanted]])
        order = np.argsort(values, kind='stable')
        return values[order], vectors[:, order], residuals[order]

    def _rounds(self, lobpcg: Any, shifted: Any, scaled_mass: Any, operator: Any,
                block: np.ndarray, constraints: np.ndarray | None, wanted: int,
                first: int) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        """Run LOBPCG a round at a time until the `wanted` lowest of the
        block, modes `first` + 1 on, converge, stall, or
        `ITERATIVE_ITERATIONS` are spent; the block comes back in
        ascending order, in LOBPCG's scale."""
        best = math.inf
        idle = 0
        # scipy reads a tolerance of zero as its own default, which a
        # block of small residuals meets at once: the first round runs whole
        tolerance = 1e-300
        failure = ''
        started = self.iterations
        while True:
            if self.ticker is not None:
                self.ticker.extend_to(ROUND_ITERATIONS)
            with warnings.catch_warnings(record=True) as said:
                warnings.simplefilter('always')
                try:
                    _mu, block = lobpcg(shifted, block, B=scaled_mass,
                                        M=operator, Y=constraints, tol=tolerance,
                                        maxiter=ROUND_ITERATIONS, largest=False)
                except (ValueError, np.linalg.LinAlgError) as error:
                    failure = str(error)
            values, residuals = relative_residuals(self.shifted, self.mass,
                                                   self.sigma, block, self.columns)
            order = np.argsort(values, kind='stable')
            values, residuals, block = values[order], residuals[order], block[:, order]
            worst = float(residuals[:wanted].max())
            self.history.append((self.iterations, worst))
            failed = failure or next((str(w.message) for w in said
                                      if str(w.message).startswith('Failed at')), '')
            if worst < ITERATIVE_RESIDUAL or failure:
                break
            idle = idle + 1 if worst > STALL_SHARE * best else 0
            best = min(best, worst)
            if idle >= STALL_ROUNDS or failed or self.iterations - started >= ITERATIVE_ITERATIONS:
                break
            # the tolerance LOBPCG drops a converged vector at: its
            # residual is absolute, of an M-normal vector, and the
            # relative one asked for is that over |(K - sigma M) x|
            reach = np.concatenate(self.columns.each(
                lambda part: np.linalg.norm(self.shifted @ part, axis=0), block[:, :wanted]))
            tolerance = ITERATIVE_RESIDUAL * float(reach.min())
        if not worst < ITERATIVE_ACCEPT:
            raise IterativeStalled(self._stall_message(first, wanted, worst, failed),
                                   self.iterations - started, worst)
        return values, residuals, block

    def _stall_message(self, first: int, wanted: int, worst: float, failed: str) -> str:
        which = (f'mode {first + 1}' if wanted == 1 else
                 f'modes {first + 1} to {first + wanted}')
        what = (f'this model has {self.weak}' if self.weak else
                'this model has none of them: look for parts much stiffer or '
                'softer than the rest, or held by a single node')
        how = (f'LOBPCG broke down ({failed.rstrip(". ")})' if failed else
               'its residuals stopped falling')
        return (f'the iterative solve did not converge: {how} after '
                f'{self.iterations:,} iterations, with the worst of {which} at a '
                f'residual of {worst:.1e}, short of '
                f'the {ITERATIVE_ACCEPT:.0e} a mode must reach. Its multigrid '
                'preconditioner is made for solid elements and works poorly on thin '
                f'shells, beams, rigid links and springs; {what}. It was solved this '
                f'way because {self.refused}')
