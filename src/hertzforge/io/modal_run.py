"""A Modal Test run folder's results as analysis objects.

`run_convert.convert` hands a Modal Test run (`WIRE['test']`) here for
its measurements; everything else about the run (the channel table, the
run record, the object group, the file) is converted as every run's is.
What a Modal Test run becomes:

- **The channel table**: as every run's, with each row the run's setup
  names as a reference (its check's `references`, the hammer or the
  shakers' force gauges) given the analysis role reference
  (`reference_rows`).
- **Measurement frames**: a `TimeHistory` per measurement holding its
  accepted hits or frames, cut from the input stream by each frame
  line's span, a record per channel per frame, each channel at the DOF
  it had in that measurement (a roving channel at the point measured),
  in SI, timed from the trigger (an impact's pre-trigger samples at
  negative times) or from the frame's start. Each record's comment says
  the measurement, the point, the hit and what the run checked of it:
  the peak force, the distance from a force target and its verdict, and
  any warning. Rejected and undone hits are not in it: they stay in the
  stream, and the run record's frame lines say what each was. A shaker's
  drive stream is in the same history, as monitors.
- **modal_frf**: one `Frf` of every measurement's FRFs, each record a
  response against a reference at the DOFs the measurement named, in SI
  from the channel rows' units; a point measured again replaces what the
  earlier measurement gave. Each record's comment says the estimator,
  the averages, the measurement and point, and the windows (the force
  window in samples and the exponential window's time constant, which
  `core.modal_test.frf_tau` reads back after a save). The averaging holds
  the block, the averages and the analysis window.
- **modal_frf fixed reference**: for a test whose excitation roved
  (its roving channels are reference rows; with no roving plan, more
  reference DOFs than response DOFs), the same FRFs turned round
  by reciprocity (`core.modal_test.fixed_reference`), marked so on every
  record, the form a modal fit takes; with its ordinary coherence turned
  the same way.
- **modal_coherence**: the ordinary coherence of each response (one
  reference) as a `Coherence`, or each response's multiple coherence
  (several references) as a `MultipleCoherence`, by the array the
  result holds.
- **modal_reference_spectra**: each measurement's reference autospectra
  as a `Psd`, a record per reference per measurement; with several
  references the condition number of their spectral matrix per line
  stays in the run record as an array.
- `modal_frames` and `modal_outcome` stay in the run record whole: the
  frame lines with every hit's checks, and the points planned and
  measured, the hits or frames taken and accepted and how the test
  ended.

Every name read from the folder is in `WIRE`, the one table to change
when the control service's spelling does; `tests/run_folders.py` writes
its synthetic folders from the same table.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np

from . import run_folder
from .rattlesnake import _stream_offsets, _unit_scale

#: every name this module reads from a Modal Test run folder: the test, its
#: results and their arrays, the members of their record lines, the
#: windows, the frame lines, the setup and the outcome
WIRE: dict[str, Any] = {
    'test': 'modal_test',
    # results, by their name in the folder, and their kinds
    'frf': 'modal_frf',
    'coherence': 'modal_coherence',
    'reference_spectra': 'modal_reference_spectra',
    'frames': 'modal_frames',
    'outcome': 'modal_outcome',
    'frf_kind': 'frf',
    'coherence_kind': 'coherence',
    'reference_spectra_kind': 'reference_spectra',
    'frames_kind': 'modal_frames',
    'outcome_kind': 'modal_outcome',
    # the arrays: one record's, the records first in the folder
    'frf_values': 'frf',
    'ordinary': 'coherence',
    'multiple': 'multiple_coherence',
    'autospectra': 'autospectra',
    'condition': 'condition_number',
    'span': 'span',
    # each result's axes, in its `layout`
    'layout': 'layout',
    'frf_layout': ['response', 'reference', 'line'],
    'coherence_layout': ['response', 'line'],
    'reference_spectra_layout': ['reference', 'line'],
    # a spectral record's line
    'measurement': 'measurement',
    'point': 'point',
    'replaces': 'replaces',
    'estimator': 'estimator',
    'averages': 'averages',
    'rows': 'rows',
    'responses': 'responses',
    'references': 'references',
    # a channel of a record: its table index, node, direction and unit
    'index': 'index',
    'node': 'node',
    'direction': 'direction',
    'unit': 'unit',
    # the windows, in the FRF result's description and the setup's resolved
    'windows': 'windows',
    'analysis': 'analysis',
    'force': 'force',
    'pre_frames': 'pre_frames',
    'width_frames': 'width_frames',
    'fall_frames': 'fall_frames',
    'exponential': 'exponential',
    'tau_s': 'tau_s',
    'end': 'end',
    # a frame line: the event, the tile and the hit's report
    'event': 'event',
    'taken': 'taken',
    'changed': 'status',
    'n': 'n',
    'seq': 'seq',
    'status': 'status',
    'accepted': 'accepted',
    'reason': 'reason',
    'overload': 'overload',
    'double_hit': 'double_hit',
    'peak': 'peak',
    'tip_warning': 'tip_warning',
    'force_window_cut': 'force_window_cut',
    'target_verdict': 'target_verdict',
    'target_diff_pct': 'target_diff_pct',
    'verdicts': {'high': 'high', 'low': 'low', 'on_target': 'on target'},
    # a stream's first station frame, and the frames a cut stream was cut from
    'first_station_frame': 'first_station_frame',
    'cut_from_frames': 'cut_from_frames',
    # the setup: `config` as accepted, `check` as the run was configured
    'config': 'config',
    'check': 'check',
    'mode': 'mode',
    'shaker': 'shaker',
    'impact': 'impact',
    'block': 'block',
    'roving': 'roving',
    'roving_channels': 'channels',
    'points': 'points',
    'trigger': 'trigger',
    'pretrigger_frames': 'pretrigger_frames',
    'shaker_window': 'window',
    'overlap_pct': 'overlap_pct',
    # the outcome (`modal_outcome` and `end.json`)
    'totals': 'totals',
    'points_planned': 'points_planned',
    'points_measured': 'points_measured',
    'taken_total': 'taken',
    'accepted_total': 'accepted',
    # the folder numbers points from this, and measurements from this
    'first_point': 0,
    'first_measurement': 1,
}

#: the name of the FRFs turned round by reciprocity, after the measured
#: result's own
FIXED = 'fixed reference'


def _w(key: str) -> Any:
    return WIRE[key]


def _mapping(value: Any) -> Mapping[str, Any]:
    return value if isinstance(value, Mapping) else {}


def _config(run: run_folder.RunFolder) -> Mapping[str, Any]:
    return _mapping(_mapping(run.setup).get(_w('config')))


def _check(run: run_folder.RunFolder) -> Mapping[str, Any]:
    return _mapping(_mapping(run.setup).get(_w('check')))


def _number(m: Any) -> int | None:
    return int(m) if isinstance(m, int) and not isinstance(m, bool) else None


def _real(x: Any) -> float | None:
    return float(x) if isinstance(x, (int, float)) and not isinstance(x, bool) else None


def _point_said(p: int | None) -> str:
    return '?' if p is None else str(p - _w('first_point') + 1)


def _measurement_said(m: int | None) -> str:
    return '?' if m is None else str(m - _w('first_measurement') + 1)


def _block(run: run_folder.RunFolder) -> int | None:
    return _number(_check(run).get(_w('block'))) or _number(_config(run).get(_w('block')))


def _shaker(run: run_folder.RunFolder) -> bool:
    mode = _check(run).get(_w('mode')) or _config(run).get(_w('mode'))
    return str(mode or '') == 'shaker'


def _rows(record: Mapping[str, Any], side: str) -> list[Mapping[str, Any]]:
    return [e for e in _mapping(record.get(_w('rows'))).get(_w(side)) or []
            if isinstance(e, Mapping)]


def reference_rows(run: run_folder.RunFolder) -> list[int]:
    """The table indexes of the rows a Modal Test run's setup names as its
    references (the setup's check: the hammer, or each shaker's force
    gauge).

    The control service's table holds a force gauge as an input row whose
    analysis role is reference; the general mapping of a control table
    (`run_convert.channel_table_from_rows`) reads an input row as a
    response, so the converter gives these rows the role reference.

    Parameters
    ----------
    run : RunFolder
        A Modal Test run.

    Returns
    -------
    list of int
    """
    return [i for e in _check(run).get(_w('references')) or []
            if isinstance(e, Mapping) and (i := _number(e.get(_w('index')))) is not None]


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
        roving = _mapping(_config(run).get(_w('roving')))
        self.roving: list[int] = [int(i) for i in roving.get(_w('roving_channels')) or []
                                  if isinstance(i, int)]
        self.points: list[Mapping[str, Any]] = [
            p for p in _check(run).get(_w('points')) or [] if isinstance(p, Mapping)]

    def dof(self, entry: Mapping[str, Any]) -> str:
        """A record's channel as a DOF string, as `ChannelTable.dof_strings`
        spells one: its node and direction joined, the table's own where
        the record names neither.

        Raises
        ------
        ValueError
            Naming the channel, for a node and direction that spell no
            DOF (``'5-Z'``): an analysis object would refuse it later,
            with nothing to say which record wrote it.
        """
        from ..core.validate import dofs

        node, direction = entry.get(_w('node')), entry.get(_w('direction'))
        index = int(entry[_w('index')])
        if node is None and not direction:
            return self.dofs[self.row[index]]
        text = f'{node if node is not None else ""}{direction or ""}' or self.dofs[
            self.row[index]]
        return dofs([text], f'the DOF of channel {index}, node {node!r} and direction '
                            f'{direction!r},')[0]

    def planned(self, index: int, point: int | None) -> str:
        """Channel `index`'s DOF in a measurement of `point`, from the
        setup's check, which names every point's DOFs; the table's where
        the point does not name the channel."""
        table = self.dofs[self.row[index]]
        k = None if point is None else point - _w('first_point')
        if k is None or not 0 <= k < len(self.points):
            return table
        for side in ('references', 'responses'):
            for entry in self.points[k].get(_w(side)) or []:
                if isinstance(entry, Mapping) and _number(entry.get(_w('index'))) == index:
                    try:
                        return self.dof(entry)
                    except ValueError:
                        return table
        return table


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


def _layout_refusal(result: run_folder.Result, key: str) -> str:
    layout = result.description.get(_w('layout'))
    if layout is not None and list(layout) != list(_w(key)):
        return f'its values are laid out {list(layout)}, not {list(_w(key))}'
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
    measurement, its own of those too. At each point the latest complete
    measurement stands and replaces the earlier ones (the record's
    `replaces` names the one it replaces); a later incomplete one (its
    coherence or spectra never written, as a run cut off leaves it)
    replaces nothing and is left out. A point with no complete measurement
    keeps its latest. Hits alone (rejected, or a set never finished) are
    no measurement and replace nothing.
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
        where = '' if point is None else f' of point {_point_said(point)}'
        for m in ms:
            short = [words for words, held in has.items() if m not in held]
            if m == stands:
                if short:
                    said.append(f'measurement {_measurement_said(m)}{where} has no '
                                f'{" and no ".join(short)}; it is kept as it is')
                continue
            if m in complete or m < stands:
                out[m] = 'replaced'
            else:
                out[m] = 'incomplete'
                said.append(f'measurement {_measurement_said(m)}{where} is incomplete (it has '
                            f'no {" and no ".join(short)}), so it is left out and measurement '
                            f'{_measurement_said(stands)} stands')
    return out, said


def _windows(result: run_folder.Result) -> Mapping[str, Any]:
    return _mapping(result.description.get(_w('windows')))


def _window_words(windows: Mapping[str, Any]) -> str:
    from ..core.modal_test import tau_words

    if not windows:
        return 'windows not recorded'
    said = []
    analysis = windows.get(_w('analysis'))
    if analysis:
        said.append(f'{analysis} analysis window')
    force = _mapping(windows.get(_w('force')))
    pre, width, fall = (_number(force.get(_w(k)))
                        for k in ('pre_frames', 'width_frames', 'fall_frames'))
    if width is not None:
        said.append(f'force window on the references, {width} samples after the trigger'
                    + (f' at sample {pre}' if pre is not None else '')
                    + (f' and a fall of {fall}' if fall is not None else ''))
    exponential = _mapping(windows.get(_w('exponential')))
    tau = _real(exponential.get(_w('tau_s')))
    if tau is not None and tau > 0:
        part = f'{tau_words(tau)} (adds a decay rate of {1.0 / tau:g} 1/s)'
        end = _real(exponential.get(_w('end')))
        if end is not None:
            part += f', ending at {end * 100:g} %'
        said.append(part)
    return '; '.join(said)


def _averaging(run: run_folder.RunFolder, windows: Mapping[str, Any],
               records: Sequence[Mapping[str, Any]], lines: np.ndarray) -> Any:
    """The averaging the FRFs were estimated with: the block, the averages
    and the analysis window (uniform is a boxcar), with a shaker's overlap;
    an impact's force and exponential windows are in each record's
    comment (`core.modal_test.tau_words`)."""
    from ..core.averaging import Averaging, normalize_window

    block = _block(run)
    if block is None or block < 2 or not records or len(lines) < 2:
        return None
    averages = {_number(r.get(_w('averages'))) for r in records}
    shaker = _mapping(_check(run).get(_w('shaker')))
    overlap = _real(shaker.get(_w('overlap_pct')))
    overlap = overlap / 100.0 if overlap is not None and 0 <= overlap < 100 else 0.0
    try:
        window = normalize_window(str(windows.get(_w('analysis')) or 'boxcar'))
    except ValueError:
        return None
    return Averaging(frame_length=block, overlap=overlap, window=window,
                     frames=averages.pop() if len(averages) == 1 and None not in averages
                     else None)


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
    frf = result.kind == _w('frf_kind')
    if frf:
        names = [_w('frf_values')]
    else:
        names = [n for n in (_w('ordinary'), _w('multiple')) if n in result.arrays]
        if len(names) > 1:
            return None, 'it holds both an ordinary and a multiple coherence'
    if not names or names[0] not in result.arrays:
        return None, (f'it holds no array named {_w("frf_values")}' if frf else
                      f'it holds no array named {_w("ordinary")} or {_w("multiple")}')
    values = result.arrays[names[0]]
    multiple = not frf and names[0] == _w('multiple')
    why = _layout_refusal(result, 'frf_layout' if frf else 'coherence_layout')
    if why:
        return None, why
    lines = values.shape[-1]
    abscissa = _abscissa(result, lines)
    if abscissa is None or len(abscissa) != lines:
        return None, 'its abscissa is not stated, or not as long as its values'
    held: dict[tuple, tuple[int, int, int | None]] = {}
    for r, record in enumerate(records):
        responses, references = _rows(record, 'responses'), _rows(record, 'references')
        why = _unit_refusal(channels, responses + references)
        if why:
            return None, why
        shape = (len(responses), len(references)) if frf else (len(responses),)
        if tuple(values.shape[1:-1]) != shape or not responses or not references:
            return None, (f'record {r} names {len(responses)} responses and '
                          f'{len(references)} references and holds values of shape '
                          f'{tuple(values.shape[1:])}')
        if not frf and not multiple and len(references) != 1:
            return None, (f'record {r} names {len(references)} references and holds an '
                          'ordinary coherence, which is of one reference')
        if _number(record.get(_w('measurement'))) in drop:
            continue
        for i, response in enumerate(responses):
            for j, reference in enumerate(references if frf else [references[0]]):
                ri, rj = int(response[_w('index')]), int(reference[_w('index')])
                try:
                    key = (channels.dof(response), channels.scale[ri][1],
                           None if multiple else channels.dof(reference),
                           None if multiple else channels.scale[rj][1])
                except ValueError as error:
                    return None, f'in record {r}, {error}'
                # a later measurement of the same pair replaces the earlier
                held.pop(key, None)
                held[key] = (r, i, j if frf else None)
    words = _window_words(_windows(run.results[_w('frf')]) if _w('frf') in run.results
                          else {})
    rows, response_dof, reference_dof, dims, units, ref_units, comments = ([] for _ in range(7))
    for (dof_i, _di, dof_j, _dj), (r, i, j) in held.items():
        record = records[r]
        response = _rows(record, 'responses')[i]
        si, di, ui = channels.scale[int(response[_w('index')])]
        curve = values[r, i] if j is None else values[r, i, j]
        if frf:
            reference = _rows(record, 'references')[j]
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
        point = _number(record.get(_w('point')))
        where = (f'measurement {_measurement_said(_number(record.get(_w("measurement"))))}'
                 + (f', point {_point_said(point)}' if point is not None else ''))
        if frf:
            count = len(_rows(record, 'references'))
            comments.append(f'{record.get(_w("estimator")) or "estimator not recorded"} from '
                            f'{count} reference{"s" * (count != 1)}, '
                            f'{record.get(_w("averages"), "?")} averages; {where}'
                            + (f'; {words}' if words else ''))
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
        averaging = _averaging(run, _windows(result),
                               [records[r] for r, _i, _j in held.values()], abscissa)
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
    values = result.arrays.get(_w('autospectra'))
    if not records:
        return None, 'it holds no finished measurement'
    if values is None:
        return None, f'it holds no array named {_w("autospectra")}'
    why = _layout_refusal(result, 'reference_spectra_layout')
    if why:
        return None, why
    abscissa = _abscissa(result, values.shape[-1])
    if abscissa is None or len(abscissa) != values.shape[-1]:
        return None, 'its abscissa is not stated, or not as long as its values'
    rows, dofs, dims, units, blocks = [], [], [], [], []
    for r, record in enumerate(records):
        references = _rows(record, 'references')
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
            try:
                dofs.append(channels.dof(reference))
            except ValueError as error:
                return None, f'in record {r}, {error}'
            rows.append(values[r, k] * (s * s if d != UNKNOWN else 1.0))
            dims.append(f'{d}**2/frequency' if d != UNKNOWN else UNKNOWN)
            units.append(u if d != UNKNOWN else None)
            blocks.append('measurement '
                          f'{_measurement_said(_number(record.get(_w("measurement"))))}')
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


def _hits(run: run_folder.RunFolder) -> list[dict[str, Any]]:
    """Every hit or frame taken, in the order taken: its `taken` line with
    the status its last line gave it and its span, [first station frame,
    length]."""
    result = run.results.get(_w('frames'))
    if result is None:
        return []
    spans = result.arrays.get(_w('span'))
    by_seq: dict[Any, dict[str, Any]] = {}
    for k, line in enumerate(result.records or []):
        seq = line.get(_w('seq'))
        if line.get(_w('event')) == _w('taken') or seq not in by_seq:
            span = spans[k] if spans is not None and k < len(spans) else None
            by_seq[seq] = {**line, _w('span'): span}
        else:
            by_seq[seq][_w('status')] = line.get(_w('status'))
            by_seq[seq][_w('reason')] = line.get(_w('reason'))
    return list(by_seq.values())


def _measurement_hits(run: run_folder.RunFolder, lacked: list[str]
                      ) -> dict[int, tuple[int | None, list[dict[str, Any]]]]:
    """Each finished measurement's accepted hits, {measurement: (point,
    hits)}: the frame lines name no measurement, so each point's accepted
    hits, in the order taken, go to its measurements in order, as many to
    each as it averaged."""
    frf = run.results.get(_w('frf'))
    accepted: dict[Any, list[dict[str, Any]]] = {}
    for hit in _hits(run):
        if hit.get(_w('status')) == _w('accepted'):
            accepted.setdefault(_number(hit.get(_w('point'))), []).append(hit)
    out: dict[int, tuple[int | None, list[dict[str, Any]]]] = {}
    for record in (frf.records or []) if frf is not None else []:
        m, point = _number(record.get(_w('measurement'))), _number(record.get(_w('point')))
        averages = _number(record.get(_w('averages'))) or 0
        if m is None or m in out:
            continue
        held = accepted.get(point, [])
        out[m] = (point, held[:averages])
        accepted[point] = held[averages:]
    for point, rest in accepted.items():
        if rest:
            word = 'frame' if _shaker(run) else 'hit'
            lacked.append(f'{len(rest)} accepted {word}{"s" * (len(rest) != 1)} of point '
                          f'{_point_said(point)} belong to no finished measurement; they stay '
                          'in the stream and the frame lines')
    return out


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


def _hit_words(hit: Mapping[str, Any], unit: str) -> str:
    """What the run checked of a hit, in words: its peak force, its distance
    from the force target and the verdict, and its warnings."""
    said = []
    peak = _real(hit.get(_w('peak')))
    if peak is not None:
        said.append(f'peak {peak:.6g} {unit}'.rstrip())
    diff = _real(hit.get(_w('target_diff_pct')))
    verdict = hit.get(_w('target_verdict'))
    if diff is not None or verdict:
        part = f'{diff:+.4g} % from the force target' if diff is not None else 'force target'
        said.append(part + (f' ({_w("verdicts").get(verdict, verdict)})' if verdict else ''))
    for key, words in (('tip_warning', 'tip warning'),
                       ('force_window_cut', 'the force window cuts the pulse'),
                       ('overload', 'overload'), ('double_hit', 'double hit')):
        if hit.get(_w(key)) is True:
            said.append(words)
    return ', '.join(said)


def frames(run: run_folder.RunFolder, table: Any,
           lacked: list[str]) -> list[tuple[str, Any]]:
    """Each measurement's accepted hits or frames as a `TimeHistory`, by
    name.

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
    from ..core.averaging import Averaging, normalize_window
    from ..core.data import TimeHistory
    from .run_convert import _identity

    channels = _Channels(run, table)
    frf = run.results.get(_w('frf'))
    named: dict[int, dict[int, str]] = {}
    for record in (frf.records or []) if frf is not None else []:
        m = _number(record.get(_w('measurement')))
        for entry in [*_rows(record, 'responses'), *_rows(record, 'references')]:
            if isinstance(entry.get(_w('index')), int):
                try:
                    named.setdefault(m, {})[int(entry[_w('index')])] = channels.dof(entry)
                except ValueError:
                    pass  # the FRF's own refusal names it; the frame keeps the plan's
    standing, _said_of = _standing(run)
    shaker = _shaker(run)
    try:
        window = (normalize_window(str(_mapping(_check(run).get(_w('shaker'))).get(
            _w('shaker_window')) or 'hann')) if shaker else 'boxcar')
    except ValueError:
        window = 'boxcar'
    pre = 0 if shaker else (_number(_mapping(_mapping(_check(run).get(_w('impact'))).get(
        _w('trigger'))).get(_w('pretrigger_frames'))) or 0)
    word = 'frame' if shaker else 'hit'
    identity = _identity(run)
    measurements = _measurement_hits(run, lacked)
    if _hits(run) and not any(hits for _p, hits in measurements.values()):
        lacked.append(f'no {word} of the run is in a finished measurement, so it has no '
                      'measurement frames')
    stream = run.streams.get('inputs')
    reference_unit = {i: channels.unit.get(i, '') for i in reference_rows(run)}
    unit = next(iter(reference_unit.values()), '')
    cut: dict[str, int] = {}
    past = 0
    out = []
    for m in sorted(measurements):
        point, hits = measurements[m]
        if not hits:
            continue
        spans = []
        for hit in hits:
            span = hit.get(_w('span'))
            spans.append(None if span is None or len(span) != 2 else (int(span[0]),
                                                                      int(span[1])))
        lengths = {s[1] for s in spans if s is not None}
        if stream is None or len(lengths) != 1:
            lacked.append(f'the {word}s of measurement {_measurement_said(m)} are in no '
                          'stream the run holds, or are of more than one length, so it has '
                          'no measurement frames')
            continue
        length = lengths.pop()
        first = _number(stream.description.get(_w('first_station_frame'))) or 0
        inside, kept = [], []
        for hit, span in zip(hits, spans, strict=True):
            at = None if span is None else span[0] - first
            if at is not None and at >= 0 and at + length <= stream.frames:
                inside.append((at, length))
                kept.append(hit)
            else:
                whole = _number(stream.description.get(_w('cut_from_frames')))
                if at is not None and whole is not None and at + length <= whole:
                    cut[stream.name] = cut.get(stream.name, 0) + 1
                else:
                    past += 1
        if not inside:
            continue
        groups = [(stream, _read_frames(stream, inside))]
        drives = run.streams.get('drives')
        if drives is not None and all(s[0] + length <= drives.frames for s in inside):
            groups.append((drives, _read_frames(drives, inside)))
        rows, dofs, dims, units, roles, labels, comments = [], [], [], [], {}, [], []
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
                for k, hit in enumerate(kept):
                    rows.append(values[k, :, c].astype(np.float64) * s + offsets[c])
                    dofs.append(dof)
                    dims.append(d)
                    units.append(u)
                    n = _number(hit.get(_w('n')))
                    labels.append(f'{word} {n if n is not None else k + 1}')
                    checked = _hit_words(hit, unit)
                    comments.append(f'measurement {_measurement_said(m)}'
                                    + (f', point {_point_said(point)}'
                                       if point is not None else '')
                                    + (f', {word} {n}' if n is not None else '')
                                    + (f'; {checked}' if checked else '')
                                    + {'replaced': '; replaced by a later measurement',
                                       'incomplete': '; incomplete and left out of the '
                                                     'spectra'}.get(standing.get(m), ''))
        history = TimeHistory((np.arange(length) - pre) / stream.rate, np.asarray(rows),
                              response_dof=dofs, ordinate_dim=dims, ordinate_unit=units,
                              block=labels, comment=comments)
        history.roles = roles
        history.source = run.source
        history.run_identity = identity
        history.averaging = Averaging.for_records(length, window=window)
        title = (f'Measurement {_measurement_said(m)} frames'
                 + (f' (point {_point_said(point)})' if point is not None else '')
                 + (f' ({standing[m]})' if m in standing else ''))
        out.append((title, history))
    for name, count in cut.items():
        held = run.streams[name]
        lacked.append(f'the stream {name!r} holds its first {held.frames} of '
                      f'{_number(held.description.get(_w("cut_from_frames")))} frames, so '
                      f'{count} {word}{"s" * (count != 1)} past them '
                      f'{"has" if count == 1 else "have"} no frames')
    if past:
        lacked.append(f'{past} {word}{"s" * (past != 1)} '
                      f'{"runs" if past == 1 else "run"} past the end of the stream and '
                      f'{"is" if past == 1 else "are"} left out')
    return out


