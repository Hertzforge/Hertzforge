"""Operations on a modal survey from the controller: its FRFs, coherence
and time captures as waterfalls, the modes fitted to its FRFs on the
survey geometry, the operating deflection shape at one frequency, the
MAC of the fitted modes against each other or the survey's FE model, the
FRFs the fitted modes synthesize against the measured ones, and a time
capture played back on the survey geometry.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .datasets import ModalRun, _mesh
from .descriptors import Operation, Option, Param
from .protocol import refused
from .waterfalls import COMPONENTS, QUANTITY, unit_text, waterfall_view

#: what a survey can be drawn as a waterfall of, by the name its project
#: gives each
WATERFALL_OF = {'frf': 'FRF', 'coherence': 'Multiple Coherence',
                'time': 'Time History'}
OF = Param('of', 'Records', 'enum', default='frf',
           options=(Option('frf', 'FRFs'), Option('coherence', 'Multiple coherence'),
                    Option('time', 'Time captures')))
COMPONENT = Param('component', 'Component', 'enum', default='magnitude',
                  options=tuple(Option(key, label) for key, label in COMPONENTS.items()))
PAGE = Param('page', 'Page', 'integer', default=1, min=1, step=1)
LOW = Param('low_hz', 'From', 'number', default=300.0, unit='Hz', min=0.0, step=10.0)
HIGH = Param('high_hz', 'To', 'number', default=1300.0, unit='Hz', min=0.0, step=10.0)
COUNT = Param('count', 'Modes, at most', 'integer', default=5, min=1, max=30, step=1)
AT = Param('frequency_hz', 'Frequency', 'number', default=440.0, unit='Hz', min=0.0,
           step=2.0)
REFERENCE = Param('reference', 'Reference', 'integer', default=1, min=1, step=1)
CAPTURE = Param('capture', 'Capture', 'integer', default=1, min=1, step=1)
RECORD = Param('record', 'FRF record', 'integer', default=1, min=1, step=1)
AGAINST = Param('against', 'Against', 'enum', default='model',
                options=(Option('model', 'FE model'),
                         Option('self', 'Each other (auto-MAC)')))


def _translations(geometry: Any, dofs: Any, values: np.ndarray,
                  rows: np.ndarray) -> np.ndarray:
    """One value per DOF as a translation per drawn node, [N, 3], complex
    when the values are. Nodes with no measured DOF stay where they are."""
    from hertzforge.deform import dof_directions

    dof_rows, dof_vectors, dof_used = dof_directions(geometry, dofs)
    values = np.asarray(values)[dof_used]

    def spread(part: np.ndarray) -> np.ndarray:
        return np.stack([np.bincount(dof_rows, dof_vectors[:, axis] * part,
                                     minlength=geometry.num_nodes)
                         for axis in range(3)], axis=1)[rows]

    if np.iscomplexobj(values):
        return spread(values.real) + 1j * spread(values.imag)
    return spread(values)


def waterfall(run: ModalRun, params: dict[str, Any]) -> list[dict]:
    """`modal_run.waterfall`: the survey's FRFs, coherence or time captures
    with every record at its own depth, colored by level
    (`waterfalls.waterfall_view`). An FRF can be drawn as its magnitude,
    real or imaginary part, or phase; the choice does nothing for real
    data. An object the survey does not have is refused."""
    name = WATERFALL_OF[params['of']]
    if name not in run.project:
        raise refused(f'{run.title} has no {name}.')
    return [waterfall_view(f'{run.title}: {name}', run.project[name],
                           params['page'], params['component'],
                           params['quantity'])]


def fitted(run: ModalRun, low: float, high: float, count: int) -> Any:
    """The shape set `Project.fit_modes` fits to the survey's FRFs within
    `low` to `high` Hz, at most `count` modes, fitted once per band and
    count. The loop is the fitting screen's: confirm the suggestion, take
    the next, until `count`; so the count is the person's stopping
    point, which eleven channels need, since left uncapped the loop takes
    modes out of the noise floor."""
    key = (float(low), float(high), int(count))
    if key not in run.fits:
        try:
            name = run.project.fit_modes('FRF', bounds=(low, high), limit=count,
                                         name='Identified Modes')
        # a band that holds none of the FRF lines, said in the core's words
        except ValueError as error:
            raise refused(str(error)) from None
        run.fits[key] = run.project[name]
        del run.project[name]
    return run.fits[key]


def modes(run: ModalRun, params: dict[str, Any]) -> list[dict]:
    """`modal_run.modes`: the modes fitted to the survey's FRFs within a
    band, each as a translation per node of the survey geometry, as one
    `shape3d` view. Nodes with no measured DOF stay where they are. A band
    that is empty, backwards or outside the FRFs, a survey with no
    geometry or FRFs are refused. A complex shape is sent with its
    imaginary part."""
    low, high = float(params['low_hz']), float(params['high_hz'])
    if not high > low:
        raise refused(f'the band {low:g} to {high:g} Hz is empty; To must be '
                      'above From')
    for needed in ('FRF', 'Geometry'):
        if needed not in run.project:
            raise refused(f'{run.title} has no {needed} to fit modes on.')
    geometry = run.project['Geometry']
    shapes = fitted(run, low, high, params['count'])
    if not shapes.num_shapes:
        raise refused(f'no mode found between {low:g} and {high:g} Hz')
    rows, triangles, lines = _mesh(geometry)
    out = [v.mode(k + 1, float(shapes.frequency[k]), float(shapes.damping[k]),
                  _translations(geometry, shapes.coordinate,
                                shapes.shape_matrix[k], rows))
           for k in range(shapes.num_shapes)]
    xyz = np.asarray(geometry.node_xyz, dtype=float)[rows]
    extra: dict[str, Any] = {'normalization': 'unscaled'
                             if getattr(shapes, 'unscaled', False) else 'mass'}
    if shapes.num_shapes < params['count']:
        extra['note'] = (f'{shapes.num_shapes} of the {params["count"]} modes '
                         f'asked for were found between {low:g} and {high:g} Hz')
    return [v.shape3d(f'{run.title}: {shapes.num_shapes} Identified Modes',
                      'm' if geometry.units_defined else 'undefined', xyz, out,
                      triangles=triangles, lines=lines, **extra)]


def ods(run: ModalRun, params: dict[str, Any]) -> list[dict]:
    """`modal_run.ods`: the operating deflection shape of the survey's FRFs
    to one reference at the line nearest `frequency_hz`, as one complex
    `shape3d` mode, scaled and phase-rotated as Visual Dynamics draws it
    (`deform.OdsDeflection`: the largest record is 1 and phase zero is the
    fullest deflection). A reference past the survey's, a frequency past
    its lines, and a survey with no geometry or FRFs are refused."""
    from hertzforge.deform import OdsDeflection

    for needed in ('FRF', 'Geometry'):
        if needed not in run.project:
            raise refused(f'{run.title} has no {needed} to draw a shape on.')
    frf, geometry = run.project['FRF'], run.project['Geometry']
    references = list(dict.fromkeys(frf.reference_dof))
    number = params['reference']
    if number > len(references):
        raise refused(f'{run.title} has {len(references)} references, '
                      f'{", ".join(references)}; there is no reference {number}')
    reference = references[number - 1]
    lines = np.asarray(frf.abscissa, dtype=float)
    at = float(params['frequency_hz'])
    if not lines[0] <= at <= lines[-1]:
        raise refused(f'{at:g} Hz is outside the FRFs, which span '
                      f'{lines[0]:g} to {lines[-1]:g} Hz')
    line = int(np.argmin(np.abs(lines - at)))
    chosen = [k for k, dof in enumerate(frf.reference_dof) if dof == reference]
    shape = OdsDeflection(geometry, [frf.response_dof[k] for k in chosen],
                          np.asarray(frf.ordinate)[chosen])
    shape.line = line
    # offsets(theta) is Re(pattern * exp(i*theta)): its real part at 0 and
    # minus its imaginary part a quarter cycle on
    pattern = shape.offsets(0.0).copy() - 1j * shape.offsets(np.pi / 2)
    full = np.zeros((geometry.num_nodes, 3), dtype=complex)
    full[shape.rows] = pattern
    rows, triangles, lines_drawn = _mesh(geometry)
    return [v.shape3d(f'{run.title}: Operating Shape at {lines[line]:g} Hz, '
                      f'Reference {reference}',
                      'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[rows],
                      [v.mode(1, float(lines[line]), None, full[rows],
                              label='Operating shape')],
                      triangles=triangles, lines=lines_drawn)]


def _hz(frequency: float) -> str:
    return f'{frequency:.1f} Hz'


def mac(run: ModalRun, params: dict[str, Any]) -> list[dict]:
    """`modal_run.mac`: the Modal Assurance Criterion of the modes fitted
    within a band (the same fit as `modal_run.modes`) against each other,
    or against the survey's FE model on the DOFs both have
    (`core.shapes.cross_mac`). The model's columns are its elastic modes
    from two thirds of the band's bottom to half again its top, so a mode
    the fit placed a little off its model partner still has one. A survey
    with no FE model is refused when asked for one.

    With the survey geometry, the matrix comes with a `shape3d` view of the
    rows' modes and the columns' as `overlays`, both as translations of
    the survey nodes, and the matrix `selects` it: clicking a cell shows
    that row's mode with that column's overlaid (`_pairs`)."""
    from hertzforge.core.shapes import cross_mac

    low, high = float(params['low_hz']), float(params['high_hz'])
    if not high > low:
        raise refused(f'the band {low:g} to {high:g} Hz is empty; To must be '
                      'above From')
    if 'FRF' not in run.project:
        raise refused(f'{run.title} has no FRF to fit modes on.')
    shapes = fitted(run, low, high, params['count'])
    if not shapes.num_shapes:
        raise refused(f'no mode found between {low:g} and {high:g} Hz')
    identified = {'label': 'Identified', 'ticks': [_hz(f) for f in shapes.frequency]}
    rows = range(shapes.num_shapes)
    if params['against'] == 'self':
        title = f'{run.title}: Auto-MAC of {shapes.num_shapes} Identified Modes'
        return _paired(run, v.matrix(title, identified, identified,
                                     shapes.auto_mac(), 'MAC', (0.0, 1.0)),
                       shapes, rows, shapes, rows)
    model = run.analysis_model()
    if model is None:
        raise refused(f'{run.title} has no FE model to compare against.')
    frequency = np.asarray(model.shapes.frequency, dtype=float)
    # a mode that strains no element is given exactly 0 Hz by the solver
    keep = np.flatnonzero((frequency > 0.0) & (frequency >= low / 1.5)
                          & (frequency <= high * 1.5))
    if not keep.size:
        raise refused(f'{model.title} has no elastic mode between '
                      f'{low / 1.5:g} and {high * 1.5:g} Hz')
    values = cross_mac(shapes, model.shapes)[:, keep]
    return _paired(run, v.matrix(f'{run.title}: MAC Against {model.title}', identified,
                                 {'label': 'FE model', 'ticks': [_hz(f) for f in frequency[keep]]},
                                 values, 'MAC', (0.0, 1.0)),
                   shapes, rows, model.shapes, keep)


def _paired(run: ModalRun, matrix: dict, rows: Any, row_modes: Any,
            columns: Any, column_modes: Any) -> list[dict]:
    """`paired` on the survey's geometry, when it has one."""
    return paired(run.project.get('Geometry'), matrix, rows, row_modes, columns,
                  column_modes)


