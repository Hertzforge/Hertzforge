"""Operations on a modal model: its mode shapes on its mesh.

Shapes are `fem.Model.eigensolution`'s, mass-normalized as it returns
them, turned into one translation per node along the directions
`hertzforge.deform.dof_directions` gives (each DOF's direction, out of
the node's displacement coordinate system), as `node_displacements` does.
Nothing is rescaled here: the shell scales a shape for display.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .datasets import ModalModel
from .descriptors import Operation, Param
from .protocol import refused

COUNT = Param('count', 'Modes', 'integer', default=6, min=1, max=24, step=1)
RIGID = Param('rigid', 'Include rigid-body modes', 'boolean', default=False)


def modes(model: ModalModel, params: dict[str, Any]) -> list[dict]:
    shapes = model.shapes
    frequency = np.asarray(shapes.frequency, dtype=float)
    damping = np.asarray(shapes.damping, dtype=float)
    # a mode that strains no element is given exactly 0 Hz by the solver
    pool = (np.arange(frequency.size) if params['rigid']
            else np.flatnonzero(frequency > 0.0))
    chosen = pool[:params['count']]
    if not chosen.size:
        raise refused(f'{model.title} has no elastic mode among the '
                      f'{frequency.size} solved; include the rigid-body modes '
                      'to see those.')
    out = []
    for k in chosen:
        moved = model.translations(int(k))
        # no damping was stated for the model, and 0 is the solver's word
        # for that: sent as null rather than as a measured zero
        out.append(v.mode(int(k) + 1, frequency[k],
                          float(damping[k]) if damping[k] > 0.0 else None,
                          moved))
    extra: dict[str, Any] = {
        'normalization': 'mass',
        'shape_unit': 'm/√kg' if model.unit == 'm' else 'undefined',
    }
    if chosen.size < params['count']:
        extra['note'] = (f'{chosen.size} of the {params["count"]} modes asked '
                         f'for: the model was solved for its lowest '
                         f'{frequency.size}')
    kind = 'Modes' if params['rigid'] else 'Elastic Modes'
    return [v.shape3d(f'{model.title}: {chosen.size} {kind} (Mass-Normalized)',
                      model.unit, model.nodes, out,
                      triangles=model.triangles, lines=model.lines, **extra)]


OPERATIONS = (
    Operation(
        id='modal.modes', title='Mode Shapes', workspace='modal',
        accepts='modal_model',
        summary='The model\'s lowest normal modes on its mesh, '
                'mass-normalized, one translation per node.',
        params=(COUNT, RIGID), outputs=('shape3d',), run=modes),
)
