"""A Modal Test run folder's results as analysis objects.

`run_convert.convert` hands a Modal Test run (`WIRE['test']`) here for
its measurements; everything else about the run (the channel table, the
run record, the object group, the file) is converted as every run's is.
What a Modal Test run becomes:

- **Measurement frames**: a `TimeHistory` per measurement holding its
  accepted frames, a record per channel per frame, each channel at the
  DOF it had in that measurement (a roving channel at the node of the
  point measured), in SI, timed from the trigger (an impact's
  pre-trigger samples at negative times) or from the frame's start.
  Trial hits and rejected frames are not in it: they stay in the input
  stream, and the run record's frame lines say what each was and why it
  was rejected. One history per measurement, not per frame: a survey of
  a hundred points at five hits is a hundred objects, not five hundred,
  and the frames of one measurement share their DOFs and their length,
  which is what an estimate of them needs (`core.modal_test.reprocess`).
  A drive stream's frames are in the same history, as monitors.
- **modal_frf**: one `Frf` of every measurement's FRFs, each record a
  response against a reference at the DOFs the measurement named, in SI
  from the channel rows' units; a point measured again replaces what the
  earlier measurement gave. Each record's comment says the estimator,
  the averages, the measurement and point, and the windows (kinds,
  widths, the exponential window's time constant and the decay rate it
  added). The averaging holds the block, the averages and the
  exponential window by its end value, so the time constant survives a
  save and the fit's damping can be read less it
  (`core.modal_test.frf_tau`).
- **modal_frf fixed reference**: for a test whose excitation roved
  (its roving channels are reference rows; with no roving plan, more
  reference DOFs than response DOFs), the same FRFs turned round
  by reciprocity (`core.modal_test.fixed_reference`), marked so on every
  record, the form a modal fit takes; with its ordinary coherence turned
  the same way.
- **modal_coherence**: the ordinary coherence of each record (one
  reference) as a `Coherence`, or each response's multiple coherence
  (several references) as a `MultipleCoherence`.
- **modal_reference_spectra**: each measurement's reference autospectra
  as a `Psd`, a record per reference per measurement; with several
  references the condition number of their spectral matrix per line
  stays in the run record as an array.
- `modal_frames` and `modal_outcome` stay in the run record whole: the
  frame lines, and the points planned and measured, the frames taken and
  accepted and how the test ended.

Every name read from the folder is in `WIRE`, the one table to change
when the control service's spelling does.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np

from . import run_folder
from .rattlesnake import _stream_offsets, _unit_scale

#: every name this module reads from a Modal Test run folder: the test, its
#: results, the members of their record lines, the windows and the setup
WIRE: dict[str, Any] = {
    'test': 'modal_test',
    # results, by their name in the folder (each repeats: a line per
    # finished measurement, or per captured frame)
    'frf': 'modal_frf',
    'coherence': 'modal_coherence',
    'reference_spectra': 'modal_reference_spectra',
    'frames': 'modal_frames',
    'outcome': 'modal_outcome',
    # the arrays
    'values': 'values',
    'condition': 'condition',
    # a spectral record's line
    'measurement': 'measurement',
    'point': 'point',
    'responses': 'responses',
    'references': 'references',
    'estimator': 'estimator',
    'averages': 'averages',
    'window': 'window',
    'coherence_form': 'coherence',
    # a channel of a record: its table index, node, direction and unit
    'index': 'index',
    'node': 'node',
    'direction': 'direction',
    'unit': 'unit',
    # the window record: by channel side, each with its kind, and the
    # exponential window on every channel
    'window_reference': 'reference',
    'window_response': 'response',
    'window_kind': 'kind',
    'force_width': 'width',
    'force_fall': 'fall',
    'exponential': 'exponential',
    'tau_s': 'tau_s',
    'end': 'end',
    'decay_rate': 'decay_rate_per_s',
    # a frame line
    'stream': 'stream',
    'offset': 'offset',
    'length': 'length',
    'trigger_sample': 'trigger_sample',
    'average': 'average',
    'accepted': 'accepted',
    'trial': 'trial',
    'reason': 'reason',
    # the setup (`setup.json` config)
    'mode': 'mode',
    'block': 'block',
    'roving': 'roving',
    'roving_channels': 'channels',
    'roving_points': 'points',
    'shaker': 'shaker',
    'overlap': 'overlap',
    # the outcome
    'points_planned': 'points_planned',
    'points_measured': 'points_measured',
    # measurements and points are numbered from this in the folder
    'first': 0,
}

#: the name of the FRFs turned round by reciprocity, after the measured
#: result's own
FIXED = 'fixed reference'


def _w(key: str) -> Any:
    return WIRE[key]


def _config(run: run_folder.RunFolder) -> Mapping[str, Any]:
    config = run.setup.get('config') if isinstance(run.setup, Mapping) else None
    return config if isinstance(config, Mapping) else {}


def _number(m: Any) -> int | None:
    return int(m) if isinstance(m, int) and not isinstance(m, bool) else None


def _said(n: int | None) -> str:
    return '?' if n is None else str(n - _w('first') + 1)


class _Channels:
    """The table's channels as a Modal Test run names them: each row's DOF,
    unit, SI scale and analysis role, and a measurement's DOFs."""

    def __init__(self, run: run_folder.RunFolder, table: Any) -> None:
        self.row: dict[int, int] = {int(i): k for k, i in enumerate(table['channel'])}
        self.dofs: list[str] = list(table.dof_strings())
        self.roles: list[str] = table.roles()
        units = {int(r['index']): str(r.get('units', '') or '') for r in run.rows}
        self.unit: dict[int, str] = units
        self.scale: dict[int, tuple[float, str, str]] = {i: _unit_scale(u) for i, u in units.items()}
        config = _config(run)
        roving = config.get(_w('roving')) if isinstance(config.get(_w('roving')),
                                                        Mapping) else {}
        self.roving: list[int] = [int(i) for i in roving.get(_w('roving_channels')) or []
                       if isinstance(i, int)]
        self.points: list[Mapping[str, Any]] = [p for p in roving.get(_w('roving_points')) or []
                       if isinstance(p, Mapping)]

    def dof(self, entry: Mapping[str, Any]) -> str:
        """A record's channel as a DOF string, as `ChannelTable.dof_strings`
        spells one: its node and direction joined, the table's own where
        the record names neither."""
        node, direction = entry.get(_w('node')), entry.get(_w('direction'))
        if node is None and not direction:
            return self.dofs[self.row[int(entry[_w('index')])]]
        table = self.dofs[self.row[int(entry[_w('index')])]]
        return f'{node if node is not None else ""}{direction or ""}' or table

    def planned(self, index: int, point: int | None) -> str:
        """Channel `index`'s DOF in a measurement of `point`, from the roving
        plan: a roving channel takes the point's node, and its direction
        when the point gives one; any other channel keeps the table's."""
        table = self.dofs[self.row[index]]
        if point is None or index not in self.roving:
            return table
        k = point - _w('first')
        if not 0 <= k < len(self.points):
            return table
        given = self.points[k]
        node = given.get(_w('node'))
        direction = given.get(_w('direction')) or self._direction(index)
        return f'{node if node is not None else ""}{direction or ""}'

    def _direction(self, index: int) -> str:
        dof, node = self.dofs[self.row[index]], ''
        for ch in dof:
            if ch.isdigit():
                node += ch
            else:
                break
        return dof[len(node):]