def paired(geometry: Any, matrix: dict, rows: Any, row_modes: Any,
           columns: Any, column_modes: Any) -> list[dict]:
    """The MAC `matrix` and, with a `geometry`, the `shape3d`
    view it `selects`: the row set's modes as `modes` and the column set's
    as `overlays`, each a translation per survey node, with each pair's
    `overlay_phase`, the angle that aligns the column mode with the row
    mode (`core.shapes.alignment_factor`: a real mode is defined only up to
    its sign, a complex one up to a rotation), as translations of the geometry's nodes. Without a geometry the
    matrix comes alone."""
    from hertzforge.core.shapes import alignment_factor

    if geometry is None:
        return [matrix]
    drawn, triangles, lines = _mesh(geometry)

    def on_survey(shapes: Any, k: int) -> np.ndarray:
        return _translations(geometry, shapes.coordinate, shapes.shape_matrix[k], drawn)

    modes = [v.mode(r + 1, float(rows.frequency[r]), float(rows.damping[r]),
                    on_survey(rows, r)) for r in row_modes]
    named = matrix['columns']['label']
    overlays = [v.mode(int(c) + 1, float(columns.frequency[c]),
                       float(columns.damping[c]) if columns.damping[c] > 0 else None,
                       on_survey(columns, int(c)), label=f'{named} mode {int(c) + 1}')
                for c in column_modes]
    phase = np.array([[np.angle(alignment_factor(rows, r, columns, int(c)))
                       for c in column_modes] for r in row_modes], dtype=np.float32)
    matrix['selects'] = 'shape3d'
    shape = v.shape3d(matrix['title'], 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn], modes,
                      triangles=triangles, lines=lines, overlays=overlays,
                      overlay_phase=phase)
    return [matrix, shape]


