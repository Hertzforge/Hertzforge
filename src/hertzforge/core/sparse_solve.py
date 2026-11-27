"""A sparse symmetric matrix factored once and solved many times, by the
fastest solver the machine has.

The modal solve of a large model factors (K - sigma M) once and solves
with it a few hundred times: every Lanczos step and every polish step
is a solve. On macOS the factorization is Apple's, in the Accelerate
framework every Mac carries (Sparse Solvers: a supernodal Cholesky, or
LDLT when the matrix is not positive definite), reached through ctypes
from the layouts in its public header. On Windows and Linux on Intel
and AMD processors it is PARDISO, the multithreaded supernodal solver
in Intel's oneMKL, through pypardiso (Cholesky, or LDLT with
Bunch-Kaufman pivoting). Everywhere else, and wherever those refuse a
matrix, it is SuperLU's LU through scipy, told the matrix is symmetric
so it orders and pivots for that.

A factor that would not fit in the machine's memory is refused before
the factorization starts (`factor_memory`, `FactorTooLarge`): the
factorization is one long call into the library, which nothing can
interrupt, and a factor larger than memory runs it in swap for hours.
"""

from __future__ import annotations

import atexit
import ctypes as C
import math
import os
import sys
import weakref
from typing import Any

import numpy as np
import scipy.sparse as sp

#: set to 'superlu' to keep Accelerate and PARDISO out, which is how the
#: tests run both paths on one machine
BACKEND_VARIABLE = 'HERTZFORGE_SPARSE_SOLVER'


def _superlu_only() -> bool:
    return os.environ.get(BACKEND_VARIABLE, '').lower() == 'superlu'


#: the backward error a probe solve may have before the factor is
#: refused as that of a matrix singular to working precision. A
#: factorization reports success on such a matrix and hands back a
#: factor whose solves are garbage: a tied model with a rotation
#: nothing held had solves with a backward error of 11 under SuperLU
#: and finite garbage under Accelerate's LDLT (2026-10-19), and two
#: eigensolvers ran on them. A sound factor of an ill-conditioned
#: matrix measures 1e-8 to 2e-6 on the models here (the springs' the
#: worst), a garbage one 0.25, 11 and 6e8.
PROBE_LIMIT = 1e-3


def probe_backward_error(matrix: Any, solve: Any) -> float:
    """|A x - b| / |b| for one solve with a fixed right-hand side, the
    cosines of the row numbers; infinite when the answer is not."""
    probe = np.cos(np.arange(matrix.shape[0], dtype=np.float64))
    answer = solve(probe)
    if not np.isfinite(answer).all():
        return math.inf
    return float(np.linalg.norm(matrix @ answer - probe) / np.linalg.norm(probe))


def count_below(stiffness: Any, mass: Any, shift: float, held: int = 0) -> int:
    """How many eigenvalues of K phi = lambda M phi lie below `shift`,
    read off the signs of one factorization of K - shift M.

    By Sylvester's law of inertia, a symmetric matrix factored as
    L D Lᵀ has as many negative entries in D as it has negative
    eigenvalues, and the negative eigenvalues of K - shift M are the
    modes below the shift (M positive definite). Where PARDISO is
    available its LDLT reports the count itself; Bunch-Kaufman's 2 x 2
    pivots keep the count. Elsewhere SuperLU in symmetric mode with no
    pivoting off the diagonal is that factorization, its U's diagonal
    being D, which is why the threshold is zero here and not the
    factor the solves use. A pivot within round-off of zero,
    from a shift on top of an eigenvalue, has a sign that is round-off,
    so a caller allows the count one either way.

    Parameters
    ----------
    stiffness, mass : scipy.sparse matrix
        K and M, symmetric, M positive definite.
    shift : float
        The eigenvalue to count below, (rad/s)².
    held : int, default 0
        Bytes already held, by the factor the modal solve keeps: the
        count's own factorization must fit beside them.

    Returns
    -------
    int
        The count, rigid-body modes included when the shift is above
        zero.

    Raises
    ------
    RuntimeError
        If the factorization fails, had to pivot off the diagonal or
        perturbed a pivot, so its signs are not the inertia.
    FactorTooLarge
        Before anything is factored, when the factorization and `held`
        would take more than `MEMORY_SHARE` of the machine's memory.

    References
    ----------
    A standard method, used here to size the search up to a frequency:

    1. J. J. Sylvester, *A demonstration of the theorem that every
       homogeneous quadratic polynomial is reducible by real orthogonal
       substitutions to the form of a sum of positive and negative
       squares*, Philosophical Magazine, 1852.
    """
    from scipy.sparse.linalg import splu

    shifted = sp.csc_matrix(stiffness - float(shift) * mass)
    # 1.5 s where SuperLU took 29 on a 135,000-DOF frame, 5.2 s to 41 on
    # a 106,000-DOF block of bricks, the same counts (2026-11-02)
    if not _superlu_only() and _pypardiso() is not None:
        return _pardiso_negatives(shifted, held)
    _refuse_beyond_memory(held + _superlu_memory(shifted), 'superlu', shifted.shape[0])
    factor = splu(shifted,
                  permc_spec='MMD_AT_PLUS_A',
                  options={'SymmetricMode': True, 'DiagPivotThresh': 0.0})
    if not np.array_equal(factor.perm_r, factor.perm_c):
        raise RuntimeError('the factorization pivoted off the diagonal')
    return int(np.count_nonzero(factor.U.diagonal() < 0.0))