def _unit_refusal(channels: _Channels, entries: Sequence[Mapping[str, Any]]) -> str:
    for entry in entries:
        try:
            index = int(entry[_w('index')])
        except (KeyError, TypeError, ValueError):
            return 'a channel of its records has no table index'
        if index not in channels.unit:
            return f'it names channel {index}, which the table does not'
        stated = entry.get(_w('unit'))
        if stated not in (None, '') and str(stated) != channels.unit[index]:
            return (f'it gives channel {index} the unit {stated!r} and the table '
                    f'{channels.unit[index]!r}')
    return ''


def _measured(result: run_folder.Result | None) -> set[int]:
    return {m for record in ((result.records or []) if result is not None else [])
            if (m := _number(record.get(_w('measurement')))) is not None}


def _standing(run: run_folder.RunFolder) -> tuple[dict[int, str], list[str]]:
    """Which measurements every object leaves out, and why: {measurement:
    'replaced' or 'incomplete'}, and what to say of them.

    One rule for the FRFs, the coherence, the reference spectra and the
    frames: a measurement is complete when it has its FRF records and,
    where the run wrote coherence or reference spectra for any
    measurement, its own of those too. At each point (or, with nothing
    roving, the whole layout) the latest complete measurement stands and
    replaces the earlier ones; a later incomplete one (its coherence or
    spectra never written, as a killed run leaves it) replaces nothing and
    is left out. A point with no complete measurement keeps its latest.
    Frames alone (trial or rejected hits) are no measurement and replace
    nothing.
    """
    frf = run.results.get(_w('frf'))
    has = {'FRFs': _measured(frf)}
    for key, words in (('coherence', 'coherence'), ('reference_spectra', 'reference spectra')):
        result = run.results.get(_w(key))
        if result is not None and result.records:
            has[words] = _measured(result)
    points: dict[Any, list[int]] = {}
    for record in (frf.records or []) if frf is not None else []:
        m = _number(record.get(_w('measurement')))
        if m is not None:
            at = points.setdefault(_number(record.get(_w('point'))), [])
            if m not in at:
                at.append(m)
    out: dict[int, str] = {}
    said: list[str] = []
    for point, ms in points.items():
        complete = [m for m in ms if all(m in held for held in has.values())]
        stands = max(complete) if complete else max(ms)
        where = '' if point is None else f' of point {_said(point)}'
        for m in ms:
            short = [words for words, held in has.items() if m not in held]
            if m == stands:
                if short:
                    said.append(f'measurement {_said(m)}{where} has no '
                                f'{" and no ".join(short)}; it is kept as it is')
                continue
            if m in complete or m < stands:
                out[m] = 'replaced'
            else:
                out[m] = 'incomplete'
                said.append(f'measurement {_said(m)}{where} is incomplete (it has no '
                            f'{" and no ".join(short)}), so it is left out and measurement '
                            f'{_said(stands)} stands')
    return out, said