def motion(run: ModalRun, params: dict[str, Any]) -> list[dict]:
    """`modal_run.motion`: one of the survey's time captures played back on
    the survey geometry, as a `shape3d` view with no modes and a `motion`,
    and the records it plays as a time plot under it. The two share the
    `link` `time`, so the plot marks the sample the geometry shows and
    dragging the mark moves playback there.
    The records follow Visual Dynamics' animation rules
    (`deform.quantity_records`, `deform.animation_records`): one quantity,
    the one most records measure, and one record per DOF, the first; the
    rest are named in the view's note. Each DOF is sent as its samples
    with the drawn node it moves and its direction, not as a frame per
    sample, so a large mesh with a few sensors travels as the sensors'
    data. A capture past the survey's, and a survey with no time history
    or geometry, are refused."""
    from hertzforge.deform import (
        TimeDeflection,
        animation_records,
        dof_directions,
        quantity_records,
    )

    for needed in ('Time History', 'Geometry'):
        if needed not in run.project:
            raise refused(f'{run.title} has no {needed} to animate.')
    data, geometry = run.project['Time History'], run.project['Geometry']
    captures = (list(dict.fromkeys(data.block)) if data.block is not None
                else [None])
    number = params['capture']
    if number > len(captures):
        raise refused(f'{run.title} has {len(captures)} captures; there is no '
                      f'capture {number}')
    records = [i for i in range(data.num_records)
               if captures[0] is None or data.block[i] == captures[number - 1]]
    records, quantity, other = quantity_records(data, records)
    records, repeated = animation_records(data, records)
    dofs = [data.response_dof[i] for i in records]
    rows, directions, used = dof_directions(geometry, dofs)
    drawn_rows, triangles, lines = _mesh(geometry)
    drawn = np.full(geometry.num_nodes, -1, dtype=np.int64)
    drawn[drawn_rows] = np.arange(drawn_rows.size)
    shown = drawn[rows] >= 0
    played = np.asarray(records)[used][shown]
    values = np.asarray(data.ordinate)[played].real
    if not values.shape[0]:
        raise refused(f'none of {run.title}\'s {quantity} records lands on '
                      'its geometry')
    abscissa = np.asarray(data.abscissa, dtype=float)
    notes = []
    if other:
        notes.append(f'{other} records of another quantity are not shown')
    if repeated:
        notes.append(repeated.lstrip(' —'))
    if not np.all(used) or not np.all(shown):
        notes.append(f'{len(dofs) - values.shape[0]} records have no node on '
                     'the geometry')
    dimension = data.known_dim(records[0])
    capture = (f'Capture {number} of {len(captures)}' if captures[0] is not None
               else 'Time History')
    value_unit = unit_text(dimension) or ''
    extra: dict[str, Any] = {'link': 'time', 'motion': {
        'label': 'Time', 'unit': 's', 'start': float(abscissa[0]),
        'step': float(abscissa[1] - abscissa[0]) if abscissa.size > 1 else 0.0,
        'quantity': quantity.capitalize(), 'value_unit': value_unit,
        'node': drawn[rows[shown]].astype(np.uint32),
        'direction': np.asarray(directions, dtype=np.float32)[shown],
        'values': np.ascontiguousarray(values, np.float32),
        'peak': TimeDeflection(geometry, list(np.asarray(dofs)[used][shown]),
                               values).peak_magnitude}}
    if notes:
        extra['note'] = '; '.join(notes)
    title = f'{run.title}: {capture}, {quantity.capitalize()}'
    plot = v.spectrum(
        title, v.axis('Time', 's', 'linear'),
        v.axis(quantity.capitalize(), value_unit or None, 'linear'),
        [v.series(data.record_label(i), 'measured', abscissa, values[d])
         for d, i in enumerate(played)], link='time')
    return [v.shape3d(title, 'm' if geometry.units_defined else 'undefined',
                      np.asarray(geometry.node_xyz, dtype=float)[drawn_rows], [],
                      triangles=triangles, lines=lines, **extra),
            plot]


