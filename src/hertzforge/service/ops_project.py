"""Operations on the connection's own project: each param that names an
object lists the project's objects of the kinds it takes (`options_from:
"objects:<kind>"`), and each operation is one of the core's verbs. A verb that adds an object returns a view of what it
added; the server returns the project's summary beside the views.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from hertzforge.core.rigid import INERTIA_TERMS, MassProperties, rigid_body_shapes

from . import views as v
from .descriptors import Operation, Option, Param, Workspace
from .mesh import drawn_mesh
from .mode_pairs import translations
from .op_progress import hook
from .parsing import parse_picks
from .protocol import refused
from .waterfalls import COMPONENTS, QUANTITY, axes, waterfall_view

#: the data objects a waterfall draws, by kind
DATA_KINDS = ('time_history', 'psd', 'frf', 'coherence', 'multiple_coherence',
              'spectrum', 'srs', 'specification', 'shock_specification',
              'transient_specification')

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
    return parse_picks(text, count, 'record')


def plot(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.plot`: chosen records of a data object as curves on one
    plot. Records of another quantity than the first are left out, since
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
    except (ValueError, TypeError) as error:
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


def _zero_is_unset(value: float) -> float | None:
    return float(value) or None


def _verb(verb: str, **fixed: Any) -> Any:
    """An operation's run that calls the core verb on `source` with the
    other params as its keywords, and draws what it added. A param named
    in `fixed` is converted first: `fixed[name](value)`."""
    def run(project: Any, params: dict[str, Any]) -> list[dict]:
        import inspect

        keywords = {name: fixed.get(name, lambda value: value)(value)
                    for name, value in params.items() if name != 'source'}
        call = getattr(project, verb)
        # a verb that reports its progress reports the operation's (`op_progress`)
        if 'progress' in inspect.signature(call).parameters:
            keywords['progress'] = hook()
        return _derived(project, lambda: call(params['source'], **keywords))
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
SRS_LOW = Param('low', 'Lowest line, 0 for the record\'s band', 'number', default=0.0,
                unit='Hz', min=0.0, step=10.0)
SRS_HIGH = Param('high', 'Highest line, 0 for the record\'s band', 'number', default=0.0,
                 unit='Hz', min=0.0, step=100.0)
BANDS = Param('per_octave', 'Bands per octave', 'enum', default=6, options=tuple(
    Option(n, label) for n, label in ((1, 'Full octave'), (3, 'Third octave'),
                                      (6, 'Sixth octave'), (12, 'Twelfth octave'),
                                      (24, '24th octave'))))
KEEP_DIAGONAL = Param('keep_diagonal', 'Keep the diagonal', 'boolean', default=True)


def detect_shocks(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.detect_shocks`: mark the shocks found on a time history
    (`Project.detect_shocks`), which Compute SRS then reads."""
    from .ops_sine_shock import _editable

    source = params['source']
    _editable(project[source])
    try:
        count = project.detect_shocks(source)
    except ValueError as error:
        raise refused(str(error)) from None
    return [v.reading(f'Shocks on {source}', [
        v.row('Shocks marked', float(count), '', 'info')])]


SINE_SPECIFICATION = Param('specification', 'Specification', 'enum', default='',
                           options_from='objects:sine_sweep_specification')