def _window_words(record: Mapping[str, Any], block: int | None) -> str:
    from ..core.modal_test import tau_words

    window = record.get(_w('window'))
    if not isinstance(window, Mapping):
        return 'windows not recorded'
    said = []
    for side, words in ((_w('window_reference'), 'the references'),
                        (_w('window_response'), 'the responses')):
        one = window.get(side)
        if not isinstance(one, Mapping):
            continue
        kind = str(one.get(_w('window_kind')) or '')
        if kind == 'force':
            width, fall = one.get(_w('force_width')), one.get(_w('force_fall'))
            part = 'force window'
            if isinstance(width, (int, float)):
                part += f' {width * 100:g} % of the block after the trigger'
            if isinstance(fall, (int, float)):
                part += f', falling over a further {fall * 100:g} % of its width'
            said.append(f'{part} on {words}')
        elif kind:
            said.append(f'{kind} window on {words}')
    exponential = window.get(_w('exponential'))
    if isinstance(exponential, Mapping):
        tau = exponential.get(_w('tau_s'))
        part = 'exponential window on every channel'
        if isinstance(tau, (int, float)) and tau > 0:
            part = f'{tau_words(tau)} (adds a decay rate of {1.0 / tau:g} 1/s)'
        end = exponential.get(_w('end'))
        if isinstance(end, (int, float)):
            part += f', ending at {end * 100:g} %'
        said.append(part)
    return '; '.join(said) or 'no windows'


def _averaging(run: run_folder.RunFolder, records: Sequence[Mapping[str, Any]],
               lines: np.ndarray, lacked: list[str]) -> Any:
    """The averaging the FRFs were estimated with: the block, the averages
    and the shaker's window, or none (a boxcar) for an impact's frames,
    whose force and exponential windows each record's comment says
    (`core.modal_test.tau_words`)."""
    from ..core.averaging import Averaging, normalize_window

    config = _config(run)
    block = _number(config.get(_w('block')))
    if block is None or block < 2 or not records or len(lines) < 2:
        return None
    taus, kinds = set(), set()
    for record in records:
        window = record.get(_w('window')) if isinstance(record.get(_w('window')),
                                                        Mapping) else {}
        exponential = window.get(_w('exponential'))
        tau = exponential.get(_w('tau_s')) if isinstance(exponential, Mapping) else None
        taus.add(float(tau) if isinstance(tau, (int, float)) and tau > 0 else None)
        response = window.get(_w('window_response'))
        kinds.add(str(response.get(_w('window_kind')) or 'boxcar')
                  if isinstance(response, Mapping) else 'boxcar')
    if len(taus) > 1 or len(kinds) > 1:
        lacked.append('the measurements were windowed differently, so the FRFs carry no '
                      'averaging and no one exponential window; each record\'s comment says '
                      'its windows')
        return None
    averages = _number(records[0].get(_w('averages'))) or 1
    shaker = config.get(_w('shaker')) if isinstance(config.get(_w('shaker')), Mapping) else {}
    overlap = shaker.get(_w('overlap'))
    overlap = float(overlap) if isinstance(overlap, (int, float)) and 0 <= overlap < 1 else 0.0
    kind, = kinds
    try:
        window = normalize_window(kind)
    except ValueError:
        return None
    return Averaging(frame_length=block, overlap=overlap, window=window, frames=averages)