class SuperLUFactor:
    """SuperLU's factorization of a symmetric matrix, through scipy."""

    backend = 'superlu'
    #: the bytes `factor_memory` estimated, set by `factor_symmetric`
    memory = 0

    def __init__(self, matrix: Any) -> None:
        from scipy.sparse.linalg import splu

        # Told the matrix is symmetric (2026-10-18): the default ordering
        # treats it as general, and on a 135,000-DOF model factored in
        # 14 s with 164 M nonzeros in the factors where this takes 3.8 s
        # and 87 M; every solve is half the work as well
        self._factor = splu(sp.csc_matrix(matrix), permc_spec='MMD_AT_PLUS_A',
                            options={'SymmetricMode': True})
        self.shape: tuple[int, int] = self._factor.shape
        residual = probe_backward_error(matrix, self.solve)
        if not residual < PROBE_LIMIT:
            raise RuntimeError('the matrix is singular to working precision: '
                               f'a probe solve has a backward error of {residual:.1e}')

    def solve(self, rhs: Any) -> np.ndarray:
        """X with A X = `rhs`, for one right-hand side or a block of
        them, in the shape `rhs` came in."""
        return self._factor.solve(np.asarray(rhs, dtype=np.float64))


# ---- Accelerate's Sparse Solvers, from <Accelerate/Sparse/Solve.h> --------
#
# The public functions in that header are inline wrappers over the
# `_Sparse*_Double` entry points below, and the layouts are copied from
# its struct definitions; `tests/test_sparse_solve.py` pins their sizes.

class _MatrixStructure(C.Structure):
    _fields_ = [('rowCount', C.c_int), ('columnCount', C.c_int),
                ('columnStarts', C.POINTER(C.c_long)),
                ('rowIndices', C.POINTER(C.c_int)),
                ('attributes', C.c_uint32), ('blockSize', C.c_uint8)]


class _SparseMatrix(C.Structure):
    _fields_ = [('structure', _MatrixStructure),
                ('data', C.POINTER(C.c_double))]


class _DenseMatrix(C.Structure):
    _fields_ = [('rowCount', C.c_int), ('columnCount', C.c_int),
                ('columnStride', C.c_int), ('attributes', C.c_uint32),
                ('data', C.POINTER(C.c_double))]


class _Symbolic(C.Structure):
    _fields_ = [('status', C.c_int32), ('rowCount', C.c_int),
                ('columnCount', C.c_int), ('attributes', C.c_uint32),
                ('blockSize', C.c_uint8), ('type', C.c_uint8),
                ('factorization', C.c_void_p),
                ('workspaceSize_Float', C.c_size_t),
                ('workspaceSize_Double', C.c_size_t),
                ('factorSize_Float', C.c_size_t),
                ('factorSize_Double', C.c_size_t)]


class _Factorization(C.Structure):
    _fields_ = [('status', C.c_int32), ('attributes', C.c_uint32),
                ('symbolic', _Symbolic), ('userFactorStorage', C.c_bool),
                ('numeric', C.c_void_p),
                ('solveWorkspaceRequiredStatic', C.c_size_t),
                ('solveWorkspaceRequiredPerRHS', C.c_size_t)]


_MALLOC = C.CFUNCTYPE(C.c_void_p, C.c_size_t)
_FREE = C.CFUNCTYPE(None, C.c_void_p)
_REPORT = C.CFUNCTYPE(None, C.c_char_p)


class _SymbolicOptions(C.Structure):
    _fields_ = [('control', C.c_uint32), ('orderMethod', C.c_uint8),
                ('order', C.POINTER(C.c_int)),
                ('ignoreRowsAndColumns', C.POINTER(C.c_int)),
                ('malloc', _MALLOC), ('free', _FREE), ('reportError', _REPORT)]


class _NumericOptions(C.Structure):
    _fields_ = [('control', C.c_uint32), ('scalingMethod', C.c_uint8),
                ('scaling', C.c_void_p), ('pivotTolerance', C.c_double),
                ('zeroTolerance', C.c_double)]


#: the sizes the header's layouts come to on a 64-bit machine
STRUCT_SIZES = {'SparseMatrixStructure': 32, 'SparseMatrix_Double': 40,
                'DenseMatrix_Double': 24, 'SparseOpaqueSymbolicFactorization': 64,
                'SparseOpaqueFactorization_Double': 104,
                'SparseSymbolicFactorOptions': 48, 'SparseNumericFactorOptions': 32}

