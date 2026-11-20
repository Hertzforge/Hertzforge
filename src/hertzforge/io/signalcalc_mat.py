"""Time histories from SignalCalc's MATLAB export (`.mat`, version 7.3).

    found = hertzforge.import_file('run.mat')
    history, table = found['time_data'], found['channel_table']
    part = hertzforge.import_file('run.mat', start=120, stop=130,
                                  channels=['A6501- Z', 'Current'])

SignalCalc writes a recorded time history to MATLAB as an HDF5 file
(MATLAB's version 7.3) holding, for N channels:

- `INP_1_Data` to `INP_N_Data`, one vector of samples per channel;
- `ChanName` and `Unit`, character matrices with one row per channel,
  padded with NUL characters;
- `hDelta`, the time step in seconds;
- `Sensitivity` and `InputChanNums`, one number per channel, and
  `EUType`, a code per channel.

`INP_k_Data` holds the channel named in row k of `ChanName`. It is not
the channel whose `InputChanNums` entry is k: the input numbers follow
the analyzer's wiring and need not be in row order, and a shaker's
current and voltage monitors read by row are coherent with each other
the way a shaker's are, where read by input number they are two
accelerometers. The values are in each channel's engineering unit,
which `Unit` names; the sensitivity is kept for reference, not applied.

What is read: one `TimeHistory` with a record per channel, starting at
0 s, and a `ChannelTable` with each channel's unit, quantity and
sensitivity, its comment holding its name and input number. A name that
is a DOF (`101X+`) becomes the record's response DOF; any other name
(`A6501- Z`, `Current`) labels the record and leaves its DOF to be set
in the channel table, since splitting a sensor's name into a node and a
direction would be a guess. A unit Hertzforge reads is declared,
converting the values to SI; one it does not is left undeclared with
the file's numbers, and the import says so. `EUType` is not read: no
public description of its codes was found.

`start`, `stop` and `channels` read part of a long export, as for a
Rattlesnake run (`rattlesnake.load`): the window is inclusive at both
ends on the file's clock, and only the samples it holds are read from
the file, a channel at a time.

Refused, each by name: an `hDelta` that is not one positive time step,
a channel vector that is a matrix or of another length than the rest, a
`Unit`, `Sensitivity` or `InputChanNums` whose count differs from the
names', an `INP_k_Data` with no row k and a row with no `INP_k_Data`.
A missing sample is kept as NaN and noted, and so is a channel that
never varies (`core.channel_health`).

`save` writes a time history back in the same layout, which this module
reads back; whether SignalCalc's own MATLAB import takes it has not
been tested. The format has no start time, so a history that starts
later than 0 s reads back starting at 0 s.
"""

from __future__ import annotations

import os
import re
import warnings
from collections.abc import Callable, Iterable
from dataclasses import replace
from typing import Any

import numpy as np

from .notes import ImportNote
from .stated_units import declare_stated_units

SUFFIX = '.mat'
#: the variables every export has, besides its channels
NEEDED = ('ChanName', 'Unit', 'hDelta')
_CHANNEL = re.compile(r'^INP_(\d+)_Data$')
#: MATLAB's version 7.3 header: 116 bytes of text, an 8-byte subsystem
#: offset (none), the version (0x0200) and the byte order mark
_HEADER_TEXT = b'MATLAB 7.3 MAT-file, Platform: hertzforge, HDF5 schema 1.00 .'


def sniff(path: str | os.PathLike) -> bool:
    """Whether `path` is a SignalCalc time history export: an HDF5 `.mat`
    holding `ChanName`, `Unit`, `hDelta` and at least one
    `INP_<n>_Data`. Only the names of the file's variables are read."""
    if not str(path).lower().endswith(SUFFIX):
        return False
    try:
        import h5py

        if not h5py.is_hdf5(path):
            return False
        with h5py.File(path, 'r') as f:
            names = set(f)
    except Exception:  # noqa: BLE001 - sniffers must not raise on foreign files
        return False
    return (set(NEEDED) <= names
            and any(_CHANNEL.match(name) for name in names))