def _spectral(run: run_folder.RunFolder, result: run_folder.Result, channels: _Channels,
              lacked: list[str], drop: set[int]) -> tuple[Any, str]:
    """A `modal_frf` or `modal_coherence` result as its analysis object, or
    (None, why not)."""
    from ..core.data import Coherence, Frf, MultipleCoherence
    from ..units import UNKNOWN
    from .run_convert import _abscissa

    records = result.records or []
    if not records:
        return None, 'it holds no finished measurement'
    values = result.arrays.get(_w('values'))
    if values is None:
        return None, f'it holds no array named {_w("values")}'
    lines = values.shape[-1]
    abscissa = _abscissa(result, lines)
    if abscissa is None or len(abscissa) != lines:
        return None, 'its abscissa is not stated, or not as long as its values'
    frf = result.kind == _w('frf')
    multiple = not frf and values.ndim == 3
    held: dict[tuple, tuple[int, int, int | None]] = {}
    for r, record in enumerate(records):
        responses = list(record.get(_w('responses')) or [])
        references = list(record.get(_w('references')) or [])
        why = _unit_refusal(channels, responses + references)
        if why:
            return None, why
        shape = (len(responses),) if multiple else (len(responses), len(references))
        if tuple(values.shape[1:-1]) != shape or not responses or (
                not multiple and not references):
            return None, (f'record {r} names {len(responses)} responses and '
                          f'{len(references)} references and holds values of shape '
                          f'{tuple(values.shape[1:])}')
        if _number(record.get(_w('measurement'))) in drop:
            continue
        for i, response in enumerate(responses):
            for j, reference in enumerate([None] if multiple else references):
                ri, rj = int(response[_w('index')]), None if reference is None else int(
                    reference[_w('index')])
                key = (channels.dof(response), channels.scale[ri][1],
                       None if reference is None else channels.dof(reference),
                       None if rj is None else channels.scale[rj][1])
                # a later measurement of the same pair replaces the earlier
                held.pop(key, None)
                held[key] = (r, i, j if reference is not None else None)
    block = _number(_config(run).get(_w('block')))
    rows, response_dof, reference_dof, dims, units, ref_units, comments = ([] for _ in range(7))
    for (dof_i, _di, dof_j, _dj), (r, i, j) in held.items():
        record = records[r]
        response = record[_w('responses')][i]
        si, di, ui = channels.scale[int(response[_w('index')])]
        curve = values[r, i] if j is None else values[r, i, j]
        if frf:
            reference = record[_w('references')][j]
            sj, dj, uj = channels.scale[int(reference[_w('index')])]
            known = UNKNOWN not in (di, dj)
            rows.append(curve * (si / sj if known else 1.0))
            dims.append(f'{di}/{dj}' if known else UNKNOWN)
            units.append(ui if known else None)
            ref_units.append(uj if known else None)
        else:
            rows.append(curve)
        response_dof.append(dof_i)
        reference_dof.append(dof_j)
        where = (f'measurement {_said(_number(record.get(_w("measurement"))))}'
                 + (f', point {_said(_number(record.get(_w("point"))))}'
                    if _number(record.get(_w('point'))) is not None else ''))
        if frf:
            count = len(record.get(_w('references')) or [])
            comments.append(f'{record.get(_w("estimator")) or "estimator not recorded"} from '
                            f'{count} reference{"s" * (count != 1)}, '
                            f'{record.get(_w("averages"), "?")} averages; {where}; '
                            f'{_window_words(record, block)}')
        else:
            comments.append(f'{"multiple" if multiple else "ordinary"} coherence; {where}')
        if not np.isfinite(rows[-1]).all():
            bad = int(np.count_nonzero(~np.isfinite(rows[-1])))
            lacked.append(f'the {result.name} record of {dof_i}'
                          + (f' against {dof_j}' if dof_j else '')
                          + f' in {where} is not a finite number at {bad} of its '
                          f'{lines} lines')
    if not rows:
        return None, 'every measurement it holds was measured again'
    ordinate = np.asarray(rows)
    if frf:
        out = Frf(abscissa, ordinate, response_dof=response_dof, reference_dof=reference_dof,
                  ordinate_dim=dims, ordinate_unit=units, reference_unit=ref_units,
                  comment=comments)
        averaging = _averaging(run, [records[r] for r, _i, _j in held.values()], abscissa,
                               lacked)
        if averaging is not None:
            out.averaging = averaging
        return out, ''
    clipped = np.clip(np.nan_to_num(ordinate.real, nan=0.0), 0.0, 1.0)
    if multiple:
        return MultipleCoherence(abscissa, clipped, response_dof=response_dof,
                                 comment=comments), ''
    return Coherence(abscissa, clipped, response_dof=response_dof,
                     reference_dof=reference_dof, comment=comments), ''