def _totals(run: run_folder.RunFolder) -> Mapping[str, Any]:
    outcome = run.results.get(_w('outcome'))
    totals = _mapping(outcome.description.get(_w('totals'))) if outcome is not None else {}
    return totals or _mapping(_mapping(run.end).get(_w('totals')))


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
    totals = _totals(run)
    planned = _number(totals.get(_w('points_planned')))
    planned = len(channels.points) if planned is None else planned
    first = _w('first_point')
    missing = [p for p in range(first, first + planned) if p not in measured]
    if missing and planned:
        lacked.append(f'{len(missing)} of the {planned} points planned were not measured: '
                      + ', '.join(f'point {_point_said(p)}' for p in missing))
    accepted = _number(totals.get(_w('accepted_total')))
    lines = sum(1 for hit in _hits(run) if hit.get(_w('status')) == _w('accepted'))
    if accepted is not None and accepted != lines:
        lacked.append(f'the run counts {accepted} accepted and its frame lines hold {lines}')
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
            extra = {k: v for k, v in spectra.arrays.items() if k != _w('autospectra')}
            if extra:
                kept[spectra.name] = extra
    for key in ('frames', 'outcome'):
        result = run.results.get(_w(key))
        if result is not None and result.arrays:
            kept[result.name] = dict(result.arrays)
    return out, handled, kept
