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
from .waterfalls import COMPONENTS, QUANTITY, axes, waterfall_view

#: the data objects a waterfall draws, by kind
DATA_KINDS = ('time_history', 'psd', 'frf', 'coherence', 'multiple_coherence',
              'spectrum', 'srs', 'specification')

DATA = Param('data', 'Data', 'enum', options_from='objects:' + '|'.join(DATA_KINDS))
COMPONENT = Param('component', 'Component', 'enum', default='magnitude',
                  options=tuple(Option(key, label) for key, label in COMPONENTS.items()))
PAGE = Param('page', 'Page', 'integer', default=1, min=1, step=1)
HISTORY = Param('history', 'Time history', 'enum', options_from='verb:compute_psds')
FRF_HISTORY = Param('history', 'Time history', 'enum', options_from='verb:compute_frfs')
METHOD = Param('method', 'Estimator', 'enum', default='Hv',
               options=(Option('Hv', 'Hv'), Option('H1', 'H1'), Option('H2', 'H2')))
FRF = Param('frf', 'FRF', 'enum', options_from='verb:fit_modes')
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


RECORDS = Param('records', 'Records', 'string', default='')

#: the records a plot draws when none are named
FIRST_RECORDS = 8


def chosen_records(text: str, count: int) -> list[int]:
    """The 0-based records '1-3, 7' names, numbered from 1 as the shell
    shows them; the first `FIRST_RECORDS` when `text` is blank."""
    if not text.strip():
        return list(range(min(count, FIRST_RECORDS)))
    out: list[int] = []
    for part in text.replace(' ', '').split(','):
        first, dash, last = part.partition('-')
        try:
            low, high = int(first), int(last) if dash else int(first)
        except ValueError:
            raise refused(f'{part!r} is not a record number or a range such as '
                          '3-6') from None
        if not 1 <= low <= high <= count:
            raise refused(f'records run from 1 to {count}; {part} is outside them')
        out.extend(range(low - 1, high))
    return list(dict.fromkeys(out))