def extract_sine(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.extract_sine`: the levels of each tone of a sine
    specification found in a time history (`Project.extract_sine`), one
    sine-level set added per tone. The project's one specification when
    none is picked."""
    try:
        added = project.extract_sine(params['source'], params['specification'] or None,
                                    progress=hook())
    except ValueError as error:
        raise refused(str(error)) from None
    from .show import _sine

    views = []
    for name in added:
        spec = project.provenance[name]['params']['specification']
        drawn, _notes = _sine(project, spec, name, {}, {})
        views += drawn
    return views


SPACING = Param('spacing', 'Line spacing, 0 for the one its sheet remembers', 'number',
                default=0.0, unit='Hz', min=0.0, step=1.0)


def convert_to_psd(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.convert_to_psd`: a specification read as a plain PSD or
    CPSD under its own name, its limits left out
    (`Project.convert_to_psd`). One written at breakpoints is read onto
    evenly spaced lines at the spacing given, or the one its sheet
    remembers (`core.author.read_onto_lines`); one on lines converts as it
    is. Drawn with what the Convert pane gives: the
    breakpoints, the range, the lines and the sheet's notes."""
    from hertzforge.core.author import read_onto_lines

    name = params['source']
    spec = project[name]
    f = np.asarray(spec.abscissa, dtype=float)
    breakpoints = spec.interpolation != 'bin'
    spacing = params['spacing'] or None
    used = (spacing or project.remembered_spacing(name)) if breakpoints else None
    notes: list[str] = []
    if used is not None:
        try:
            _lines, notes = read_onto_lines(spec, float(used))
        except ValueError as error:
            raise refused(str(error)) from None
    limits = spec.has_limits
    kind = 'PSD' if spec.is_autospectrum() else 'CPSD'
    views = _derived(project, lambda: project.convert_to_psd(name, spacing))
    lines = len(project[name].abscissa)
    rows = [v.row('Breakpoints' if breakpoints else 'Lines as written', float(f.size), '',
                  'info'),
            v.row('Range', f'{f[0]:g} to {f[-1]:g} Hz', None, 'info'),
            v.row('Lines', float(lines), '', 'info')]
    if used is not None:
        rows.append(v.row('Spacing', float(used), 'Hz', 'info'))
    if limits:
        rows.append(v.row('Limits', 'its warning and abort limits were left out', None,
                          'warn'))
    rows += [v.row('Note', note[0].upper() + note[1:], None, 'info') for note in notes if note]
    return [*views, v.reading(f'{name} converted to a {kind}', rows)]


def _geometry(project: Any) -> str | None:
    """The geometry data is drawn on: the active one, else the first."""
    from hertzforge.core.geometry import Geometry

    if project.active_geometry in project:
        return project.active_geometry
    return next((name for name, obj in project.items()
                 if isinstance(obj, Geometry)), None)


def _shape_view(project: Any, shapes_name: str, geometry_name: str,
                modes: list[int] | None = None) -> dict:
    """A shape set's modes on a geometry, each a translation per node:
    all of them, or the `modes` picked (indices from 0), numbered as the
    set numbers them."""
    return _shapes_drawn(f'{shapes_name} on {geometry_name}', project[shapes_name],
                         project[geometry_name], modes)


def _shapes_drawn(title: str, shapes: Any, geometry: Any,
                  modes: list[int] | None = None) -> dict:
    """Modes on a geometry, whether or not the shape set is in the project."""
    rows, triangles, lines = drawn_mesh(geometry)
    picked = list(modes) if modes else list(range(shapes.num_shapes))
    moved = translations(geometry, shapes.coordinate, shapes.shape_matrix[picked], rows)
    modes = [v.mode(k + 1, float(shapes.frequency[k]), float(shapes.damping[k]), moved[i])
             for i, k in enumerate(picked)]
    return v.shape3d(title,
                     'm' if geometry.units_defined else 'undefined',
                     np.asarray(geometry.node_xyz, dtype=float)[rows], modes,
                     triangles=triangles, lines=lines,
                     node_ids=np.asarray(geometry.node_id)[rows],
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


PARTNER = Param('shapes', 'Shape set', 'enum', options_from='objects:shape_set')
MODES = Param('num_modes', 'Modes', 'integer', default=20, min=1, max=200, step=1)
SOLVE_BY = Param('by', 'Solve for', 'enum', default='count', options=(
    Option('count', 'A number of modes'), Option('frequency', 'Every mode up to a frequency')))
SOLVED_MODES = Param('num_modes', 'Modes', 'integer', default=30, min=1, max=100000, step=1,
                     shown_with=('by', 'count'))
TOP_FREQUENCY = Param('maximum_frequency', 'Highest frequency', 'number', default=1000.0,
                      unit='Hz', min=0.0, step=10.0, shown_with=('by', 'frequency'))
MODAL_DAMPING = Param('damping', 'Damping', 'number', default=2.0, unit='%', min=0.0,
                      max=100.0, step=0.5)
ONTO = Param('onto', 'Onto', 'enum', options_from='objects:shape_set|channel_table')
PARTNER_OR_TABLE = Param('shapes', 'Shape set or channel table', 'enum',
                         options_from='objects:shape_set|channel_table')
EXPAND_MATCH = Param('tolerance', 'Node match tolerance, on another geometry', 'number',
                     default=2.0, unit='%', min=0.01, max=100.0, step=0.5)
MATCH = Param('tolerance', 'Node match tolerance', 'number', default=2.0, unit='%',
              min=0.01, max=100.0, step=0.5)


def _added_shapes(project: Any, call: Any) -> list[dict]:
    """Run a verb that adds a shape set, refusing in the core's words, and
    draw the set on the geometry it belongs to, or list its modes when it
    has none."""
    try:
        added = call()
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    found = project.geometry_for(added)
    geometry = found[0] if found else _geometry(project)
    if geometry is not None:
        return [_shape_view(project, added, geometry)]
    shapes = project[added]
    return [v.reading(f'{added}: {shapes.num_shapes} modes', [
        v.row(f'Mode {k + 1}', float(shapes.frequency[k]), 'Hz', 'info')
        for k in range(shapes.num_shapes)])]


def _point_param(axis: str) -> Param:
    return Param(axis, f'Reference point {axis.upper()}', 'number', default=0.0, unit='m',
                 setting=f'rigid.{axis}')


def _inertia_param(term: str, default: float) -> Param:
    return Param(term.lower(), term, 'number', default=default, unit='kg m^2',
                 setting=f'rigid.{term.lower()}')


RIGID_POINT = tuple(_point_param(axis) for axis in 'xyz')
RIGID_NORMALIZE = Param('normalize', 'Mass-normalize by the mass and inertia about the CG; '
                        'the reference point must be the CG', 'boolean',
                        default=False, setting='rigid.normalize')
RIGID_MASS = Param('mass', 'Mass', 'number', default=1.0, unit='kg', min=0.0,
                   setting='rigid.mass')
RIGID_INERTIA = tuple(_inertia_param(term, 1.0 if term[1] == term[2] else 0.0)
                      for term in INERTIA_TERMS)
RIGID_KEEP = Param('keep', 'Add the shape set (else the preview)', 'boolean', default=True)


def rigid_body_modes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.generate_rigid_body_modes`: the six rigid-body shapes of a
    geometry about a point, and scaled by a mass and inertia about it when
    asked (`Project.generate_rigid_body_modes`). Without `keep`, the shapes drawn and
    nothing added."""
    name = params['source']
    geometry = project[name]
    point = tuple(float(params[axis]) for axis in 'xyz')
    mass, inertia = None, None
    if params['normalize']:
        if not geometry.units_defined:
            raise refused('Declare the length unit to mass-normalize: an inertia in kg m^2 '
                          'means nothing against coordinates with no unit')
        mass = float(params['mass'])
        inertia = tuple(float(params[term.lower()]) for term in INERTIA_TERMS)
    if params['keep']:
        return _added_shapes(project, lambda: project.generate_rigid_body_modes(
            name, point=point, mass=mass, inertia=inertia))
    try:
        shapes = rigid_body_shapes(geometry, MassProperties(point, mass, inertia))
    except ValueError as error:
        raise refused(str(error)) from None
    return [_shapes_drawn(f'Preview of the rigid body modes of {name}', shapes, geometry)]


def solve_modes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.solve_modes`: the normal modes of a geometry whose element
    groups have properties (`Project.solve_modes`), a number of them or all
    up to a highest frequency."""
    if params['by'] == 'frequency':
        if not params['maximum_frequency'] > 0:
            raise refused('a highest frequency above 0 Hz')
        limit = {'maximum_frequency': params['maximum_frequency']}
    else:
        limit = {'num_modes': params['num_modes']}
    return _added_shapes(project, lambda: project.solve_modes(
        params['source'], damping=params['damping'] / 100.0, **limit, progress=hook()))


def expand(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.expand`: modal responses back to physical ones through a
    shape set, or modal FRFs to the FRFs between a channel table's channels
    (`Project.expand`). With a table, the reading the pane gives: the modes,
    references, responses and FRFs, how the shapes were taken, and what was
    dropped or skipped. The tolerance applies only when the table and the set
    are on different geometries."""
    from hertzforge.core.channel_table import ChannelTable

    source, partner = params['source'], params['shapes']
    tolerance = float(params.get('tolerance', 2.0)) / 100.0
    at_table = isinstance(project.get(partner), ChannelTable)
    if not at_table:
        return _derived(project, lambda: project.expand(source, partner))
    try:
        added = project.expand(source, partner, tolerance=tolerance)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    made = project[added]
    view = waterfall_view(added, made)
    view.pop('quantities', None)
    view.pop('quantity', None)
    report = made.transform_report
    references = list(dict.fromkeys(str(d) for d in made.reference_dof))
    responses = list(dict.fromkeys(str(d) for d in made.response_dof))
    modes = next(iter(report.shared.values()), [])
    rows = [v.row('Modes', float(len(modes)), '', 'info'),
            v.row('References', float(len(references)), '', 'info'),
            v.row('Responses', float(len(responses)), '', 'info'),
            v.row('FRFs', float(made.num_records), '', 'info'),
            v.row('Through', str(report.basis), None, 'info')]
    projection = report.projection
    if projection:
        rows.append(v.row('Shapes', f'projected: {projection.get("matched")} of '
                          f'{projection.get("total")} channels matched a node', None, 'info'))
    else:
        rows.append(v.row('Shapes', 'read at the set\'s own DOFs', None, 'info'))
    if report.dropped:
        rows.append(v.row('Dropped', ', '.join(map(str, report.dropped)), None, 'warn'))
    if report.skipped:
        rows.append(v.row('Skipped', ', '.join(f'{count} {kind}' for kind, count
                                               in report.skipped.items()), None, 'info'))
    return [view, v.reading(f'{source} at {partner}', rows)]


def onto_basis(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.project_onto_basis`: a shape set sampled at another set's
    DOFs, or at a channel table's (`Project.project_onto_basis`), a node
    matched within a share of the basis model's size, 2 %."""
    return _added_shapes(project, lambda: project.project_onto_basis(
        params['source'], onto=params['onto'], tolerance=params['tolerance'] / 100.0))


FIRST = Param('first', 'Rows', 'enum', options_from='objects:shape_set')
SECOND = Param('second', 'Columns', 'enum', options_from='objects:shape_set')


def mac_views(project: Any, first: str, second: str, row_modes: list[int] | None = None,
              column_modes: list[int] | None = None) -> list[dict]:
    """The MAC of `first`'s modes against `second`'s (`mac`), limited to
    the `row_modes` and `column_modes` picked (indices from 0) when
    given, its axes ticked with those modes' frequencies."""
    from .mode_pairs import paired

    rows, columns = project[first], project[second]
    if not rows.num_shapes or not columns.num_shapes:
        raise refused(f'{first if not rows.num_shapes else second} has no modes')
    try:
        values = rows.auto_mac() if first == second else project.comparison_mac(first, second)
    except ValueError as error:
        raise refused(str(error)) from None
    row_modes = list(row_modes) if row_modes else list(range(rows.num_shapes))
    column_modes = list(column_modes) if column_modes else list(range(columns.num_shapes))
    values = np.asarray(values)[np.ix_(row_modes, column_modes)]

    def axis(name: str, shapes: Any, modes: list[int]) -> dict:
        return {'label': name, 'ticks': [f'{shapes.frequency[k]:.1f} Hz' for k in modes]}

    title = f'Auto-MAC of {first}' if first == second else f'MAC of {first} Against {second}'
    matrix = v.matrix(title, axis(first, rows, row_modes), axis(second, columns, column_modes),
                      values, 'MAC', (0.0, 1.0))
    geometry = _geometry(project)
    return paired(None if geometry is None else project[geometry], matrix, rows, row_modes,
                  columns, column_modes)


def mac(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.mac`: the Modal Assurance Criterion of one shape set's
    modes against another's (`Project.comparison_mac`, so across two
    geometries after projection, as the selection and Match Modes read it),
    or a set against itself. With a geometry the matrix selects the pair it
    shows, the column mode overlaid on the row mode."""
    return mac_views(project, params['first'], params['second'])


def mode_shapes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.mode_shapes`: a shape set's modes on a geometry."""
    return [_shape_view(project, params['shapes'], params['geometry'])]


def compute_spectra(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.compute_spectra`, the view saying its numbers are raw: under
    an axis in the record's units a sine of 3 reads tens of thousands."""
    views = _verb('compute_spectra')(project, params)
    views[0]['z']['label'] += ', raw FFT: a sine of amplitude A on a line reads A·N/2'
    return views


def _derives(verb: str, title: str, summary: str, params: tuple = (),
             workspace: Workspace = 'signals', **fixed: Any) -> Operation:
    """A project operation that runs one core verb on an object it
    applies to and draws what it added, offered in the module `workspace`."""
    return Operation(id=f'project.{verb}', title=title, workspace=workspace, summary=summary,
                     params=(_source(verb), *params), outputs=('waterfall',),
                     run=_verb(verb, **fixed))


MODAL_VERB_OPERATIONS = (
    _derives('transform', 'Transform to Modal Coordinates',
             'A record\'s channels through a shape set to modal coordinates, one '
             'record per mode; with a geometry\'s rigid-body shapes this is the '
             'virtual point transformation.', (PARTNER,), workspace='modal'),
    Operation(
        id='project.expand', title='Expand to Physical Responses', workspace='modal',
        summary='Modal responses back through the shape set they were transformed through, '
                'at every DOF it covers; modal FRFs with a channel table, the FRFs between '
                'its channels, each response and monitor against each reference.',
        params=(_source('expand'), PARTNER_OR_TABLE, EXPAND_MATCH),
        outputs=('waterfall', 'reading'), run=expand),
    Operation(
        id='project.generate_rigid_body_modes', title='Generate Rigid Body Modes',
        workspace='modal',
        summary='The six rigid-body shapes of a geometry, three translations and '
                'three rotations about a reference point, mass-normalized by a mass and '
                'inertia about it when asked.',
        params=(_source('generate_rigid_body_modes', 'Geometry'), *RIGID_POINT,
                RIGID_NORMALIZE, RIGID_MASS, *RIGID_INERTIA, RIGID_KEEP),
        outputs=('shape3d', 'reading'), run=rigid_body_modes, commits='keep'),
    Operation(
        id='project.solve_modes', title='Solve Modes', workspace='geometry',
        summary='The normal modes of a geometry whose element groups have properties: a '
                'number of them, or all up to a highest frequency. Free-free unless a '
                'group of points is ground; the damping is given to every mode.',
        params=(_source('solve_modes', 'Geometry'), SOLVE_BY, SOLVED_MODES, TOP_FREQUENCY,
                MODAL_DAMPING),
        outputs=('shape3d', 'reading'), run=solve_modes),
    Operation(
        id='project.project_onto_basis', title='Project onto Basis DOFs',
        workspace='modal',
        summary='A shape set sampled at another set\'s DOFs, or at a channel '
                'table\'s sensors, by the nearest node and its direction.',
        params=(_source('project_onto_basis', 'Shape set'), ONTO, MATCH),
        outputs=('shape3d', 'reading'), run=onto_basis),
)


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
    Operation(
        id='project.compute_spectra', title='Compute Spectra', workspace='signals',
        summary='Raw linear spectra: each capture of a time history transformed whole, with '
                'no window and no amplitude scaling, and averaged as a complex mean. A sine '
                'of amplitude A on a line reads A times N over 2; the Amplitude Spectrum '
                'reads amplitudes. The history\'s averaging is not read.',
        params=(_source('compute_spectra'),), outputs=('waterfall',), run=compute_spectra),
    _derives('compute_cpsds', 'Compute CPSDs',
             'The cross-spectral matrix of a time history\'s channels, from '
             'its averages.', workspace='random'),
    _derives('compute_srs', 'Compute SRS',
             'Shock response spectra, one curve per channel per shock marked '
             'on the history (detected first when none is marked), over its own band '
             'or the one asked for.',
             (SRS_PER_OCTAVE, Q, SRS_KIND, SRS_LOW, SRS_HIGH), workspace='shock',
             low=_zero_is_unset, high=_zero_is_unset),
    _derives('compute_multiple_coherence', 'Compute Multiple Coherence',
             'How much of each response the references explain, frequency by '
             'frequency.', workspace='random'),
    _derives('compute_octave', 'Compute Octave Bands',
             'A PSD or specification integrated onto proportional octave '
             'bands, its total power unchanged.', (BANDS,), workspace='random'),
    _derives('make_realizable', 'Make Realizable',
             'A cross-spectral matrix repaired where no set of signals could '
             'have it.', (KEEP_DIAGONAL,), workspace='random'),
    _derives('convert_to_specification', 'Convert to Specification',
             'A PSD read as a requirement, which a test is controlled to.', workspace='random'),
    Operation(
        id='project.convert_to_psd', title='Convert to PSD', workspace='random',
        summary='A specification read as a PSD, its limits left out; one written at '
                'breakpoints is read onto lines at a spacing, 0 for the one its sheet '
                'remembers.',
        params=(_source('convert_to_psd'), SPACING), outputs=('waterfall', 'reading'),
        run=convert_to_psd, edits=True),
    Operation(
        id='project.detect_shocks', title='Detect Shocks', workspace='shock',
        summary='Mark the shocks in a time history, which Compute SRS reads.',
        params=(_source('detect_shocks'),), outputs=('reading',),
        run=detect_shocks, edits=True),
    Operation(
        id='project.extract_sine', title='Extract Sine', workspace='sine',
        summary='The level of each tone of the project\'s sine specification '
                'in a time history, as a sine-level set per tone.',
        params=(_source('extract_sine'), SINE_SPECIFICATION), outputs=('waterfall',),
        run=extract_sine),
)


OPERATIONS = (
    Operation(
        id='project.waterfall', title='Waterfall', workspace='signals', everywhere=True,
        summary='Any data object with every record at its own depth, colored '
                'by level; an FRF as magnitude, a part or phase.',
        params=(DATA, COMPONENT, PAGE, QUANTITY), outputs=('waterfall',),
        run=waterfall),
    Operation(
        id='project.plot', title='Plot Records', workspace='signals', everywhere=True,
        summary='Chosen records of any data object as curves on one plot, '
                'numbered from 1 as "1-3, 7"; the first eight when none are named.',
        params=(DATA, RECORDS, COMPONENT), outputs=('spectrum',), run=plot),
    Operation(
        id='project.compute_psds', title='Compute PSDs', workspace='random',
        summary='PSDs from a time history\'s averages, added to the project.',
        params=(HISTORY,), outputs=('waterfall',), run=compute_psds),
    Operation(
        id='project.compute_frfs', title='Compute FRFs', workspace='modal',
        summary='FRFs from a time history, one for each response and drive, '
                'added to the project.',
        params=(FRF_HISTORY, METHOD), outputs=('waterfall',), run=compute_frfs),
    Operation(
        id='project.fit_modes', title='Fit Modes', workspace='modal',
        summary='Modes fitted to an FRF set within a band, added to the '
                'project as a shape set and drawn on its geometry.',
        params=(FRF, LOW, HIGH, COUNT), outputs=('shape3d', 'reading'),
        run=fit_modes),
    Operation(
        id='project.mac', title='MAC', workspace='modal',
        summary='One shape set\'s modes against another\'s, or a set against '
                'itself; with a geometry, a cell shows that pair overlaid.',
        params=(FIRST, SECOND), outputs=('matrix', 'shape3d'), run=mac),
    Operation(
        id='project.mode_shapes', title='Mode Shapes', workspace='modal',
        summary='A shape set\'s modes on a geometry, one translation per node.',
        params=(SHAPES, GEOMETRY), outputs=('shape3d',), run=mode_shapes),
    *VERB_OPERATIONS,
    *MODAL_VERB_OPERATIONS,
)