def _reference_spectra(result: run_folder.Result, channels: _Channels,
                       drop: set[int]) -> tuple[Any, str]:
    from ..core.data import Psd
    from ..units import UNKNOWN
    from .run_convert import _abscissa

    records = result.records or []
    values = result.arrays.get(_w('values'))
    if not records or values is None:
        return None, 'it holds no finished measurement'
    abscissa = _abscissa(result, values.shape[-1])
    if abscissa is None or len(abscissa) != values.shape[-1]:
        return None, 'its abscissa is not stated, or not as long as its values'
    rows, dofs, dims, units, blocks = [], [], [], [], []
    for r, record in enumerate(records):
        references = list(record.get(_w('references')) or [])
        why = _unit_refusal(channels, references)
        if why:
            return None, why
        if values.shape[1:-1] != (len(references),):
            return None, (f'record {r} names {len(references)} references and holds '
                          f'values of shape {tuple(values.shape[1:])}')
        if _number(record.get(_w('measurement'))) in drop:
            continue
        for k, reference in enumerate(references):
            s, d, u = channels.scale[int(reference[_w('index')])]
            rows.append(values[r, k] * (s * s if d != UNKNOWN else 1.0))
            dofs.append(channels.dof(reference))
            dims.append(f'{d}**2/frequency' if d != UNKNOWN else UNKNOWN)
            units.append(u if d != UNKNOWN else None)
            blocks.append(f'measurement {_said(_number(record.get(_w("measurement"))))}')
    if not rows:
        return None, 'every measurement it holds was measured again'
    return Psd(abscissa, np.asarray(rows), response_dof=dofs, ordinate_dim=dims,
               ordinate_unit=units, block=blocks,
               comment=[f'reference autospectrum, {b}' for b in blocks]), ''


def _excitation_roved(channels: _Channels, frf: Any) -> bool:
    """Whether the test roved its excitation, so its FRFs are turned round
    for the fit: from the roving plan when the setup has one (the roving
    channels are reference rows), else from the set's shape (more
    reference DOFs than response DOFs, `core.modal_test.roving_excitation`)."""
    from ..core.modal_test import roving_excitation

    roving = [i for i in channels.roving if i in channels.row]
    if roving:
        return all(channels.roles[channels.row[i]] == 'reference' for i in roving)
    return roving_excitation(frf)


def _frame_lines(run: run_folder.RunFolder) -> list[dict[str, Any]]:
    result = run.results.get(_w('frames'))
    return [dict(r) for r in (result.records or [])] if result is not None else []


def _read_frames(stream: run_folder.Stream, spans: Sequence[tuple[int, int]]) -> np.ndarray:
    """(frames, length, channels) float32, each span as the file holds it."""
    width = len(stream.channels)
    out = []
    with open(stream.path, 'rb') as f:
        for at, length in spans:
            f.seek(at * width * run_folder.SAMPLE_BYTES)
            values = np.fromfile(f, dtype='<f4', count=length * width)
            out.append(values.reshape(length, width))
    return np.asarray(out)