def resynthesis(run: ModalRun, params: dict[str, Any]) -> list[dict]:
    """`modal_run.resynthesis`: the FRFs the modes fitted within a band (the
    same fit as `modal_run.modes`) synthesize, against the measured ones,
    as Visual Dynamics' fitting screen draws them. Two `spectrum` views
    over the band: the CMIF, each measured singular value with the
    synthesis's following it, and one FRF record's magnitude with its
    synthesis following it. The fit is rebuilt as the core's fitting
    session from the fitted set (`ModalFitSession.adopt`, how a published
    fit is edited), so the curves are the ones that screen draws. An empty
    band, a survey with no FRFs, a band with no mode and a record past the
    FRF's are refused."""
    from hertzforge.core.modal_fit import ModalFitSession

    low, high = float(params['low_hz']), float(params['high_hz'])
    if not high > low:
        raise refused(f'the band {low:g} to {high:g} Hz is empty; To must be '
                      'above From')
    if 'FRF' not in run.project:
        raise refused(f'{run.title} has no FRF to fit modes on.')
    frf = run.project['FRF']
    record = params['record']
    if record > frf.num_records:
        raise refused(f'{run.title} has {frf.num_records} FRF records; there '
                      f'is no record {record}')
    shapes = fitted(run, low, high, params['count'])
    if not shapes.num_shapes:
        raise refused(f'no mode found between {low:g} and {high:g} Hz')
    session = ModalFitSession(frf)
    session.adopt(shapes)
    f = session.frequencies
    band = (f >= low) & (f <= high)
    x = f[band]
    measured = np.linalg.svd(session.matrix, compute_uv=False).T[:, band]
    synthesized = session.synthesis_singular_values()[:, band]
    unit = unit_text(frf.known_dim(record - 1))
    frequency = v.axis('Frequency', 'Hz', 'linear')
    count = len(measured)
    cmif = v.spectrum(
        f'{run.title}: CMIF of {shapes.num_shapes} Identified Modes',
        frequency, v.axis('CMIF', unit, 'log'),
        [v.series(f'CMIF {k + 1}', 'measured', x, measured[k])
         for k in range(count)]
        + [v.series(f'CMIF {k + 1}, synthesized', 'measured', x,
                    synthesized[k], follows=k) for k in range(count)],
        link='frequency')
    label = frf.record_label(record - 1)
    i = session.rows.index(record - 1)
    response = v.spectrum(
        f'{run.title}: FRF {label}', frequency, v.axis('FRF', unit, 'log'),
        [v.series(label, 'measured', x, np.abs(frf.ordinate[record - 1])[band]),
         v.series(f'{label}, synthesized', 'measured', x,
                  np.abs(session.synthesis_records()[i])[band], follows=0)],
        link='frequency')
    return [cmif, response]