#: SparseAttributes_t: transpose (1 bit), triangle (1 bit, lower = 1),
#: kind (2 bits, symmetric = 3) — the lower triangle of a symmetric matrix
_LOWER_SYMMETRIC = (1 << 1) | (3 << 2)
#: SparseFactorization_t
_CHOLESKY, _LDLT = 0, 1
_STATUS = {0: 'ok', -1: 'the factorization failed', -2: 'the matrix is singular',
           -3: 'an internal error', -4: 'a parameter error'}

_accelerate: dict[str, Any] = {}


def _library() -> dict[str, Any] | None:
    """The entry points, bound once; None where Accelerate is not."""
    if _accelerate:
        return _accelerate if _accelerate.get('ok') else None
    _accelerate['ok'] = False
    if sys.platform != 'darwin':
        return None
    try:
        lib = C.CDLL('/System/Library/Frameworks/Accelerate.framework/Accelerate')
        libc = C.CDLL(None)
        factor = lib._SparseFactorSymmetric_Double
        solve = lib._SparseSolveOpaque_Double
        destroy = lib._SparseDestroyOpaqueNumeric_Double
        symbolic = lib._SparseSymbolicFactorSymmetric
        destroy_symbolic = lib._SparseDestroyOpaqueSymbolic
    except (OSError, AttributeError):
        return None
    symbolic.restype = _Symbolic
    symbolic.argtypes = [C.c_uint8, C.POINTER(_MatrixStructure), C.POINTER(_SymbolicOptions)]
    destroy_symbolic.restype = None
    destroy_symbolic.argtypes = [C.POINTER(_Symbolic)]
    factor.restype = _Factorization
    factor.argtypes = [C.c_uint8, C.POINTER(_SparseMatrix),
                       C.POINTER(_SymbolicOptions), C.POINTER(_NumericOptions)]
    solve.restype = None
    solve.argtypes = [C.POINTER(_Factorization), C.POINTER(_DenseMatrix),
                      C.POINTER(_DenseMatrix), C.c_void_p]
    destroy.restype = None
    destroy.argtypes = [C.POINTER(_Factorization)]
    libc.malloc.restype = C.c_void_p
    libc.malloc.argtypes = [C.c_size_t]
    libc.free.restype = None
    libc.free.argtypes = [C.c_void_p]
    messages: list[str] = []
    report = _REPORT(lambda text: messages.append(text.decode(errors='replace')))
    _accelerate.update(
        ok=True, factor=factor, solve=solve, destroy=destroy, messages=messages,
        symbolic_factor=symbolic, destroy_symbolic=destroy_symbolic,
        # Accelerate's defaults, with its errors kept rather than printed
        symbolic=_SymbolicOptions(0, 0, None, None, _MALLOC(('malloc', libc)),
                                  _FREE(('free', libc)), report),
        numeric=_NumericOptions(0, 0, None, 0.1, 0.0), report=report)
    return _accelerate


def _lower_structure(matrix: Any) -> tuple[_MatrixStructure, np.ndarray, np.ndarray, Any]:
    """Accelerate's structure of the lower triangle of a symmetric
    matrix, with the column starts and row indices it points into (to be
    kept while the structure is used) and the triangle itself."""
    lower = sp.tril(matrix, format='csc')
    lower.sort_indices()
    starts = np.ascontiguousarray(lower.indptr, dtype=np.int64)
    rows = np.ascontiguousarray(lower.indices, dtype=np.int32)
    n = lower.shape[0]
    structure = _MatrixStructure(n, n, starts.ctypes.data_as(C.POINTER(C.c_long)),
                                 rows.ctypes.data_as(C.POINTER(C.c_int)), _LOWER_SYMMETRIC, 1)
    return structure, starts, rows, lower


