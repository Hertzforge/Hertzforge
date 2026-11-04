"""Convert a random specification into the controller's native reference file.

The controller's multiple-output random test runs to a reference: the
spectral density matrix (SDM) on its grid of lines, the alarm and abort
lines per control, the band and a record of where it came from. It reads
that reference from its own file, a folder holding `reference.bin` (the
numbers, little-endian) and `reference.json` (the description), laid out
in `docs/reference-file.md`. This module writes that file from what the
controller does not read itself:

- a specification `.mat` file (MATLAB v5 through scipy, v7.3 through
  h5py): `cpsd` channels by channels by lines, `f` the lines in hertz,
  and optionally `warning_upper`, `warning_lower`, `abort_upper` and
  `abort_lower`, channels by lines (`from_mat`);
- a `.hforge` file holding a random `Specification` (`from_hforge`).

It also reads the native file back (`read`), which is how its tests check
a round trip.

Mapping onto the controller's grid follows the controller's own importer:
no interpolation; a file frequency is placed on line `round(f / df)` when it
is within 1e-9 of the spacing of that line, and refused by name otherwise;
grid lines the file does not hold are zero; `NaN` in a limit means no line.

**Convention.** The controller holds `G = E[X X^H]` per line. The source
files do not document theirs; the usual scientific-Python estimator gives
the conjugate (the Bendat and Piersol form), so `conjugate_on_import`
defaults to True, and the choice is written into the provenance. A test
with a known phase pins the sign: a cross term at `-phi` in the file
arrives at `+phi`.

The controller validates the reference again when it reads it; this module
refuses early what it can (shape, finiteness, Hermitian symmetry, positive
semi-definiteness, the band) so the message comes with the conversion. A
line that is not positive semi-definite can instead be repaired with
`repair=True`, as the controller repairs one, and the repair is recorded.
"""

from __future__ import annotations

import hashlib
import json
import os
import struct
import tempfile
import time
from dataclasses import dataclass
from typing import Any

import numpy as np

#: the file names inside a reference folder
BINARY_FILE = 'reference.bin'
SIDECAR_FILE = 'reference.json'
#: the format the sidecar names, and its version
FORMAT = 'hertzforge-reference'
FORMAT_VERSION = 1
MAGIC = b'HFREF01\x00'
CONVENTION = 'G = E[X X^H] per line'

#: a file frequency matches a grid line within this fraction of the spacing
MATCH_REL_TOL = 1e-9
#: an imported matrix is refused above this relative Hermitian error
HERMITIAN_REL_TOL = 1e-9
#: a line whose smallest eigenvalue is below this fraction of its largest
#: is not positive semi-definite
PSD_CLAMP_REL = 1e-6
#: the reject threshold recorded for a repair accepted at any depth (the
#: largest double, as the controller records it)
ANY_DEPTH = float(np.finfo(np.float64).max)

#: the limit arrays of a specification file, and the tolerance each becomes
LIMITS = {'warning_upper': 'alarm_upper', 'warning_lower': 'alarm_lower',
          'abort_upper': 'abort_upper', 'abort_lower': 'abort_lower'}
#: the order the tolerances are written in
TOLERANCES = ('alarm_upper', 'alarm_lower', 'abort_upper', 'abort_lower')
#: the variables a specification `.mat` file may hold
MAT_KEYS = {'cpsd', 'f', 'dof', *LIMITS}


@dataclass
class Transformation:
    """A transformation matrix with its row and column labels:
    `(lines, rows, cols)` complex, one line or one per grid line."""

    matrix: np.ndarray
    row_labels: list[str]
    col_labels: list[str]


@dataclass
class NativeReference:
    """The content of a native reference file.

    Attributes
    ----------
    sdm : numpy.ndarray
        `(lines, n, n)` complex, Hermitian per line, `E[X X^H]`.
    df_hz : float
        The line spacing; line `k` is at `k * df_hz`.
    band : tuple of int
        The half-open band `(lo, hi)` in lines.
    labels : list of str
        The control-space labels.
    unit : str
        The control unit (the SDM is this unit squared per hertz).
    tolerances : dict of str to numpy.ndarray
        `TOLERANCES` by name, each `(lines, n)`, `NaN` for no line.
    provenance : dict
        How, from what, when and by which version it was made.
    """

    sdm: np.ndarray
    df_hz: float
    band: tuple[int, int]
    labels: list[str]
    unit: str
    tolerances: dict[str, np.ndarray]
    provenance: dict[str, Any]
    response_transformation: Transformation | None = None
    drive_transformation: Transformation | None = None