def plot(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.plot`: chosen records of a data object as curves on one
    plot, as Visual Dynamics' data pane draws the records picked in the
    tree. Records of another quantity than the first are left out, since
    they do not share an axis; the view's title says how many."""
    import hertzforge as hz
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    name = params['data']
    data = project[name]
    records = chosen_records(params['records'], data.num_records)
    arrays = waterfall_arrays(data, records, unit_system=hz.SI, exponents=False,
                              component=params['component'])
    if not arrays['drawn']:
        raise refused(f'{name} has no records to draw.')
    lines = []
    for record, (x, y) in zip(arrays['drawn'], arrays['curves'], strict=True):
        kept = np.isfinite(x) & np.isfinite(y)
        lines.append(v.series(data.record_label(record), 'measured', x[kept], y[kept]))
    left = len(records) - len(arrays['drawn'])
    title = name if not left else f'{name} ({left} of another quantity left out)'
    return [v.spectrum(title, *axes(data, arrays['drawn'][0], params['component'], arrays),
                       lines)]


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


def _source(verb: str, label: str = 'Source') -> Param:
    """The object a verb runs on, listed by the core's table of what it
    applies to."""
    return Param('source', label, 'enum', options_from='verb:' + verb)


def _verb(verb: str, **fixed: Any) -> Any:
    """An operation's run that calls the core verb on `source` with the
    other params as its keywords, and draws what it added. A param named
    in `fixed` is converted first: `fixed[name](value)`."""
    def run(project: Any, params: dict[str, Any]) -> list[dict]:
        keywords = {name: fixed.get(name, lambda value: value)(value)
                    for name, value in params.items() if name != 'source'}
        return _derived(project, lambda: getattr(project, verb)(
            params['source'], **keywords))
    return run


DRIFT = Param('drift_corner', 'Drift high-pass', 'number', default=2.0,
              unit='Hz', min=0.0, step=0.5)
RATE = Param('rate', 'New sample rate', 'number', unit='Hz', min=0.0, step=100.0)
ANTI_ALIAS = Param('anti_alias', 'Anti-alias filter', 'boolean', default=True)
SRS_PER_OCTAVE = Param('per_octave', 'Lines per octave', 'integer', default=12,
                       min=1, max=48, step=1)
Q = Param('q', 'Q', 'number', default=10.0, min=0.5, step=1.0)
SRS_KIND = Param('kind', 'Peak', 'enum', default='maximax', options=(
    Option('maximax', 'Maximax'), Option('positive', 'Positive'),
    Option('negative', 'Negative')))
BANDS = Param('per_octave', 'Bands per octave', 'enum', default=6, options=tuple(
    Option(n, label) for n, label in ((1, 'Full octave'), (3, 'Third octave'),
                                      (6, 'Sixth octave'), (12, 'Twelfth octave'),
                                      (24, '24th octave'))))
KEEP_DIAGONAL = Param('keep_diagonal', 'Keep the diagonal', 'boolean', default=True)


def detect_shocks(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.detect_shocks`: mark the shocks found on a time history
    (`Project.detect_shocks`), which Compute SRS then reads."""
    source = params['source']
    try:
        count = project.detect_shocks(source)
    except ValueError as error:
        raise refused(str(error)) from None
    return [v.reading(f'Shocks on {source}', [
        v.row('Shocks marked', float(count), '', 'info')])]


def extract_sine(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.extract_sine`: the levels of each tone of the project's
    sine specification found in a time history (`Project.extract_sine`),
    one sine-level set added per tone."""
    try:
        added = project.extract_sine(params['source'])
    except ValueError as error:
        raise refused(str(error)) from None
    views = []
    for name in added:
        view = waterfall_view(name, project[name])
        view.pop('quantities', None)
        view.pop('quantity', None)
        views.append(view)
    return views


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


def _derives(verb: str, title: str, summary: str, params: tuple = (),
             **fixed: Any) -> Operation:
    """A project operation that runs one core verb on an object it
    applies to and draws what it added."""
    return Operation(id=f'project.{verb}', title=title, workspace='analyze',
                     accepts='project', summary=summary,
                     params=(_source(verb), *params), outputs=('waterfall',),
                     run=_verb(verb, **fixed))


VERB_OPERATIONS = (
    _derives('integrate', 'Integrate',
             'Accelerations become velocities and velocities displacements, '
             'with a high-pass after so a sensor bias does not become a ramp '
             '(0 Hz for none).', (DRIFT,),
             drift_corner=lambda hz: hz or None),
    _derives('differentiate', 'Differentiate',
             'Displacements become velocities and velocities accelerations.'),
    _derives('filter_data', 'Apply Filter',
             'The time history filtered at zero phase with its own filter '
             'settings, or a low-pass at a tenth of the sample rate when it '
             'has none.'),
    _derives('truncate_data', 'Apply Truncation',
             'The time history cut to its truncation span.'),
    _derives('resample', 'Resample',
             'The time history at a lower sample rate, a whole division of '
             'its own, through a zero-phase anti-alias filter.',
             (RATE, ANTI_ALIAS)),
    _derives('compute_spectra', 'Compute Spectra',
             'Linear spectra from a time history\'s averages.'),
    _derives('compute_cpsds', 'Compute CPSDs',
             'The cross-spectral matrix of a time history\'s channels, from '
             'its averages.'),
    _derives('compute_srs', 'Compute SRS',
             'Shock response spectra, one curve per channel per shock marked '
             'on the history (detected first when none is marked).',
             (SRS_PER_OCTAVE, Q, SRS_KIND)),
    _derives('compute_multiple_coherence', 'Compute Multiple Coherence',
             'How much of each response the references explain, frequency by '
             'frequency.'),
    _derives('compute_octave', 'Compute Octave Bands',
             'A PSD or specification integrated onto proportional octave '
             'bands, its total power unchanged.', (BANDS,)),
    _derives('make_realizable', 'Make Realizable',
             'A cross-spectral matrix repaired where no set of signals could '
             'have it.', (KEEP_DIAGONAL,)),
    _derives('convert_to_specification', 'Convert to Specification',
             'A PSD read as a requirement, which a test is controlled to.'),
    _derives('convert_to_psd', 'Convert to PSD',
             'A specification read as a PSD, its limits left out.'),
    Operation(
        id='project.detect_shocks', title='Detect Shocks', workspace='analyze',
        accepts='project',
        summary='Mark the shocks in a time history, which Compute SRS reads.',
        params=(_source('detect_shocks'),), outputs=('reading',),
        run=detect_shocks, edits=True),
    Operation(
        id='project.extract_sine', title='Extract Sine', workspace='analyze',
        accepts='project',
        summary='The level of each tone of the project\'s sine specification '
                'in a time history, as a sine-level set per tone.',
        params=(_source('extract_sine'),), outputs=('waterfall',),
        run=extract_sine),
)


OPERATIONS = (
    Operation(
        id='project.waterfall', title='Waterfall', workspace='analyze',
        accepts='project',
        summary='Any data object with every record at its own depth, colored '
                'by level; an FRF as magnitude, a part or phase.',
        params=(DATA, COMPONENT, PAGE, QUANTITY), outputs=('waterfall',),
        run=waterfall),
    Operation(
        id='project.plot', title='Plot Records', workspace='analyze',
        accepts='project',
        summary='Chosen records of any data object as curves on one plot, '
                'numbered from 1 as "1-3, 7"; the first eight when none are named.',
        params=(DATA, RECORDS, COMPONENT), outputs=('spectrum',), run=plot),
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
        params=(FRF_HISTORY, METHOD), outputs=('waterfall',), run=compute_frfs),
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
    *VERB_OPERATIONS,
)