def frames(run: run_folder.RunFolder, table: Any,
           lacked: list[str]) -> list[tuple[str, Any]]:
    """Each measurement's accepted frames as a `TimeHistory`, by name.

    Parameters
    ----------
    run : RunFolder
        A Modal Test run.
    table : ChannelTable
        Its channel table as armed (`run_convert.channel_table_from_rows`).
    lacked : list of str
        What the run lacked, in words; appended to.

    Returns
    -------
    list of (str, TimeHistory)
    """
    from ..core.averaging import Averaging
    from ..core.data import TimeHistory
    from .run_convert import _identity

    channels = _Channels(run, table)
    lines = _frame_lines(run)
    frf = run.results.get(_w('frf'))
    named: dict[int, dict[int, str]] = {}
    for record in (frf.records or []) if frf is not None else []:
        m = _number(record.get(_w('measurement')))
        for entry in [*(record.get(_w('responses')) or []),
                      *(record.get(_w('references')) or [])]:
            if isinstance(entry, Mapping) and isinstance(entry.get(_w('index')), int):
                named.setdefault(m, {})[int(entry[_w('index')])] = channels.dof(entry)
    standing, _said_of = _standing(run)
    shaker = str(_config(run).get(_w('mode')) or '') == 'shaker'
    config_shaker = _config(run).get(_w('shaker'))
    window = (str(config_shaker.get('window') or 'hann') if shaker
              and isinstance(config_shaker, Mapping) else 'hann' if shaker else 'boxcar')
    identity = _identity(run)
    measurements: dict[int, list[dict[str, Any]]] = {}
    for line in lines:
        if line.get(_w('accepted')) is not True or line.get(_w('trial')) is True:
            continue
        m = _number(line.get(_w('measurement')))
        if m is not None:
            measurements.setdefault(m, []).append(line)
    if lines and not measurements:
        lacked.append('no frame of the run was accepted, so it has no measurement frames')
    out = []
    for m in sorted(measurements):
        taken = measurements[m]
        first = taken[0]
        point = _number(first.get(_w('point')))
        shape = (_number(first.get(_w('length'))), _number(first.get(_w('trigger_sample'))) or 0,
                 str(first.get(_w('stream')) or 'inputs'))
        same = [line for line in taken
                if (_number(line.get(_w('length'))), _number(line.get(_w('trigger_sample'))) or 0,
                    str(line.get(_w('stream')) or 'inputs')) == shape]
        if len(same) < len(taken):
            lacked.append(f'measurement {_said(m)} has frames of more than one length or '
                          f'trigger; the {len(taken) - len(same)} unlike its first are left '
                          'in the stream')
        length, pre, name = shape
        stream = run.streams.get(name)
        if stream is None or not length:
            lacked.append(f'the frames of measurement {_said(m)} are in no stream the run '
                          'holds, so it has no measurement frames')
            continue
        spans = [(int(line.get(_w('offset')) or 0), length) for line in same]
        inside = [s for s in spans if s[0] >= 0 and s[0] + length <= stream.frames]
        if len(inside) < len(spans):
            lacked.append(f'{len(spans) - len(inside)} frames of measurement {_said(m)} run '
                          'past the end of the stream and are left out')
        if not inside:
            continue
        held = _read_frames(stream, inside)
        groups = [(stream, held)]
        drives = run.streams.get('drives')
        if drives is not None and name != 'drives' and all(
                s[0] + length <= drives.frames for s in inside):
            groups.append((drives, _read_frames(drives, inside)))
        count = len(inside)
        rows, dofs, dims, units, roles, labels, comments = [], [], [], [], {}, [], []
        lines_of = [same[k] for k, s in enumerate(spans) if s in inside]
        word = 'frame' if shaker else 'hit'
        for source, values in groups:
            scale_dims = [_unit_scale(u) for u in source.units]
            offsets = _stream_offsets([u for _s, _d, u in scale_dims])
            for c, index in enumerate(source.channels):
                if index not in channels.row:
                    raise run_folder.RunFolderError(
                        f'the stream {source.name!r} names channel {index}, which the table '
                        'does not')
                s, d, u = scale_dims[c]
                dof = named.get(m, {}).get(index) or channels.planned(index, point)
                role = ('monitor' if source.kind == 'drives'
                        else channels.roles[channels.row[index]] or 'response')
                roles[(dof, d)] = role
                for k in range(count):
                    rows.append(values[k, :, c].astype(np.float64) * s + offsets[c])
                    dofs.append(dof)
                    dims.append(d)
                    units.append(u)
                    average = _number(lines_of[k].get(_w('average')))
                    labels.append(f'{word} {average if average is not None else k + 1}')
                    comments.append(f'measurement {_said(m)}'
                                    + (f', point {_said(point)}' if point is not None else '')
                                    + {'replaced': ', replaced by a later measurement',
                                       'incomplete': ', incomplete and left out of the '
                                                     'spectra'}.get(standing.get(m), ''))
        rate = stream.rate
        history = TimeHistory((np.arange(length) - pre) / rate, np.asarray(rows),
                              response_dof=dofs, ordinate_dim=dims, ordinate_unit=units,
                              block=labels, comment=comments)
        history.roles = roles
        history.source = run.source
        history.run_identity = identity
        history.averaging = Averaging.for_records(length, window=window)
        title = (f'Measurement {_said(m)} frames'
                 + (f' (point {_said(point)})' if point is not None else '')
                 + (f' ({standing[m]})' if m in standing else ''))
        out.append((title, history))
    return out