def _tool() -> str:
    from .. import __version__
    return f'hertzforge {__version__}'


def _sha256(data: bytes) -> str:
    return 'sha256:' + hashlib.sha256(data).hexdigest()


# ---- the binary layout ---------------------------------------------------

def _string(out: bytearray, text: str) -> None:
    raw = text.encode('utf-8')
    out += struct.pack('<Q', len(raw)) + raw
    out += b'\x00' * (-len(raw) % 8)


def _complex(values: np.ndarray) -> bytes:
    return np.ascontiguousarray(values, dtype='<c16').tobytes()


def encode(ref: NativeReference) -> bytes:
    """The bytes of `reference.bin` for a reference."""
    lines, n, _ = ref.sdm.shape
    out = bytearray(MAGIC)
    out += struct.pack('<4Q', lines, n, ref.band[0], ref.band[1])
    out += struct.pack('<d', ref.df_hz)
    flags = int(ref.response_transformation is not None) \
        | (int(ref.drive_transformation is not None) << 1)
    out += struct.pack('<Q', flags)
    _string(out, ref.unit)
    for label in ref.labels:
        _string(out, label)
    if ref.response_transformation is not None:
        t = ref.response_transformation
        out += struct.pack('<3Q', *t.matrix.shape)
        for label in t.col_labels:
            _string(out, label)
    if ref.drive_transformation is not None:
        t = ref.drive_transformation
        out += struct.pack('<3Q', *t.matrix.shape)
        for label in [*t.row_labels, *t.col_labels]:
            _string(out, label)
    upper = np.triu_indices(n)
    out += _complex(ref.sdm[:, upper[0], upper[1]])
    for name in TOLERANCES:
        out += np.ascontiguousarray(ref.tolerances[name], dtype='<f8').tobytes()
    for t in (ref.response_transformation, ref.drive_transformation):
        if t is not None:
            out += _complex(t.matrix)
    return bytes(out)


class _Reader:
    def __init__(self, data: bytes) -> None:
        self.data, self.at = data, 0

    def take(self, size: int) -> bytes:
        if size < 0 or self.at + size > len(self.data):
            raise ValueError('the reference file is damaged or truncated; '
                             'write it again from its source')
        chunk = self.data[self.at:self.at + size]
        self.at += size
        return chunk

    def u64(self, count: int = 1) -> tuple[int, ...]:
        return struct.unpack(f'<{count}Q', self.take(8 * count))

    def string(self) -> str:
        (size,) = self.u64()
        text = self.take(size).decode('utf-8')
        self.take(-size % 8)
        return text

    def array(self, dtype: str, count: int) -> np.ndarray:
        size = np.dtype(dtype).itemsize
        return np.frombuffer(self.take(size * count), dtype=dtype).copy()