class AccelerateFactor:
    """Apple's supernodal factorization of a symmetric matrix: Cholesky,
    or LDLT when the matrix is not positive definite.

    Raises `ValueError` when Accelerate refuses the matrix, naming its
    status, so the caller can fall back.
    """

    backend = 'accelerate'
    #: the bytes `factor_memory` estimated, set by `factor_symmetric`
    memory = 0

    def __init__(self, matrix: Any) -> None:
        lib = _library()
        if lib is None:
            raise ValueError('Accelerate is not available here')
        # kept for the factorization's life: Accelerate reads them in
        # place and the factor may refer back to the structure
        structure, self._starts, self._rows, lower = _lower_structure(matrix)
        self._values = np.ascontiguousarray(lower.data, dtype=np.float64)
        n = lower.shape[0]
        self.shape: tuple[int, int] = (n, n)
        self._matrix = _SparseMatrix(structure,
                                     self._values.ctypes.data_as(C.POINTER(C.c_double)))
        self._factor = None
        for kind in (_CHOLESKY, _LDLT):
            del lib['messages'][:]
            factored = lib['factor'](kind, C.byref(self._matrix),
                                     C.byref(lib['symbolic']), C.byref(lib['numeric']))
            if factored.status == 0:
                self._factor = factored
                self.kind: str = 'Cholesky' if kind == _CHOLESKY else 'LDLT'
                # LDLT reports success on an exactly singular matrix and
                # its solves come back NaN (measured 2026-10-18: an empty
                # matrix, a zero row, a rank-one block all 'ok'), or
                # finite garbage on one singular to working precision
                # (2026-10-19, a backward error of 0.25), where Cholesky
                # succeeding proves the matrix definite. One probe solve
                # tells, and a refusal here sends the matrix on to
                # SuperLU, whose own probe the modal solve turns into
                # its message
                if kind == _LDLT and not probe_backward_error(
                        matrix, self.solve) < PROBE_LIMIT:
                    lib['destroy'](C.byref(factored))
                    self._factor = None
                    raise ValueError('Accelerate found the matrix singular')
                return
            lib['destroy'](C.byref(factored))
        why = _STATUS.get(factored.status, f'status {factored.status}')
        said = '; '.join(m.strip() for m in lib['messages'] if m.strip())
        raise ValueError(f'Accelerate refused the matrix: {why}'
                         + (f' ({said})' if said else ''))

    def solve(self, rhs: Any) -> np.ndarray:
        """X with A X = `rhs`, for one right-hand side or a block of
        them, in the shape `rhs` came in."""
        lib = _library()
        given = np.asarray(rhs, dtype=np.float64)
        b = np.asfortranarray(given.reshape(given.shape[0], -1))
        n, count = b.shape
        x = np.zeros((n, count), dtype=np.float64, order='F')
        right = _DenseMatrix(n, count, n, 0, b.ctypes.data_as(C.POINTER(C.c_double)))
        out = _DenseMatrix(n, count, n, 0, x.ctypes.data_as(C.POINTER(C.c_double)))
        needed = (self._factor.solveWorkspaceRequiredStatic
                  + count * self._factor.solveWorkspaceRequiredPerRHS)
        workspace = C.create_string_buffer(int(needed) + 64)
        lib['solve'](C.byref(self._factor), C.byref(right), C.byref(out),
                     C.cast(workspace, C.c_void_p))
        return x.reshape(given.shape) if given.ndim == 1 else x

    def __del__(self) -> None:
        lib = _accelerate if _accelerate.get('ok') else None
        if lib is not None and getattr(self, '_factor', None) is not None:
            lib['destroy'](C.byref(self._factor))
            self._factor = None


# ---- PARDISO, in Intel's oneMKL, through pypardiso -------------------------
#
# A dependency on x86-64 Windows and Linux only, where oneMKL is built
# (Brandon, 2026-11-02). A Windows machine's 193,627-brick modal solve
# took 70 minutes through SuperLU where a Mac's Accelerate took two;
# measured on a two-core Linux laptop, PARDISO factored a 106,000-DOF
# block of bricks in 6.1 s on one thread and 3.6 s on four, against
# SuperLU's 239 s, and the 608,000-DOF block of 192,000 bricks in 106 s.
# Its own nested-dissection ordering (METIS) is most of that: the
# factor had half SuperLU's nonzeros on the block, and fewer on a thin
# frame as well.

_pardiso: dict[str, Any] = {}


def _pypardiso() -> Any:
    """The pypardiso module, imported once; None where it or the oneMKL
    library it loads is not."""
    if 'module' not in _pardiso:
        # threaded through TBB, before oneMKL loads (Brandon, 2026-11-02):
        # the packaged builds carry TBB and not the OpenMP runtime, which
        # is under Intel's developer-tools EULA, and TBB factored as fast
        # on the measured machine. A default, so a user's own choice
        # stands where the library for it is installed
        os.environ.setdefault('MKL_THREADING_LAYER', 'TBB')
        try:
            import pypardiso
        except (ImportError, OSError) as missing:
            pypardiso = None
            _pardiso['why'] = f'{type(missing).__name__}: {missing}'
        _pardiso['module'] = pypardiso
        if pypardiso is not None:
            atexit.register(_hold_libraries)
    return _pardiso['module']