def results(run: run_folder.RunFolder, table: Any, lacked: list[str]
            ) -> tuple[list[tuple[str, Any]], set[str], dict[str, dict[str, np.ndarray]]]:
    """The analysis objects of a Modal Test run's results.

    Parameters
    ----------
    run : RunFolder
        A Modal Test run.
    table : ChannelTable
        Its channel table as armed.
    lacked : list of str
        What the run lacked, in words; appended to.

    Returns
    -------
    (list of (str, object), set of str, dict)
        The objects by name, in order; the results they came from (or that
        stay in the run record whole), which the general mapping leaves
        alone; and the arrays of those results the run record keeps.
    """
    from ..core.modal_test import fixed_reference, fixed_reference_refusal

    channels = _Channels(run, table)
    handled = {_w(k) for k in ('frf', 'coherence', 'reference_spectra', 'frames', 'outcome')}
    kept: dict[str, dict[str, np.ndarray]] = {}
    out: list[tuple[str, Any]] = []
    frf_result = run.results.get(_w('frf'))
    records = (frf_result.records or []) if frf_result is not None else []
    standing, said = _standing(run)
    lacked.extend(said)
    drop = set(standing)
    measured = sorted({p for r in records if (p := _number(r.get(_w('point')))) is not None})
    planned = len(channels.points)
    outcome = run.results.get(_w('outcome'))
    if outcome is not None and isinstance(outcome.description.get(_w('points_planned')), int):
        planned = int(outcome.description[_w('points_planned')])
    first = _w('first')
    missing = [p for p in range(first, first + planned) if p not in measured]
    if missing and planned:
        lacked.append(f'{len(missing)} of the {planned} points planned were not measured: '
                      + ', '.join(f'point {_said(p)}' for p in missing))
    if frf_result is None:
        lacked.append('the run finished no measurement, so it has no FRFs')
    frf = None
    for key in ('frf', 'coherence'):
        result = run.results.get(_w(key))
        if result is None:
            continue
        held, why = _spectral(run, result, channels, lacked, drop)
        if held is None:
            lacked.append(f'the result {result.name!r} is kept as arrays in the run record: '
                          f'{why}')
            kept[result.name] = dict(result.arrays)
            continue
        held.source = run.source
        out.append((result.name, held))
        if key == 'frf':
            frf = held
        if frf is None or not _excitation_roved(channels, frf):
            continue
        why = fixed_reference_refusal(held)
        if why:
            if key == 'frf':
                lacked.append(f'the FRFs were measured with the excitation roving and are not '
                              f'turned round for the fit: {why}')
            continue
        out.append((f'{result.name} {FIXED}', fixed_reference(held)))
    spectra = run.results.get(_w('reference_spectra'))
    if spectra is not None:
        held, why = _reference_spectra(spectra, channels, drop)
        if held is None:
            lacked.append(f'the result {spectra.name!r} is kept as arrays in the run record: '
                          f'{why}')
            kept[spectra.name] = dict(spectra.arrays)
        else:
            held.source = run.source
            out.append((spectra.name, held))
            extra = {k: v for k, v in spectra.arrays.items() if k != _w('values')}
            if extra:
                kept[spectra.name] = extra
    for key in ('frames', 'outcome'):
        result = run.results.get(_w(key))
        if result is not None and result.arrays:
            kept[result.name] = dict(result.arrays)
    return out, handled, kept
