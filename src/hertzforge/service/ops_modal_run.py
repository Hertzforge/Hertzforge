"""Operations on a modal survey from the controller: its FRFs, coherence
and time captures as waterfalls, and the modes fitted to its FRFs on the
survey geometry.
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
        name = run.project.fit_modes('FRF', bounds=(low, high), limit=count,
                                     name='Identified Modes')
        run.fits[key] = run.project[name]
        del run.project[name]
    return run.fits[key]


def modes(run: ModalRun, params: dict[str, Any]) -> list[dict]:
    """`modal_run.modes`: the modes fitted to the survey's FRFs within a
    band, each as a translation per node of the survey geometry, as one
    `shape3d` view. Nodes with no measured DOF stay where they are. A band
    that is empty, backwards or outside the FRFs, a survey with no
    geometry or FRFs, and complex shapes are refused."""
    from hertzforge.deform import dof_directions

    low, high = float(params['low_hz']), float(params['high_hz'])
    if not high > low:
        raise refused(f'the band {low:g} to {high:g} Hz is empty; To must be '
                      'above From')
    for needed in ('FRF', 'Geometry'):
        if needed not in run.project:
            raise refused(f'{run.title} has no {needed} to fit modes on.')
    geometry = run.project['Geometry']
    lines_hz = np.asarray(run.project['FRF'].abscissa, dtype=float)
    # a band that misses the FRFs must not come back as a fit of the whole
    # band, which is what the fitting loop does with bounds it cannot use
    if high <= lines_hz[0] or low >= lines_hz[-1]:
        raise refused(f'no mode found between {low:g} and {high:g} Hz: the FRFs '
                      f'span {lines_hz[0]:g} to {lines_hz[-1]:g} Hz')
    shapes = fitted(run, low, high, params['count'])
    if not shapes.num_shapes:
        raise refused(f'no mode found between {low:g} and {high:g} Hz')
    if shapes.is_complex:
        raise refused('the fitted shapes are complex, and the viz kit draws '
                      'real translations only')
    rows, triangles, lines = _mesh(geometry)
    dof_rows, dof_vectors, dof_used = dof_directions(geometry, shapes.coordinate)
    out = []
    for k in range(shapes.num_shapes):
        values = np.asarray(shapes.shape_matrix[k])[dof_used]
        moved = np.stack([np.bincount(dof_rows, dof_vectors[:, axis] * values,
                                      minlength=geometry.num_nodes)
                          for axis in range(3)], axis=1)[rows]
        out.append(v.mode(k + 1, float(shapes.frequency[k]),
                          float(shapes.damping[k]), moved))
    xyz = np.asarray(geometry.node_xyz, dtype=float)[rows]
    extra: dict[str, Any] = {'normalization': 'unscaled'
                             if getattr(shapes, 'unscaled', False) else 'mass'}
    if shapes.num_shapes < params['count']:
        extra['note'] = (f'{shapes.num_shapes} of the {params["count"]} modes '
                         f'asked for were found between {low:g} and {high:g} Hz')
    return [v.shape3d(f'{run.title}: {shapes.num_shapes} Identified Modes',
                      'm' if geometry.units_defined else 'undefined', xyz, out,
                      triangles=triangles, lines=lines, **extra)]


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
)