def _hold_libraries() -> list[str]:
    """Take one more reference on every shared library the process has
    mapped, so that closing one no longer unloads it, and return their
    paths. Linux only: nothing is held, and the list is empty, where
    there is no ``/proc/self/maps``.

    Run as the interpreter exits in a process that loaded oneMKL. TBB
    opens libraries for itself as it starts (its memory allocator, the
    thread composability manager, hwloc behind that) and closes them
    when its last reference goes. At exit that reference is let go
    without waiting for the worker threads, so it is the last worker
    out that closes them, while the main thread is still walking the
    exit handlers: the main thread then calls a destructor in a library
    the worker has just unmapped. With a reference held here the
    worker's close unloads nothing, and the destructors run where they
    always do, on the main thread.
    """
    # About one process in 175 that had factored with PARDISO died of
    # SIGSEGV on its way out, its work done and printed: 233 of 40,685
    # on four-core Linux machines under load, none of 3,804 with oneMKL
    # sequential, none of 4,919 through SuperLU and none of 28,957 with
    # this hold (oneMKL 2026.2.1, TBB 2023.2.0, 2026-11-25). Of 60
    # cores read, 58 had libtcm gone from the process and the other two
    # a worker part way through closing it; 40 showed the main thread
    # under exit()'s handlers at an address no longer mapped, and 29 a
    # TBB worker in dlclose under governor::release_resources. Taken at
    # exit, not at the first factorization: TBB opens what it needs as
    # it needs it, and nothing is closed before the handlers run
    try:
        with open('/proc/self/maps', encoding='utf-8', errors='replace') as maps:
            # address, permissions, offset, device, inode, then the path
            mapped = {fields[5].strip() for fields in (line.split(None, 5) for line in maps)
                      if len(fields) == 6 and fields[5].startswith('/')}
    except OSError:
        return []
    held = []
    for path in sorted(mapped):
        if '.so' not in os.path.basename(path):
            continue
        try:
            # only what is loaded already: a reference, never a load
            C.CDLL(path, mode=os.RTLD_NOW | os.RTLD_NOLOAD)
        except OSError:
            continue
        held.append(path)
    _pardiso['held'] = held
    return held


def expected_backend() -> str:
    """The backend this machine should factor with: 'accelerate' on a
    Mac, 'pardiso' on x86-64 Windows and Linux (where `pypardiso` is a
    dependency), 'superlu' anywhere else."""
    import platform

    if sys.platform == 'darwin':
        return 'accelerate'
    if platform.machine() in ('x86_64', 'AMD64'):
        return 'pardiso'
    return 'superlu'


def check() -> tuple[int, str]:
    """Check that this installation factors with the solver it should.

    `hertzforge check-solver` runs this and exits with the
    returned code. The packaged builds are tested with it, because a
    bundle that leaves oneMKL out still solves, through SuperLU, many
    times slower on a large model.

    Returns
    -------
    tuple of (int, str)
        0 and the backend, with the library it loaded, when a small
        model's factorization took `expected_backend` and its solve
        and mode count are right; 1 and what happened otherwise.
    """
    n = 1000
    matrix = sp.diags([np.full(n - 1, -1.0), np.full(n, 2.5), np.full(n - 1, -1.0)],
                      [-1, 0, 1], format='csc')
    expected = expected_backend()
    try:
        factor = factor_symmetric(matrix)
        rhs = np.cos(np.arange(n, dtype=np.float64))
        residual = float(np.linalg.norm(matrix @ factor.solve(rhs) - rhs)
                         / np.linalg.norm(rhs))
        below = count_below(matrix, sp.identity(n, format='csc'), 1.0)
    except Exception as failure:  # noqa: BLE001 — whatever the packaged app would hit
        return 1, f'the factorization failed: {type(failure).__name__}: {failure}'
    # the eigenvalues are 2.5 - 2 cos(k pi / (n + 1)), k = 1 to n
    eigenvalues = 2.5 - 2.0 * np.cos(np.arange(1, n + 1) * np.pi / (n + 1))
    exact = int(np.count_nonzero(eigenvalues < 1.0))
    where = ''
    if factor.backend == 'pardiso':
        from threadpoolctl import threadpool_info

        layers = {pool.get('threading_layer') for pool in threadpool_info()
                  if pool['internal_api'] == 'mkl'}
        where = f' ({factor._solver.libmkl._name}, threaded by {", ".join(sorted(map(str, layers)))})'
    said = f'{factor.backend}{where}: residual {residual:.1e}, {below} of {n} modes below 1.0'
    if factor.backend != expected:
        why = _pardiso.get('why', '') if expected == 'pardiso' else ''
        return 1, f'expected {expected}, got {said}' + (f'; {why}' if why else '')
    if not (residual < 1e-12 and below == exact):
        return 1, f'wrong answers from {said}; {exact} are below 1.0'
    return 0, said


def _upper_triangle(matrix: Any) -> Any:
    """The upper triangle, CSR, every diagonal entry stored even where
    it is zero, as PARDISO requires of a symmetric matrix."""
    strict = sp.triu(matrix, k=1, format='coo')
    index = np.arange(matrix.shape[0])
    # built from triplets: a sum of matrices would drop the zeros
    upper = sp.csr_matrix(
        (np.concatenate([np.asarray(matrix.diagonal(), dtype=np.float64),
                         strict.data.astype(np.float64)]),
         (np.concatenate([index, strict.row]), np.concatenate([index, strict.col]))),
        shape=matrix.shape)
    upper.sort_indices()
    return upper


#: PARDISO's iparm, 1-based as its manual numbers them: the settings
#: given rather than oneMKL's defaults (1), nested dissection by
#: parallel METIS (2), pivots perturbed at 1e-8 in LDLT (10), no
#: scaling (11) and no matching (13), which the matrix's own symmetric
#: scaling makes needless, Bunch-Kaufman pivots (21), and the two-level
#: factorization that spreads over many cores (24)
PARDISO_SETTINGS = {1: 1, 2: 3, 10: 8, 11: 0, 13: 0, 21: 1, 24: 1}