# ---- reading ------------------------------------------------------------------

def _text_rows(f: Any, name: str) -> list[str]:
    """A MATLAB character matrix as its rows, the NUL and space padding
    taken off. HDF5 holds the matrix transposed, as uint16 codes."""
    item = f[name]
    if item.attrs.get('MATLAB_class', b'') not in (b'char', 'char'):
        raise ValueError(f'{name} is not a character matrix')
    codes = np.atleast_2d(item[()])
    if codes.size == 0:
        return []
    return [''.join(map(chr, row)).strip('\x00 ') for row in codes.T]


def _per_channel(f: Any, name: str, count: int) -> np.ndarray | None:
    """One number per channel from a numeric variable, or None when the
    file has no `name`."""
    if name not in f:
        return None
    values = np.asarray(f[name][()], dtype=float).ravel()
    if values.size != count:
        raise ValueError(f'{name} holds {values.size} values for {count} '
                         'channels named in ChanName')
    return values


def _time_step(f: Any) -> float:
    if 'hDelta' not in f:
        raise ValueError('there is no hDelta, the time step')
    values = np.asarray(f['hDelta'][()], dtype=float).ravel()
    if values.size != 1:
        raise ValueError(f'hDelta holds {values.size} values; it is one time step')
    step = float(values[0])
    if not (np.isfinite(step) and step > 0):
        raise ValueError(f'hDelta is {step!r}; a time step is a positive number '
                         'of seconds')
    return step


def _channel_vectors(f: Any, count: int) -> list[Any]:
    """`INP_1_Data` to `INP_<count>_Data`, in row order, each checked to
    be one vector, all of one length."""
    found = {int(m.group(1)): name for name in f
             if (m := _CHANNEL.match(name))}
    extra = sorted(k for k in found if not 1 <= k <= count)
    if extra:
        raise ValueError(f'{found[extra[0]]} has no row in ChanName, which names '
                         f'{count} channels')
    missing = [k for k in range(1, count + 1) if k not in found]
    if missing:
        raise ValueError(f'INP_{missing[0]}_Data is missing; ChanName names '
                         f'{count} channels')
    vectors, length = [], None
    for k in range(1, count + 1):
        dataset = f[found[k]]
        if dataset.ndim != 2 or 1 not in dataset.shape:
            raise ValueError(f'{found[k]} is a {dataset.shape} matrix; a channel '
                             'is one vector')
        if dataset.dtype.kind not in 'fiu':
            raise ValueError(f'{found[k]} holds {dataset.dtype}, not real numbers')
        samples = max(dataset.shape)
        if length is None:
            length = samples
        elif samples != length:
            raise ValueError(f'{found[k]} holds {samples} samples where '
                             f'{found[1]} holds {length}')
        vectors.append(dataset)
    return vectors


def _read_span(dataset: Any, first: int, last: int) -> np.ndarray:
    # MATLAB's n×1 column is (1, n) in HDF5, and a 1×n row is (n, 1)
    if dataset.shape[0] == 1:
        return dataset[0, first:last]
    return dataset[first:last, 0]