def decode(data: bytes) -> NativeReference:
    """A reference from the bytes of `reference.bin` (provenance empty)."""
    r = _Reader(data)
    if r.take(8) != MAGIC:
        raise ValueError('not a reference file of a version this software reads')
    lines, n, lo, hi = r.u64(4)
    (df_hz,) = struct.unpack('<d', r.take(8))
    (flags,) = r.u64()
    unit = r.string()
    labels = [r.string() for _ in range(n)]
    resp = drive = None
    if flags & 1:
        dims = r.u64(3)
        resp = (dims, [r.string() for _ in range(dims[2])])
    if flags & 2:
        dims = r.u64(3)
        drive = (dims, [r.string() for _ in range(dims[1])],
                 [r.string() for _ in range(dims[2])])
    packed = r.array('<c16', lines * n * (n + 1) // 2).reshape(lines, -1)
    upper = np.triu_indices(n)
    sdm = np.zeros((lines, n, n), dtype=np.complex128)
    sdm[:, upper[0], upper[1]] = packed
    sdm[:, upper[1], upper[0]] = np.conj(packed)
    tolerances = {name: r.array('<f8', lines * n).reshape(lines, n)
                  for name in TOLERANCES}
    response = drive_t = None
    if resp is not None:
        dims, cols = resp
        response = Transformation(r.array('<c16', int(np.prod(dims))).reshape(dims),
                                  list(labels), cols)
    if drive is not None:
        dims, rows, cols = drive
        drive_t = Transformation(r.array('<c16', int(np.prod(dims))).reshape(dims),
                                 rows, cols)
    if r.at != len(data):
        raise ValueError('the reference file has bytes after its last array')
    return NativeReference(sdm, df_hz, (lo, hi), labels, unit, tolerances, {},
                           response, drive_t)


# ---- the folder ------------------------------------------------------------

def sidecar(ref: NativeReference, binary: bytes) -> dict[str, Any]:
    """The sidecar for a reference, as the controller reads it: a capture
    snapshot envelope whose payload describes the binary."""
    def dims(t: Transformation | None) -> list[int] | None:
        return None if t is None else [int(d) for d in t.matrix.shape]

    payload = {
        'format': FORMAT,
        'format_version': FORMAT_VERSION,
        'binary_file': BINARY_FILE,
        'binary_bytes': len(binary),
        'content_hash': _sha256(binary),
        'convention': CONVENTION,
        'lines': int(ref.sdm.shape[0]),
        'n': int(ref.sdm.shape[1]),
        'df_hz': float(ref.df_hz),
        'band': [int(ref.band[0]), int(ref.band[1])],
        'labels': list(ref.labels),
        'unit': ref.unit,
        'space': 'physical' if ref.response_transformation is None else 'virtual',
        'response_transformation': dims(ref.response_transformation),
        'drive_transformation': dims(ref.drive_transformation),
        'provenance': ref.provenance,
    }
    return {'unix_secs': int(ref.provenance.get('created_unix_secs', 0)),
            'settings_json': '', 'marks': [], 'payload': payload}


def write(ref: NativeReference, folder: str | os.PathLike) -> str:
    """Write a reference into `folder` (created if needed), the binary first
    and the sidecar last. Returns the content hash."""
    folder = os.fspath(folder)
    os.makedirs(folder, exist_ok=True)
    binary = encode(ref)
    side = sidecar(ref, binary)
    for name, data in ((BINARY_FILE, binary),
                       (SIDECAR_FILE, json.dumps(side, indent=2).encode('utf-8'))):
        path = os.path.join(folder, name)
        with open(path + '.partial', 'wb') as f:
            f.write(data)
        os.replace(path + '.partial', path)
    return side['payload']['content_hash']


def read(folder: str | os.PathLike) -> NativeReference:
    """Read a reference folder, checking the content hash and that the
    sidecar describes the binary.

    Raises ValueError when either check fails.
    """
    folder = os.fspath(folder)
    with open(os.path.join(folder, SIDECAR_FILE), encoding='utf-8') as f:
        side = json.load(f)['payload']
    if side.get('format') != FORMAT or side.get('format_version') != FORMAT_VERSION:
        raise ValueError(f'{folder}: not a version {FORMAT_VERSION} {FORMAT} file')
    with open(os.path.join(folder, BINARY_FILE), 'rb') as f:
        binary = f.read()
    if _sha256(binary) != side['content_hash']:
        raise ValueError(f'{folder}: the binary does not match the content hash '
                         'its sidecar records; write it again from its source')
    ref = decode(binary)
    if (list(ref.band) != side['band'] or ref.labels != side['labels']
            or ref.df_hz != side['df_hz'] or ref.unit != side['unit']):
        raise ValueError(f'{folder}: the sidecar does not describe the binary beside it')
    ref.provenance = side['provenance']
    return ref


# ---- building a reference from a specification ----------------------------

def _match_lines(freqs: np.ndarray, df_hz: float, lines: int, name: str) -> np.ndarray:
    """The grid line of each file frequency, refusing one off the grid."""
    out = []
    used: dict[int, int] = {}
    for idx, f in enumerate(np.asarray(freqs, dtype=np.float64)):
        if not np.isfinite(f) or f < 0:
            raise ValueError(f'{name}: frequency {idx} is {f}; every frequency must be a '
                             'number at or above 0')
        k = round(f / df_hz)
        if abs(f - k * df_hz) > MATCH_REL_TOL * df_hz:
            raise ValueError(
                f'{name}: the line at {f:g} Hz (entry {idx}) is not on the controller\'s '
                f'grid of {df_hz:g} Hz lines; the file\'s spacing must be a whole multiple '
                'of the grid\'s, so convert at a spacing that divides it')
        if k >= lines:
            raise ValueError(f'{name}: the line at {f:g} Hz (entry {idx}) is above the '
                             f'grid\'s last line ({(lines - 1) * df_hz:g} Hz)')
        if k in used:
            raise ValueError(f'{name}: entries {used[k]} and {idx} fall on one line')
        used[k] = idx
        out.append(k)
    return np.asarray(out, dtype=int)


def _coherence(g: np.ndarray) -> np.ndarray:
    d = np.real(np.diag(g))
    scale = np.outer(d, d)
    with np.errstate(divide='ignore', invalid='ignore'):
        return np.where(scale > 0, np.abs(g) ** 2 / scale, 0.0)


def _repair_line(sdm: np.ndarray, k: int, values: np.ndarray,
                 vectors: np.ndarray) -> tuple[int, float, float]:
    """Clamp line `k`'s negative eigenvalues to zero and rescale its diagonal
    back to what it was. Returns the eigenvalues clamped, the smallest
    relative eigenvalue before, and the largest change in squared
    coherence."""
    before = sdm[k].copy()
    want = np.real(np.diag(before))
    clamped = int(np.sum(values < -PSD_CLAMP_REL * values[-1]))
    rebuilt = (vectors * np.maximum(values, 0.0)) @ vectors.conj().T
    have = np.real(np.diag(rebuilt))
    s = np.where(have > 0, want / np.where(have > 0, have, 1.0), 0.0)
    sdm[k] = np.sqrt(np.outer(s, s)) * rebuilt
    change = float(np.max(np.abs(_coherence(sdm[k]) - _coherence(before))))
    return clamped, float(values[0] / values[-1]), change


def build(cpsd: np.ndarray, frequencies: np.ndarray, *, df_hz: float, lines: int,
          labels: list[str], unit: str, limits: dict[str, np.ndarray] | None = None,
          conjugate_on_import: bool = True, method: str, source: str,
          source_sha256: str | None = None, repair: bool = False,
          created_unix_secs: int | None = None) -> NativeReference:
    """A reference on the controller's grid from a specification's arrays.

    Parameters
    ----------
    cpsd : numpy.ndarray
        `(lines in the file, n, n)`, the file's matrices.
    frequencies : numpy.ndarray
        The file's lines in hertz.
    df_hz, lines : float, int
        The controller's grid.
    labels : list of str
        One per channel, in the file's order.
    unit : str
        The channels' unit; the file cannot state one.
    limits : dict, optional
        `LIMITS` keys, each `(lines in the file, n)`.
    conjugate_on_import : bool
        Conjugate every cross term (see the module notes).
    method, source, source_sha256 : str
        For the provenance.
    repair : bool
        Repair a line that is not positive semi-definite instead of refusing
        it: its negative eigenvalues are set to zero and its diagonal is
        rescaled to the specified PSDs (the controller's repair, at any
        depth), and the change is recorded in the provenance. Off by default,
        so a user sees the refusal before accepting a changed reference.
    created_unix_secs : int, optional
        The time to record; now when not given.

    Raises
    ------
    ValueError
        For a shape, a frequency off the grid, a value that is not finite, a
        matrix that is not Hermitian or not positive semi-definite, or a
        channel with no level inside the band; each message says which.
    """
    cpsd = np.asarray(cpsd)
    frequencies = np.asarray(frequencies, dtype=np.float64).reshape(-1)
    nf = len(frequencies)
    if cpsd.ndim != 3 or cpsd.shape[0] != nf or cpsd.shape[1] != cpsd.shape[2] or nf == 0:
        raise ValueError(f'{source}: cpsd is {cpsd.shape}; it must be lines by channels by '
                         f'channels ({nf} by n by n), one matrix per frequency in f')
    n = cpsd.shape[1]
    if len(labels) != n or len(set(labels)) != n or any(not str(x).strip() for x in labels):
        raise ValueError(f'{source}: {len(labels)} labels for {n} channels; give one '
                         'distinct name per channel in the file\'s order')
    if not np.all(np.isfinite(cpsd)):
        raise ValueError(f'{source}: cpsd holds a value that is not a finite number')
    at = _match_lines(frequencies, df_hz, lines, source)
    sdm = np.zeros((lines, n, n), dtype=np.complex128)
    sdm[at] = np.conj(cpsd) if conjugate_on_import else cpsd
    for k in at:
        g = sdm[k]
        norm = np.linalg.norm(g)
        if norm and np.linalg.norm(g - g.conj().T) / norm > HERMITIAN_REL_TOL:
            raise ValueError(f'{source}: the matrix at {k * df_hz:g} Hz is not Hermitian; '
                             'each cross term must be the conjugate of its mirror')
    sdm = 0.5 * (sdm + np.conj(np.swapaxes(sdm, 1, 2)))
    diag = np.real(np.einsum('kii->ki', sdm))
    nonzero = np.flatnonzero(np.any(diag != 0, axis=1))
    if not len(nonzero):
        raise ValueError(f'{source}: no line has a level')
    band = (int(nonzero[0]), int(nonzero[-1]) + 1)
    repaired: list[tuple[int, float, float]] = []
    for k in range(*band):
        for i in range(n):
            if diag[k, i] <= 0:
                raise ValueError(
                    f'{source}: channel {labels[i]} has no level at {k * df_hz:g} Hz, inside '
                    'the band; every channel needs a positive PSD across the band (a file '
                    'coarser than the grid leaves the lines between its own at zero)')
        values, vectors = np.linalg.eigh(sdm[k])
        if values[0] < -PSD_CLAMP_REL * values[-1]:
            if not repair:
                raise ValueError(
                    f'{source}: the matrix at {k * df_hz:g} Hz is not positive '
                    f'semi-definite (smallest eigenvalue {values[0]:.4g} against largest '
                    f'{values[-1]:.4g}); correct the coherences and phases, or convert with '
                    'repair=True and review the coherence changes it records')
            repaired.append(_repair_line(sdm, k, values, vectors))
    tolerances = {name: np.full((lines, n), np.nan) for name in TOLERANCES}
    for key, values in (limits or {}).items():
        values = np.asarray(values, dtype=np.float64)
        if values.shape != (nf, n):
            raise ValueError(f'{source}: {key} is {values.shape}; it must be lines by '
                             f'channels ({nf} by {n})')
        tolerances[LIMITS[key]][at] = values
    convention = (
        'the file\'s cross-spectral convention is not documented; it was taken as the '
        'Bendat and Piersol form, the conjugate of E[X X^H], and every cross term was '
        'conjugated' if conjugate_on_import else
        'the file\'s cross-spectral convention is not documented; it was taken as '
        'E[X X^H] and read unchanged')
    created = int(time.time()) if created_unix_secs is None else int(created_unix_secs)
    provenance = {
        'method': method,
        'source': source,
        'source_sha256': source_sha256,
        'created_unix_secs': created,
        'tool': _tool(),
        'conjugated_on_import': bool(conjugate_on_import),
        'assumptions': [
            convention,
            (f'file frequencies placed on grid lines within {MATCH_REL_TOL:g} of the '
             f'{df_hz:g} Hz spacing; grid lines absent from the file are zero'),
            'rows and columns taken in the control channels\' order',
        ],
        'repair': None if not repaired else {
            'lines_repaired': len(repaired),
            'eigenvalues_clamped': int(sum(r[0] for r in repaired)),
            'worst_relative_eigenvalue': float(min(r[1] for r in repaired)),
            'max_coherence_change': float(max(r[2] for r in repaired)),
            'clamp_rel': PSD_CLAMP_REL,
            'reject_rel': ANY_DEPTH,
        },
    }
    return NativeReference(sdm, float(df_hz), band, [str(x) for x in labels], unit,
                           tolerances, provenance)


def _file_hash(path: str) -> str:
    with open(path, 'rb') as f:
        return _sha256(f.read())


def _mat_variables(path: str) -> dict[str, np.ndarray]:
    """The variables of a `.mat` file, MATLAB's own axis order, v5 or v7.3
    (`dof` from v5 only)."""
    import h5py

    if h5py.is_hdf5(path):
        out = {}
        with h5py.File(path, 'r') as f:
            for key in f:
                # '#refs#' is MATLAB's own; names in a v7.3 file are cell
                # references, which are not read: pass the labels instead
                if key.startswith('#') or key == 'dof':
                    continue
                data = f[key]
                if not isinstance(data, h5py.Dataset):
                    raise TypeError(f'{os.path.basename(path)}: {key} is not a plain '
                                    'array')
                values = data[()]
                if values.dtype.names and {'real', 'imag'} <= set(values.dtype.names):
                    values = values['real'] + 1j * values['imag']
                # HDF5 holds MATLAB's column-major array with its axes reversed
                out[key] = np.asarray(values).T
        return out
    from scipy.io import loadmat

    return {k: v for k, v in loadmat(path).items() if not k.startswith('__')}


def _names(values: Any) -> list[str]:
    """Channel names from a `.mat` character matrix or cell array."""
    out = []
    for item in np.asarray(values, dtype=object).ravel():
        while isinstance(item, np.ndarray):
            item = item.ravel()[0] if item.size else ''
        out.append(str(item).strip())
    return out


def from_mat(path: str | os.PathLike, folder: str | os.PathLike, *, df_hz: float,
             lines: int, labels: list[str] | None = None, unit: str = 'g',
             conjugate_on_import: bool = True, repair: bool = False,
             created_unix_secs: int | None = None) -> str:
    """Convert a specification `.mat` file into a native reference folder.

    The file holds `cpsd` (channels by channels by lines), `f` (the lines in
    hertz) and optionally the four limits (channels by lines), the layout's
    arrays transposed from the `.npz` form. Returns the content hash.

    Raises ValueError for a missing or unknown variable or any refusal of
    `build`, and TypeError for a v7.3 variable that is not a plain array.
    """
    path = os.fspath(path)
    name = os.path.basename(path)
    variables = _mat_variables(path)
    unknown = sorted(set(variables) - MAT_KEYS)
    if unknown:
        raise ValueError(f'{name}: holds {unknown[0]!r}, which is not part of the '
                         f'specification layout ({", ".join(sorted(MAT_KEYS))})')
    for key in ('cpsd', 'f'):
        if key not in variables:
            raise ValueError(f'{name}: has no {key!r}; a specification holds at least '
                             'cpsd and f')
    m = np.asarray(variables['cpsd'])
    if m.ndim == 2:            # one line: MATLAB drops the trailing 1
        m = m[:, :, np.newaxis]
    if m.ndim != 3:
        raise ValueError(f'{name}: cpsd is {m.shape}; it must be channels by channels by '
                         'lines')
    cpsd = np.moveaxis(m, 2, 0)
    n = cpsd.shape[1]
    limits = {}
    for key in LIMITS:
        if key in variables:
            values = np.asarray(variables[key], dtype=np.float64)
            limits[key] = values.T if values.ndim == 2 else values.reshape(-1, n)
    if labels is None:
        if 'dof' in variables:
            labels = _names(variables['dof'])
        else:
            labels = [str(k + 1) for k in range(n)]
    ref = build(cpsd, np.asarray(variables['f']).ravel(), df_hz=df_hz, lines=lines,
                labels=labels, unit=unit, limits=limits,
                conjugate_on_import=conjugate_on_import, method='specification_mat',
                source=name, source_sha256=_file_hash(path), repair=repair,
                created_unix_secs=created_unix_secs)
    return write(ref, folder)


def from_hforge(path: str | os.PathLike, folder: str | os.PathLike, *, df_hz: float,
                lines: int, unit: str | None = None, conjugate_on_import: bool = True,
                repair: bool = False, created_unix_secs: int | None = None) -> str:
    """Convert a `.hforge` file holding a random `Specification` into a
    native reference folder. Its matrix is formed as the controller's
    specification writer forms it (pairs not held are zero), its values are
    taken as stored, and its channel names become the labels. Returns the
    content hash.

    Raises TypeError when the file holds something other than a
    specification, and ValueError for any refusal of `build`.
    """
    from ..core.data import Specification
    from . import native, rattlesnake_specification

    path = os.fspath(path)
    name = os.path.basename(path)
    spec = native.load(path)
    if not isinstance(spec, Specification):
        raise TypeError(f'{name}: holds {type(spec).__name__}, not a random '
                        'specification')
    with tempfile.TemporaryDirectory() as scratch:
        npz = os.path.join(scratch, 'specification.npz')
        rattlesnake_specification.save(spec, npz)
        with np.load(npz, allow_pickle=False) as d:
            arrays = {k: np.asarray(d[k]) for k in d.files}
    if unit is None:
        units = {u for u in (spec.ordinate_unit or []) if u}
        unit = units.pop() if len(units) == 1 else 'as stored'
    ref = build(arrays['cpsd'], arrays['f'], df_hz=df_hz, lines=lines,
                labels=[str(x) for x in arrays['dof']], unit=unit,
                limits={k: arrays[k] for k in LIMITS if k in arrays},
                conjugate_on_import=conjugate_on_import, method='hforge', source=name,
                source_sha256=_file_hash(path), repair=repair,
                created_unix_secs=created_unix_secs)
    return write(ref, folder)