#: iterative refinement steps on each solve (iparm 8), by matrix type.
#: None for Cholesky: its factor is exact, and a refined answer is
#: corrected by its own residual, so the operator is no longer
#: symmetric. oneMKL's default of two steps left it 1.3e-8 off
#: symmetric on the rigid-link test model, against 7e-16 unrefined, and
#: the block Lanczos, which reads that asymmetry as part of every
#: pair's residual, left a free plate's 60th mode 1.2e-5 off where
#: SuperLU's solves gave 1.7e-12. Two for LDLT: its perturbed pivots
#: factor a nearby matrix, and refinement brings the solve back, a
#: shifted Laplacian's backward error 1.5e-8 to 8e-16 (2026-11-02)
PARDISO_REFINEMENT = {2: 0, -2: 2}


def _pardiso_solver(mtype: int) -> Any:
    """A pypardiso solver for matrix type `mtype` (2 positive definite,
    -2 symmetric indefinite), with `PARDISO_SETTINGS` and the type's
    `PARDISO_REFINEMENT`."""
    # the matrix is kept, not hashed, to tell the factored one at each
    # solve: a hash of a large model's matrix took longer than the solve
    # it guarded
    solver = _pypardiso().PyPardisoSolver(mtype=mtype, size_limit_storage=math.inf)
    for index, value in PARDISO_SETTINGS.items():
        solver.set_iparm(index, value)
    solver.set_iparm(8, PARDISO_REFINEMENT[mtype])
    return solver


def _pardiso_negatives(matrix: Any, held: int = 0) -> int:
    """The negative eigenvalue count of a symmetric matrix, from
    PARDISO's LDLT (`count_below`), refused with `FactorTooLarge` when
    its factor and the `held` bytes would not fit in memory."""
    from pypardiso.pardiso_wrapper import PyPardisoError

    upper = _upper_triangle(matrix)
    solver = _pardiso_solver(-2)
    try:
        # the analysis first, so the factor's size is known before the
        # numeric factorization, which then reuses the ordering
        _refuse_beyond_memory(held + _pardiso_analysis(solver, upper), 'pardiso',
                              upper.shape[0])
        solver.set_phase(22)
        solver._call_pardiso(upper, np.zeros((upper.shape[0], 1)))
        perturbed = int(solver.get_iparm(14))
        negatives = int(solver.get_iparm(23))
    except FactorTooLarge:
        raise
    except (PyPardisoError, ValueError) as failure:
        raise RuntimeError(f'PARDISO could not factor the matrix: {failure}') from None
    finally:
        solver.free_memory(everything=True)
    if perturbed:
        raise RuntimeError(f'PARDISO perturbed {perturbed} pivots')
    return negatives


class PardisoFactor:
    """PARDISO's supernodal factorization of a symmetric matrix:
    Cholesky, or LDLT when the matrix is not positive definite.

    Raises `ValueError` when PARDISO is not available or refuses the
    matrix, so the caller can fall back.
    """

    backend = 'pardiso'
    #: the bytes `factor_memory` estimated, set by `factor_symmetric`
    memory = 0

    def __init__(self, matrix: Any) -> None:
        pypardiso = _pypardiso()
        if pypardiso is None:
            raise ValueError('PARDISO is not available here')
        from pypardiso.pardiso_wrapper import PyPardisoError

        self._upper = _upper_triangle(matrix)
        n = self._upper.shape[0]
        self.shape: tuple[int, int] = (n, n)
        self._solver = None
        failure: Exception | None = None
        for mtype, kind in ((2, 'Cholesky'), (-2, 'LDLT')):
            solver = _pardiso_solver(mtype)
            try:
                solver.factorize(self._upper)
            except (PyPardisoError, ValueError) as refused:
                solver.free_memory(everything=True)
                failure = refused
                continue
            self._solver, self.kind = solver, kind
            # oneMKL's own memory, which Python does not see, freed when
            # the factor is dropped: without it a factor held its
            # gigabytes until exit. A finalizer rather than __del__,
            # which ran during interpreter shutdown, after the modules
            # pypardiso's release calls on were gone
            self._finalizer = weakref.finalize(self, solver.free_memory, True)
            # LDLT perturbs a pivot within round-off of zero and reports
            # success on a singular matrix, as Accelerate's LDLT does;
            # one probe solve tells, and is cheap beside the factor
            if not probe_backward_error(matrix, self.solve) < PROBE_LIMIT:
                self._release()
                raise ValueError('PARDISO found the matrix singular')
            return
        raise ValueError(f'PARDISO refused the matrix: {failure}')

    def solve(self, rhs: Any) -> np.ndarray:
        """X with A X = `rhs`, for one right-hand side or a block of
        them, in the shape `rhs` came in."""
        return self._solver.solve(self._upper, np.asarray(rhs, dtype=np.float64))

    def _release(self) -> None:
        self._finalizer()
        self._solver = None