def load(path: str | os.PathLike, start: float | None = None,
         stop: float | None = None,
         channels: Iterable[int | str] | None = None,
         progress: Callable[[int, int], None] | None = None) -> dict[str, Any]:
    """A SignalCalc time history export, read as a time history and its
    channel table.

    Parameters
    ----------
    path : str or os.PathLike
        The `.mat` file.
    start, stop : float, optional
        A window in seconds on the file's clock, which starts at 0;
        either end open when omitted.
    channels : iterable of int or str, optional
        Which channels to read, by 0-based row or by name as the record
        is labeled (its DOF, or its name from `ChanName`). All of them
        when omitted.
    progress : callable, optional
        Called with `(done, total)` samples as the channels are read.

    Returns
    -------
    dict
        'time_data', the `TimeHistory`, and 'channel_table', the
        `ChannelTable` of every channel in the file.

    Raises
    ------
    ValueError
        For anything the module docstring lists as refused.
    """
    import h5py

    from ..core.channel_health import channel_health
    from ..core.channel_table import CHANNEL_TYPES, ChannelTable
    from ..core.data import TimeHistory
    from ..core.progress import Ticker
    from ..core.validate import dofs
    from ..units import dimension_of, from_si
    from .rattlesnake import _channel_rows, _sample_window

    path = str(path)
    base = os.path.basename(path)

    def refused(error: ValueError) -> ValueError:
        return ValueError(f'{base}: {error}')

    with h5py.File(path, 'r') as f:
        try:
            names = _text_rows(f, 'ChanName')
            if not names:
                raise ValueError('ChanName names no channels')
            units = _text_rows(f, 'Unit')
            if len(units) != len(names):
                raise ValueError(f'Unit holds {len(units)} units for {len(names)} '
                                 'channels named in ChanName')
            sensitivity = _per_channel(f, 'Sensitivity', len(names))
            inputs = _per_channel(f, 'InputChanNums', len(names))
            step = _time_step(f)
            vectors = _channel_vectors(f, len(names))
        except ValueError as error:
            raise refused(error) from None

        response, block = [], []
        for name in names:
            try:
                [dof] = dofs([name])
            except ValueError:
                response.append('')
                block.append(name)
            else:
                response.append(dof)
                block.append('')
        labels = [dof or name for dof, name in zip(response, block, strict=True)]
        samples = max(vectors[0].shape)
        try:
            rows = (list(range(len(names))) if channels is None
                    else _channel_rows(channels, labels))
            span = _sample_window(start, stop, samples, 1.0 / step)
        except ValueError as error:
            raise refused(error) from None
        if span is None:
            raise refused(ValueError(f'the window holds fewer than two of its '
                                     f'{samples} samples'))
        first, last = span
        ticker = Ticker(progress)
        ticker.add(len(rows) * (last - first))
        values = np.empty((len(rows), last - first), dtype=np.float64)
        for position, row in enumerate(rows):
            values[position] = _read_span(vectors[row], first, last)
            ticker.tick(last - first)

    window = ('' if (first, last) == (0, samples) else
              f'{first * step:g} to {(last - 1) * step:g} s of ')
    history = TimeHistory(
        np.arange(first, last) * step, values,
        response_dof=[response[row] for row in rows],
        block=[block[row] for row in rows] if any(block[row] for row in rows) else None,
        comment=[f'{window}INP_{row + 1}_Data of {base}' for row in rows])
    notes = []
    chosen = [units[row] for row in rows]
    for k, unit, reason in declare_stated_units(history, chosen):
        notes.append(f'{labels[rows[k]]}: the unit {unit!r} {reason}; left '
                     "undeclared, holding the file's numbers")
    for k, unit in enumerate(chosen):
        if not unit:
            notes.append(f'{labels[rows[k]]}: the file states no unit; left '
                         "undeclared, holding the file's numbers")
    for finding in channel_health(history):
        unit = history.ordinate_unit[finding.record]
        if finding.dead and unit:
            finding = replace(finding, constant=float(from_si(finding.constant, unit)))
        notes.append(finding.describe(history.record_label(finding.record),
                                      history.abscissa, unit))
    for note in notes:
        warnings.warn(ImportNote(f'{base}: {note}'), stacklevel=2)

    quantity = [dimension_of(unit) if unit else None for unit in units]
    table = ChannelTable({
        'channel': np.arange(1, len(names) + 1),
        'node': [_node(dof) for dof in response],
        'direction': [_direction(dof) for dof in response],
        'unit': units,
        'channel_type': [q if q in CHANNEL_TYPES else '' for q in quantity],
        'sensitivity': (['' if not np.isfinite(s) else repr(float(s)) for s in sensitivity]
                        if sensitivity is not None else [''] * len(names)),
        'comment': [name if inputs is None else f'{name}, input {int(inputs[k])}'
                    for k, name in enumerate(names)],
    })
    return {'time_data': history, 'channel_table': table}


