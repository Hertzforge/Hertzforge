"""Operations on the connection's own project (`accepts: "project"`):
each param that names an object lists the project's objects of the kinds
it takes (`options_from: "objects:<kind>"`), and each operation is one of
the core's verbs. A verb that adds an object returns a view of what it
added; the server returns the project's summary beside the views.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .datasets import _mesh
from .descriptors import Operation, Option, Param
from .ops_modal_run import _translations
from .protocol import refused
from .waterfalls import COMPONENTS, QUANTITY, waterfall_view

#: the data objects a waterfall draws, by kind
DATA_KINDS = ('time_history', 'psd', 'frf', 'coherence', 'multiple_coherence',
              'spectrum', 'srs', 'specification')

DATA = Param('data', 'Data', 'enum', options_from='objects:' + '|'.join(DATA_KINDS))
COMPONENT = Param('component', 'Component', 'enum', default='magnitude',
                  options=tuple(Option(key, label) for key, label in COMPONENTS.items()))
PAGE = Param('page', 'Page', 'integer', default=1, min=1, step=1)
HISTORY = Param('history', 'Time history', 'enum', options_from='objects:time_history')
METHOD = Param('method', 'Estimator', 'enum', default='Hv',
               options=(Option('Hv', 'Hv'), Option('H1', 'H1'), Option('H2', 'H2')))
FRF = Param('frf', 'FRF', 'enum', options_from='objects:frf')
LOW = Param('low_hz', 'From', 'number', default=0.0, unit='Hz', min=0.0, step=10.0)
HIGH = Param('high_hz', 'To', 'number', default=1000.0, unit='Hz', min=0.0, step=10.0)
COUNT = Param('count', 'Modes, at most', 'integer', default=10, min=1, max=60, step=1)
SHAPES = Param('shapes', 'Shapes', 'enum', options_from='objects:shape_set')
GEOMETRY = Param('geometry', 'Geometry', 'enum', options_from='objects:geometry')


def waterfall(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.waterfall`: any data object with every record at its own
    depth (`waterfalls.waterfall_view`)."""
    name = params['data']
    return [waterfall_view(name, project[name], params['page'],
                           params['component'], params['quantity'])]


def _derived(project: Any, call: Any) -> list[dict]:
    """Run a verb that adds an object, refusing in the core's words, and
    draw what it added as a waterfall. The view offers no choice of
    quantity: choosing would run the verb again and add a second object.
    The Waterfall operation draws the others."""
    try:
        added = call()
    except ValueError as error:
        raise refused(str(error)) from None
    view = waterfall_view(added, project[added])
    view.pop('quantities', None)
    view.pop('quantity', None)
    return [view]


def compute_psds(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.compute_psds`: PSDs from a time history's averages, added
    to the project (`Project.compute_psds`)."""
    source = params['history']
    return _derived(project, lambda: project.compute_psds(source))


def compute_frfs(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.compute_frfs`: FRFs from a time history, one for each
    response and drive, added to the project (`Project.compute_frfs`)."""
    source = params['history']
    return _derived(project, lambda: project.compute_frfs(source, params['method']))


def _geometry(project: Any) -> str | None:
    """The geometry data is drawn on: the active one, else the first."""
    from hertzforge.core.geometry import Geometry

    if project.active_geometry in project:
        return project.active_geometry
    return next((name for name, obj in project.items()
                 if isinstance(obj, Geometry)), None)


def _shape_view(project: Any, shapes_name: str, geometry_name: str) -> dict:
    """A shape set's modes on a geometry, each a translation per node."""
    shapes, geometry = project[shapes_name], project[geometry_name]
    rows, triangles, lines = _mesh(geometry)
    modes = [v.mode(k + 1, float(shapes.frequency[k]), float(shapes.damping[k]),
                    _translations(geometry, shapes.coordinate,
                                  shapes.shape_matrix[k], rows))
             for k in range(shapes.num_shapes)]
    return v.shape3d(f'{shapes_name} on {geometry_name}',
                     'm' if geometry.units_defined else 'undefined',
                     np.asarray(geometry.node_xyz, dtype=float)[rows], modes,
                     triangles=triangles, lines=lines,
                     normalization='unscaled' if getattr(shapes, 'unscaled', False)
                     else 'mass')


def fit_modes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.fit_modes`: modes fitted to an FRF set within a band, as
    the fitting screen's loop fits them (`Project.fit_modes`), added to
    the project as a shape set. Drawn on the project's geometry when it
    has one, and listed otherwise."""
    low, high = float(params['low_hz']), float(params['high_hz'])
    if not high > low:
        raise refused(f'the band {low:g} to {high:g} Hz is empty; To must be '
                      'above From')
    try:
        added = project.fit_modes(params['frf'], bounds=(low, high),
                                  limit=params['count'])
    except ValueError as error:
        raise refused(str(error)) from None
    shapes = project[added]
    geometry = _geometry(project)
    if geometry is not None:
        return [_shape_view(project, added, geometry)]
    return [v.reading(f'{added}: {shapes.num_shapes} modes', [
        v.row(f'Mode {k + 1}', float(shapes.frequency[k]), 'Hz', 'info')
        for k in range(shapes.num_shapes)])]


def mode_shapes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.mode_shapes`: a shape set's modes on a geometry."""
    return [_shape_view(project, params['shapes'], params['geometry'])]


OPERATIONS = (
    Operation(
        id='project.waterfall', title='Waterfall', workspace='analyze',
        accepts='project',
        summary='Any data object with every record at its own depth, colored '
                'by level; an FRF as magnitude, a part or phase.',
        params=(DATA, COMPONENT, PAGE, QUANTITY), outputs=('waterfall',),
        run=waterfall),
    Operation(
        id='project.compute_psds', title='Compute PSDs', workspace='analyze',
        accepts='project',
        summary='PSDs from a time history\'s averages, added to the project.',
        params=(HISTORY,), outputs=('waterfall',), run=compute_psds),
    Operation(
        id='project.compute_frfs', title='Compute FRFs', workspace='analyze',
        accepts='project',
        summary='FRFs from a time history, one for each response and drive, '
                'added to the project.',
        params=(HISTORY, METHOD), outputs=('waterfall',), run=compute_frfs),
    Operation(
        id='project.fit_modes', title='Fit Modes', workspace='analyze',
        accepts='project',
        summary='Modes fitted to an FRF set within a band, added to the '
                'project as a shape set and drawn on its geometry.',
        params=(FRF, LOW, HIGH, COUNT), outputs=('shape3d', 'reading'),
        run=fit_modes),
    Operation(
        id='project.mode_shapes', title='Mode Shapes', workspace='analyze',
        accepts='project',
        summary='A shape set\'s modes on a geometry, one translation per node.',
        params=(SHAPES, GEOMETRY), outputs=('shape3d',), run=mode_shapes),
)