# ---- whether a factor fits in memory --------------------------------------
#
# A 754,640-brick model (2.5 million degrees of freedom) on a 32 GB Mac
# took the analysis service to 84 GB with 59 GB of it in swap, and the
# factorization crawled on with no end in sight (2026-11-27). The call
# into the library cannot be interrupted, so the only way out was to
# quit the app; the factor's size is known, or well estimated, before
# that call starts.

#: the share of the machine's physical memory a factorization may take;
#: the rest is the system's, the matrices' and the eigenvectors'
MEMORY_SHARE = 0.75

#: bytes a nonzero of SuperLU's L and U take at its peak, the value, its
#: row index and the arrays it grows as it goes: 22 and 23.6 measured on
#: blocks of 14,553 and 52,728 degrees of freedom
SUPERLU_BYTES = 24

#: SuperLU's peak over the size of Accelerate's factor of the same
#: matrix, where both are: measured 4.0 on the Frame-Wing, 4.9 on two
#: blocks of bricks and 5.4 on a plate of bricks one thick
SUPERLU_OVER_ACCELERATE = 8.0


def physical_memory() -> int:
    """The machine's physical memory, in bytes."""
    if sys.platform == 'win32':
        class _MemoryStatus(C.Structure):
            _fields_ = [('dwLength', C.c_ulong), ('dwMemoryLoad', C.c_ulong),
                        ('ullTotalPhys', C.c_ulonglong), ('ullAvailPhys', C.c_ulonglong),
                        ('ullTotalPageFile', C.c_ulonglong),
                        ('ullAvailPageFile', C.c_ulonglong),
                        ('ullTotalVirtual', C.c_ulonglong), ('ullAvailVirtual', C.c_ulonglong),
                        ('ullAvailExtendedVirtual', C.c_ulonglong)]

        status = _MemoryStatus()
        status.dwLength = C.sizeof(_MemoryStatus)
        C.windll.kernel32.GlobalMemoryStatusEx(C.byref(status))  # type: ignore[attr-defined]
        return int(status.ullTotalPhys)
    return int(os.sysconf('SC_PAGE_SIZE') * os.sysconf('SC_PHYS_PAGES'))


def _gigabytes(size: float) -> str:
    # binary gigabytes, the unit a machine's memory is sold and shown in
    # (a 32 GB Mac has 32 x 2^30 bytes)
    value = size / 2**30
    return f'{value:.0f} GB' if value >= 10 else f'{value:.2g} GB'


class FactorTooLarge(ValueError):
    """A factorization refused because its factor would not fit in the
    machine's memory, raised before it starts.

    Attributes
    ----------
    needed : int
        The bytes the factorization was estimated to need, with any
        factor already held.
    physical : int
        The machine's physical memory, in bytes (`physical_memory`).
    backend : str
        The solver the estimate is for: 'accelerate', 'pardiso' or
        'superlu'.
    """

    def __init__(self, needed: int, physical: int, backend: str, size: int) -> None:
        self.needed, self.physical, self.backend = needed, physical, backend
        how = {'accelerate': "by Accelerate's symbolic factorization",
               'pardiso': "by PARDISO's analysis",
               'superlu': "by an estimate of SuperLU's fill"}[backend]
        super().__init__(
            f'factoring this model of {size:,} degrees of freedom would need about '
            f'{_gigabytes(needed)} of memory {how}, more than the '
            f'{_gigabytes(MEMORY_SHARE * physical)} a factorization may take on this '
            f'machine ({MEMORY_SHARE:.0%} of its {_gigabytes(physical)} of physical '
            'memory); mesh it coarser, or solve it on a machine with more memory')


def _refuse_beyond_memory(needed: int, backend: str, size: int) -> None:
    physical = physical_memory()
    if needed > MEMORY_SHARE * physical:
        raise FactorTooLarge(needed, physical, backend, size)


def _accelerate_memory(matrix: Any) -> tuple[int, int] | None:
    """(factor, workspace) bytes of Accelerate's Cholesky of the
    matrix, from its symbolic factorization alone; None where Accelerate
    is not or refuses the structure. The factor is the least a Cholesky
    takes; an LDLT that delays pivots takes more."""
    lib = _library()
    if lib is None:
        return None
    structure, _starts, _rows, _lower = _lower_structure(matrix)
    symbolic = lib['symbolic_factor'](_CHOLESKY, C.byref(structure), C.byref(lib['symbolic']))
    status, factor, workspace = (symbolic.status, symbolic.factorSize_Double,
                                 symbolic.workspaceSize_Double)
    lib['destroy_symbolic'](C.byref(symbolic))
    return (int(factor), int(workspace)) if status == 0 else None