OPERATIONS = (
    Operation(
        id='modal_run.waterfall', title='Waterfall', workspace='modal',
        accepts='modal_run',
        summary='Every FRF, coherence or time capture of the survey at its own '
                'depth, colored by level; an FRF as magnitude, a part or phase.',
        params=(OF, COMPONENT, PAGE, QUANTITY), outputs=('waterfall',),
        run=waterfall),
    Operation(
        id='modal_run.modes', title='Identified Modes', workspace='modal',
        accepts='modal_run',
        summary='Modes fitted to the survey\'s FRFs within a band, on the survey '
                'geometry, one translation per node.',
        params=(LOW, HIGH, COUNT), outputs=('shape3d',), run=modes),
    Operation(
        id='modal_run.ods', title='Operating Shape', workspace='modal',
        accepts='modal_run',
        summary='The survey\'s FRFs to one reference at one frequency line, '
                'a magnitude and phase per node, animated through a cycle.',
        params=(AT, REFERENCE), outputs=('shape3d',), run=ods),
    Operation(
        id='modal_run.mac', title='MAC', workspace='modal',
        accepts='modal_run',
        summary='The Modal Assurance Criterion of the modes fitted within a '
                'band, against the FE model or against each other.',
        params=(LOW, HIGH, COUNT, AGAINST), outputs=('matrix',), run=mac),
    Operation(
        id='modal_run.resynthesis', title='Resynthesis', workspace='modal',
        accepts='modal_run',
        summary='The FRFs the modes fitted within a band synthesize, dashed over '
                'the measured CMIF and one measured FRF.',
        params=(LOW, HIGH, COUNT, RECORD), outputs=('spectrum',),
        run=resynthesis),
    Operation(
        id='modal_run.motion', title='Time Animation', workspace='modal',
        accepts='modal_run',
        summary='One time capture of the survey played back on the survey '
                'geometry, one record per DOF, with its time plot.',
        params=(CAPTURE,), outputs=('shape3d', 'spectrum'), run=motion),
)
