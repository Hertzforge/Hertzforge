"""Operations on a modal survey from the controller: its FRFs, coherence
and time captures as waterfalls, the modes fitted to its FRFs on the
survey geometry, and the operating deflection shape at one frequency.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .datasets import ModalRun, _mesh
from .descriptors import Operation, Option, Param
from .protocol import refused
from .waterfalls import COMPONENTS, waterfall_view

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
                           params['page'], params['component'])]


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


OPERATIONS = (
    Operation(
        id='modal_run.waterfall', title='Waterfall', workspace='analyze',
        accepts='modal_run',
        summary='Every FRF, coherence or time capture of the survey at its own '
                'depth, colored by level; an FRF as magnitude, a part or phase.',
        params=(OF, COMPONENT, PAGE), outputs=('waterfall',), run=waterfall),
    Operation(
        id='modal_run.modes', title='Identified Modes', workspace='analyze',
        accepts='modal_run',
        summary='Modes fitted to the survey\'s FRFs within a band, on the survey '
                'geometry, one translation per node.',
        params=(LOW, HIGH, COUNT), outputs=('shape3d',), run=modes),
    Operation(
        id='modal_run.ods', title='Operating Shape', workspace='analyze',
        accepts='modal_run',
        summary='The survey\'s FRFs to one reference at one frequency line, '
                'a magnitude and phase per node, animated through a cycle.',
        params=(AT, REFERENCE), outputs=('shape3d',), run=ods),
)