def _pardiso_analysis(solver: Any, upper: Any) -> int:
    """Run PARDISO's analysis (phase 11: ordering and symbolic
    factorization) of `upper` on `solver`, and return the bytes its
    factorization will take at its peak, max(iparm 15, iparm 16 +
    iparm 17) kilobytes, as oneMKL's manual gives it."""
    solver._check_A(upper)
    solver.set_phase(11)
    solver._call_pardiso(upper, np.zeros((upper.shape[0], 1)))
    symbolic, permanent, numeric = (int(solver.get_iparm(index)) for index in (15, 16, 17))
    return 1024 * max(symbolic, permanent + numeric)


def _pardiso_memory(matrix: Any, mtype: int) -> int | None:
    """The bytes PARDISO's factorization of the matrix will take, from
    an analysis on a solver of its own; None where PARDISO is not or
    refuses the matrix."""
    if _pypardiso() is None:
        return None
    from pypardiso.pardiso_wrapper import PyPardisoError

    solver = _pardiso_solver(mtype)
    try:
        return _pardiso_analysis(solver, _upper_triangle(matrix))
    except (PyPardisoError, ValueError):
        return None
    finally:
        solver.free_memory(everything=True)


def _superlu_memory(matrix: Any) -> int:
    """The bytes SuperLU's factorization of the matrix will take at its
    peak, estimated; SuperLU has no symbolic step to ask.

    Where Accelerate is, `SUPERLU_OVER_ACCELERATE` times the size of
    Accelerate's factor. Elsewhere, from the envelope of the matrix in
    reverse Cuthill-McKee order: the entries between each row's first
    nonzero and the diagonal, which an envelope solver would fill, taken
    for L and for U, at `SUPERLU_BYTES` each. That came out 1.4 to 2.5
    times SuperLU's own fill on blocks and plates of bricks of 4,000 to
    52,000 degrees of freedom, and 4.5 times on the Frame-Wing, but 0.8
    times on a bar of 1,845: an estimate, and a generous one for a
    large model, not a bound.
    """
    accelerate = _accelerate_memory(matrix)
    if accelerate is not None:
        return int(SUPERLU_OVER_ACCELERATE * accelerate[0])
    from scipy.sparse.csgraph import reverse_cuthill_mckee

    rows = sp.csr_matrix(matrix)
    order = reverse_cuthill_mckee(rows, symmetric_mode=True)
    lower = sp.tril(rows[order][:, order], format='csr')
    lower.sort_indices()
    n = lower.shape[0]
    filled = np.ones(n, dtype=np.int64)
    stored = np.diff(lower.indptr) > 0
    first = lower.indices[lower.indptr[:-1][stored]]
    filled[stored] = np.arange(n)[stored] - first + 1
    return int(SUPERLU_BYTES * 2 * int(filled.sum()))


def factor_memory(matrix: Any) -> tuple[int, str]:
    """The bytes `factor_symmetric` will take to factor the matrix, at
    its peak, and the backend the estimate is for.

    Each backend is asked in the order `factor_symmetric` tries them,
    the first that takes the matrix answering: Accelerate's symbolic
    factorization gives its factor and workspace exactly, in a small
    part of the factorization's time (0.15 s of 4.1 s on a block of
    89,373 degrees of freedom); PARDISO's analysis gives its peak, at
    the cost of one more ordering of the matrix; SuperLU's is estimated
    (`_superlu_memory`).

    Parameters
    ----------
    matrix : sparse matrix
        Square and symmetric.

    Returns
    -------
    tuple of (int, str)
        The bytes, and 'accelerate', 'pardiso' or 'superlu'.
    """
    if not _superlu_only():
        accelerate = _accelerate_memory(matrix)
        if accelerate is not None:
            return sum(accelerate), 'accelerate'
        pardiso = _pardiso_memory(matrix, 2)
        if pardiso is not None:
            return pardiso, 'pardiso'
    return _superlu_memory(matrix), 'superlu'


def factor_symmetric(matrix: Any) -> AccelerateFactor | PardisoFactor | SuperLUFactor:
    """The matrix factored by the fastest solver here that takes it:
    Accelerate on a Mac, PARDISO on x86-64 Windows and Linux, else
    SuperLU. Each answers `solve(rhs)` and `shape`.

    Parameters
    ----------
    matrix : sparse matrix
        Square and symmetric.

    Returns
    -------
    AccelerateFactor, PardisoFactor or SuperLUFactor
        With `memory`, the bytes `factor_memory` estimated it would take.

    Raises
    ------
    FactorTooLarge
        Before anything is factored, when `factor_memory` is more than
        `MEMORY_SHARE` of the machine's physical memory.
    """
    needed, backend = factor_memory(matrix)
    _refuse_beyond_memory(needed, backend, matrix.shape[0])
    factor: AccelerateFactor | PardisoFactor | SuperLUFactor | None = None
    if not _superlu_only():
        for fast in (AccelerateFactor, PardisoFactor):
            try:
                factor = fast(matrix)
                break
            except ValueError:
                pass
    if factor is None:
        factor = SuperLUFactor(matrix)
    factor.memory = needed
    return factor