def _node(dof: str) -> str:
    """The node of a DOF string as a channel table cell, blank for none."""
    from ..core.data import parse_dof

    node, _ = parse_dof(dof) if dof else (None, '')
    return '' if node is None else str(node)


def _direction(dof: str) -> str:
    """The direction of a DOF string as a channel table cell, blank for
    none."""
    from ..core.data import parse_dof

    return parse_dof(dof)[1] if dof else ''


# ---- writing --------------------------------------------------------------------

def handles(obj: Any) -> bool:
    """Whether `save` can write `obj`: a time history."""
    from ..core.data import TimeHistory

    return isinstance(obj, TimeHistory)


def _char_matrix(f: Any, name: str, rows: list[str]) -> None:
    """Rows of text as a MATLAB character matrix, NUL-padded as
    SignalCalc pads its own."""
    width = max([len(row) for row in rows] + [1])
    codes = np.zeros((len(rows), width), dtype=np.uint16)
    for k, row in enumerate(rows):
        codes[k, :len(row)] = [ord(c) for c in row]
    dataset = f.create_dataset(name, data=codes.T)
    dataset.attrs['MATLAB_class'] = np.bytes_(b'char')
    dataset.attrs['MATLAB_int_decode'] = np.int32(2)


def _double(f: Any, name: str, values: Any) -> None:
    # a MATLAB column of n is (1, n) in HDF5
    dataset = f.create_dataset(name, data=np.asarray(values, dtype=np.float64).reshape(1, -1))
    dataset.attrs['MATLAB_class'] = np.bytes_(b'double')


def save(history: Any, path: str | os.PathLike, unit_system: Any = None,
         **_kwargs: Any) -> None:
    """Write a time history as a SignalCalc time history export, which
    `load` reads back.

    One `INP_k_Data` per record, `ChanName` from each record's label (its
    DOF, or its block), `Unit` naming each record's unit and `hDelta`
    the time step. With `unit_system` each record is written in that
    system's unit for its quantity (g included, since `Unit` can say g);
    without one, as stored, in SI. A record with no units is written as
    its numbers with an empty unit. `Sensitivity`, `InputChanNums` and
    `EUType` are not written: a time history does not hold them. The
    start time is not kept, since the format has no place for one.

    Parameters
    ----------
    history : TimeHistory
        What to write.
    path : str or os.PathLike
        Where; `.mat` is added when it does not end in it.
    unit_system : UnitSystem, optional
        The units to write in.

    Returns
    -------
    None

    Raises
    ------
    ValueError
        When the history is not evenly sampled.
    """
    import h5py

    from ..units import SI, UNKNOWN

    system = SI if unit_system is None else unit_system
    step = 1.0 / history.sample_rate
    path = str(path)
    if not path.lower().endswith(SUFFIX):
        path += SUFFIX
    names, units = [], []
    ordinate = np.real(np.asarray(history.ordinate))
    with h5py.File(path, 'w', userblock_size=512) as f:
        for i in range(history.num_records):
            names.append(history.record_label(i) or f'record {i + 1}')
            dimension = history.ordinate_dim[i]
            if dimension == UNKNOWN:
                units.append('')
                values = ordinate[i]
            else:
                units.append(system.unit(dimension))
                values = system.from_si(ordinate[i], dimension)
            _double(f, f'INP_{i + 1}_Data', values)
        _char_matrix(f, 'ChanName', names)
        _char_matrix(f, 'Unit', units)
        _double(f, 'hDelta', [step])
    with open(path, 'r+b') as fh:
        fh.write(_HEADER_TEXT.ljust(116, b' ') + bytes(8) + b'\x00\x02IM')
