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
from .mode_pairs import FIRST_MODES, SECOND_MODES, translations
from .op_progress import hook
from .parsing import parse_optional_picks, parse_picks
from .protocol import refused
from .waterfalls import COMPONENTS, QUANTITY, axes, unit_text, waterfall_view

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
LIMITS = Param('limits', 'Warning and abort limits', 'boolean', default=True)

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
    plot, a density drawn flat across its bins (`_curve`). Records of another
    quantity than the first are left out, since they do not share an axis; the
    view's title says how many. With `limits`, the warning and abort limits of
    an object that carries them follow its records (`_limit_lines`)."""
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
        lines.append(_curve(data.record_label(record), 'measured', data, x, y,
                            params['component']))
    if params.get('limits', True) and params['component'] == 'magnitude':
        lines += _limit_lines(data, arrays['drawn'])
    left = len(records) - len(arrays['drawn'])
    title = name if not left else f'{name} ({left} of another quantity left out)'
    return [v.spectrum(title, *axes(data, arrays['drawn'][0], params['component'], arrays),
                       lines)]


def _curve(name: str, role: Any, data: Any, x: np.ndarray, y: np.ndarray,
           component: str) -> dict:
    """One record's curve as the 2-D plot draws it. A density per bin (a PSD, a
    specification on a controller's lines, anything banded) is a `steps` series on its
    bins' edges, flat across each bin, so the area drawn is the area the values stand
    for. The stage's arrays give such a curve as an outline, two points a bin on its
    edges; sent as points, the plot merged the two at each shared edge and drew a line
    through the bins (Brandon, 2026-11-17). Anything else is its finite points."""
    from hertzforge.drawing.plot import drawing_shape

    x, y = np.asarray(x, dtype=float), np.asarray(y, dtype=float)
    outlined = (drawing_shape(data, component != 'magnitude') == 'steps' and x.size >= 4
                and x.size % 2 == 0 and np.array_equal(x[1:-1:2], x[2::2])
                and np.array_equal(y[::2], y[1::2], equal_nan=True))
    if outlined and np.isfinite(x).all() and np.isfinite(y).all():
        edges = np.concatenate([x[:1], x[1::2]])
        return v.series(name, role, 0.5 * (edges[:-1] + edges[1:]), y[::2], 'steps', edges)
    kept = np.isfinite(x) & np.isfinite(y)
    return v.series(name, role, x[kept], y[kept])


def _limit_lines(data: Any, records: list[int]) -> list[dict]:
    """The warning and abort limits of the records drawn, for an object that carries
    them (a specification): each drawn the way the specification is, as the object its
    `limit` returns, and named by its record when more than one is drawn. A record with
    no limit (not a control channel) adds no curve."""
    import hertzforge as hz
    from hertzforge.drawing.viz.waterfall import waterfall_arrays

    from .spec_lines import NAMES, ROLES

    lines: list[dict] = []
    if not getattr(data, 'limits', None):
        return lines
    for key in data.LIMITS:
        limit = data.limit(key)
        if limit is None:
            continue
        arrays = waterfall_arrays(limit, records, unit_system=hz.SI, exponents=False,
                                  component='magnitude')
        for record, (x, y) in zip(arrays['drawn'], arrays['curves'], strict=True):
            if not (np.isfinite(x) & np.isfinite(y)).any():
                continue
            name = (NAMES[key] if len(records) == 1
                    else f'{data.record_label(record)}: {NAMES[key]}')
            lines.append(_curve(name, ROLES[key], limit, x, y, 'magnitude'))
    return lines


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
    records = picked_records(project, source, params.get('records', ''))
    return _derived(project, lambda: project.compute_psds(source, records=records))


def compute_frfs(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.compute_frfs`: FRFs from a time history, one for each
    response and drive, added to the project (`Project.compute_frfs`)."""
    source = params['history']
    records = picked_records(project, source, params.get('records', ''))
    return _derived(project, lambda: project.compute_frfs(source, params['method'],
                                                          records=records))


def _source(verb: str, label: str = 'Source') -> Param:
    """The object a verb runs on, listed by the core's table of what it
    applies to."""
    return Param('source', label, 'enum', options_from='verb:' + verb)


def _zero_is_unset(value: float) -> float | None:
    return float(value) or None


def picked_dofs_of(obj: Any) -> list[str]:
    """What the tree lists under an object that holds channels, in its
    order, each by its DOF: a data object's records, a channel table's rows,
    a sine object's control DOFs (`ops_sine_shock.sine_dofs`)."""
    from hertzforge.core.channel_table import ChannelTable

    if isinstance(obj, ChannelTable):
        return list(obj.dof_strings())
    if hasattr(obj, 'num_records'):
        return [str(dof) for dof in obj.response_dof]
    from .ops_sine_shock import sine_dofs

    return sine_dofs(obj)


def picked_records(project: Any, name: str, text: str) -> list[int] | None:
    """The items of `name` a `records` param names ('1-3, 7', numbered
    from 1: a data object's records, a shape set's modes, a channel
    table's rows), None for every one when it is blank."""
    obj = project[name]
    count = obj.num_shapes if hasattr(obj, 'num_shapes') else len(picked_dofs_of(obj))
    return parse_optional_picks(text, count, 'record')


def picks_param(of: str) -> Param:
    """The records picked under the object param `of` names, '1-3, 7' or
    blank for every record: filled from the tree by its name,
    `<of>_records` (`action_table`), and left off the form."""
    return Param(f'{of}_records', 'Records (blank: all)', 'string', default='')


def narrowed(project: Any, params: dict[str, Any], of: str) -> Any:
    """The object param `of` names as an action reads it: the records its
    `picks_param` names (`DataArray.kept`), every record when blank."""
    name = params[of]
    return project[name].kept(picked_records(project, name, params.get(f'{of}_records', '')))


def _verb(verb: str, **fixed: Any) -> Any:
    """An operation's run that calls the core verb on `source` with the
    other params as its keywords, and draws what it added. A param named
    in `fixed` is converted first: `fixed[name](value)`. `records`, the
    ones picked under the source, is read against the source's count."""
    def run(project: Any, params: dict[str, Any]) -> list[dict]:
        import inspect

        keywords = {name: fixed.get(name, lambda value: value)(value)
                    for name, value in params.items() if name not in ('source', 'records')}
        if 'records' in params:
            keywords['records'] = picked_records(project, params['source'], params['records'])
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
        count = project.detect_shocks(
            source, records=picked_records(project, source, params.get('records', '')))
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
        added = project.extract_sine(
            params['source'], params['specification'] or None, progress=hook(),
            records=picked_records(project, params['source'], params.get('records', '')))
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
    moved = translations(geometry, shapes, shapes.shape_matrix[picked], rows)
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
                                  limit=params['count'],
                                  records=picked_records(project, params['frf'],
                                                         params.get('records', '')))
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
ONTO = Param('onto', 'Onto', 'enum', options_from='objects:shape_set|channel_table|geometry')
PARTNER_OR_TABLE = Param('shapes', 'Shape set or channel table', 'enum',
                         options_from='objects:shape_set|channel_table')
#: how far a node may be from the nearest node of the other geometry, typed in the display
#: system's length (or the model's own units); 0 for 2 % of the target geometry's size plus
#: half its thickest plate (`Project.match_distance`), which the action table fills in
MATCH = Param('distance', "Node match tolerance (0 for 2 % of the target's size)", 'number',
              default=0.0, unit='m', min=0.0, step=0.001, length=True)
#: DOF Mapping for two shape sets on two geometries, the setting every comparison of the pair
#: reads: the two sets, whose nodes are matched to whose DOFs, and how far a node may be
PAIR_FIRST = Param('first', 'First', 'enum', options_from='objects:shape_set')
PAIR_SECOND = Param('second', 'Second', 'enum', options_from='objects:shape_set|geometry')
#: whose nodes are matched to whose DOFs: the first set's to the second's, or the other way.
#: Asked only when both ways can be made (two sets that each hold every direction at every
#: node); the action table words each way after the sets
PAIR_DIRECTION = Param('direction', 'Matched', 'enum', default='named', options=(
    Option('named', 'First at the DOFs of Second'),
    Option('reversed', 'Second at the DOFs of First')))
#: the modes Expand expands and the modes it expands through, the ones picked under each set
#: in the tree, every one of a set picked whole
EXPANDED_MODES = Param('modes', 'Test modes (blank: all)', 'string', default='')
BASIS_MODES = Param('basis', 'Model modes (blank: all)', 'string', default='')
EXPAND_KEEP = Param('keep', 'Add the expanded set (else show it)', 'boolean', default=False)
#: which of two shape sets is projected onto the other: as `source` and `onto` name them, or
#: the other way. The action table opens on the way that can be made, words each way after
#: the sets, and says why the other cannot be made when it cannot (`project.projection_why`)
DIRECTION = Param('direction', 'Direction', 'enum', default='named', options=(
    Option('named', 'Shape set onto Onto'), Option('reversed', 'Onto onto Shape set')))
EXPAND_MATCH = Param('distance', 'Node match tolerance, on another geometry (0 for 2 % of '
                     "the target's size)", 'number', default=0.0, unit='m', min=0.0,
                     step=0.001, length=True)


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
    distance = float(params.get('distance') or 0.0) or None
    at_table = isinstance(project.get(partner), ChannelTable)
    if not at_table:
        return _derived(project, lambda: project.expand(source, partner))
    try:
        added = project.expand(source, partner, distance=distance)
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


#: how the node matches are drawn: a model node a test node was matched to is reached by a
#: link; a test node matched, one with no model node within the distance, and one the
#: projection does not read (no DOF of the target at it) each in their own color
MATCHED_COLOR = (67, 160, 71)
UNMATCHED_COLOR = (229, 57, 53)
UNREAD_COLOR = (140, 140, 140)
LINK_COLOR = (255, 193, 7)
TARGET_LINE_COLOR = (120, 144, 156)


def node_matches(project: Any, source: str, onto: str, report: dict) -> dict:
    """Both geometries of a projection overlaid in one frame, as `project_shapes` measured
    them (`core.correlate.si_points`): the model's mesh, the target's nodes, those matched
    in one color and those with no model node within the distance in another, the target's
    own lines, and a link from each matched node to the model node it was matched to
    (`report['pairs']`). What Project onto Basis DOFs previews as the distance is typed."""
    from hertzforge.core.correlate import si_points
    from hertzforge.core.geometry import Geometry

    home = (onto, project[onto]) if isinstance(project[onto], Geometry) else project.geometry_for(onto)
    theirs = project.geometry_for(source)
    model, target = theirs[1], home[1]
    model_points, target_points = si_points(model), si_points(target)
    rows, triangles, lines = drawn_mesh(model)
    model_row = {int(node): k for k, node in enumerate(model.node_id)}
    target_row = {int(node): k for k, node in enumerate(target.node_id)}
    matched = [target_row[t] for t, _m in report.get('pairs', [])]
    unmatched = [target_row[t] for t in report.get('unmatched', [])]
    unread = sorted(set(range(target.num_nodes)) - set(matched) - set(unmatched))

    def layer(picked: list[int], color: tuple, size: float) -> dict:
        return {'xyz': np.ascontiguousarray(target_points[picked], np.float32),
                'colors': np.tile(np.asarray(color, np.uint8), (len(picked), 1)), 'size': size}

    links = np.asarray([[target_points[target_row[t]], model_points[model_row[m]]]
                        for t, m in report.get('pairs', [])], dtype=np.float32).reshape(-1, 3)
    # the model node each was matched to, in the link's color: a pair reads as two dots
    ends = {'xyz': np.ascontiguousarray(links[1::2], np.float32),
            'colors': np.tile(np.asarray(LINK_COLOR, np.uint8), (len(links) // 2, 1)),
            'size': 5.0}
    drawn = [layer(unread, UNREAD_COLOR, 4.0), layer(matched, MATCHED_COLOR, 7.0), ends,
             layer(unmatched, UNMATCHED_COLOR, 10.0)]
    _target_rows, _faces, target_lines = drawn_mesh(target)
    edges = target_points[_target_rows][np.asarray(target_lines, dtype=np.int64)].reshape(-1, 3)
    points = [held for held in drawn if len(held['xyz'])]
    segments = [{'xyz': np.ascontiguousarray(xyz, np.float32), 'color': list(color)}
                for xyz, color in ((edges, TARGET_LINE_COLOR), (links, LINK_COLOR)) if len(xyz)]
    return v.shape3d(f'Node matches: {source} onto {onto}',
                     'm' if model.units_defined else 'undefined', model_points[rows], [],
                     triangles=triangles, lines=lines, node_ids=np.asarray(model.node_id)[rows],
                     points=points, segments=segments)


def projection_views(project: Any, source: str, onto: str) -> list[dict]:
    """A shape set and a geometry it does not belong on, picked together: what Reduce would
    add, the set's modes at that geometry's nodes at the distance DOF Mapping keeps for the
    pair, drawn and read and not added (`node_matches`); or why it cannot be reduced, and
    for an expansion, which needs a model's modes, the set to pick it with
    (`project.mapping_why`)."""
    from hertzforge.core.correlate import NoNodeMatched
    from hertzforge.project import mapping_why, projection_why

    from .ops_modal_plan import _report_rows

    why = projection_why(project, source, onto)
    if why:
        rows = [v.row('Not reduced', why, None, 'warn'),
                v.row('Not expanded', mapping_why(project, source, onto, 'expansion'), None,
                      'info'),
                v.row('Drawn on it', 'no: its DOFs are not that geometry\'s nodes, so they '
                      'are not drawn there by number; Link them if they go together', None,
                      'info')]
        return [v.reading(f'{source} cannot be reduced onto {onto}', rows)]
    distance = project.projection_of(source, onto)[2]
    try:
        report = project.projected(source, onto, distance=distance).projection_report
        title = f'{source} at the DOFs of {onto}: what Reduce would add'
    except NoNodeMatched as error:
        report = error.report
        title = (f'No node of {onto} matched {source} within the '
                 f'{"default " if distance is None else ""}distance')
    defined = project.geometry_for(source)[1].units_defined
    return [node_matches(project, source, onto, report),
            v.reading(title, _report_rows(report, defined))]


def expansion_rows(report: dict, defined: bool) -> list[dict]:
    """What an expansion matched and how well it fits: the model modes it was built on, how
    well the test DOFs tell them apart, and the worst misfit at those DOFs
    (`core.correlate.expand_shapes`)."""
    from .ops_modal_plan import _report_rows

    condition = float(report['condition'])
    worst = max(report['residual'], default=0.0)
    return [v.row('Model modes used', float(len(report['modes'])), '', 'info'),
            v.row('Condition', condition, '', 'warn' if condition > 1e3 else 'info'),
            v.row('Worst misfit at the test DOFs', 100.0 * worst, '%',
                  'warn' if worst > 0.1 else 'info'),
            *_report_rows(report, defined)]


def set_projection(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.set_projection`: DOF Mapping, how the DOFs of two shape sets on two geometries
    are mapped onto each other (`Project.set_projection`): one set's nodes matched to the
    other's DOFs within the distance. Kept as it is changed; the node pairs matched are drawn
    (`node_matches`) and read. The comparisons, Reduce and Expand under it read what it keeps.
    For a shape set and a geometry it is not on, the set's nodes matched to the geometry's."""
    from hertzforge.core.correlate import NoNodeMatched
    from hertzforge.core.geometry import Geometry

    first, second = params['first'], params['second']
    source, onto = (second, first) if params['direction'] == 'reversed' else (first, second)
    distance = params['distance'] or None
    try:
        project.set_projection(first, second, source=source, distance=distance)
        report = (project.projected(source, onto, distance=distance).projection_report
                  if isinstance(project[onto], Geometry)
                  else project.compared_sets(first, second)[3])
    except NoNodeMatched as error:
        report = error.report
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    from .ops_modal_plan import _report_rows

    defined = project.geometry_for(source)[1].units_defined
    return [node_matches(project, source, onto, report),
            v.reading(f'{source} at the DOFs of {onto}', _report_rows(report, defined))]


def expand_modes(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.expand_modes`: Expand, a test's modes expanded to a model's DOFs through the
    model's modes (`Project.expand_modes`, SEREP): the modes picked under each set, every one
    of a set picked whole, at the distance the pair's DOF mapping keeps. It opens on what it
    would add and shows it again as the picks change, so a model mode left out shows at once
    what it did to the fit: the expanded modes on the model's geometry, how well they fit,
    and what the node pairs matched. With `keep`, Apply, it adds the set, linked with the
    model."""
    source, onto = params['source'], params['onto']
    modes = parse_optional_picks(params['modes'], project[source].num_shapes, 'test mode')
    basis = parse_optional_picks(params['basis'], project[onto].num_shapes, 'model mode')
    distance = params['distance'] or None
    try:
        if params['keep']:
            added = project.expand_modes(source, onto=onto, modes=modes, basis=basis,
                                         distance=distance)
            expanded, title = project[added], f'{added}: expanded'
        else:
            expanded = project.expanded(source, onto, modes=modes, basis=basis,
                                        distance=distance)
            title = f'{source} expanded through {onto}: what Apply would add'
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    report = expanded.expansion_report
    model = project.geometry_for(onto)
    defined = project.geometry_for(source)[1].units_defined
    # the node pairs are DOF Mapping's to draw; here they are read, beside how well it fits
    return [_shapes_drawn(f'{source} expanded on {model[0]}', expanded, model[1]),
            v.reading(title, expansion_rows(report, defined))]


def onto_basis(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.project_onto_basis`: a shape set sampled at another set's
    DOFs, at a channel table's, or at every node of a geometry
    (`Project.project_onto_basis`), a node matched within the distance
    typed, or `Project.match_distance`'s for 0: the two geometries
    overlaid with the node pairs matched and the nodes left out
    (`node_matches`), what the projection matched, and the set it adds; when
    no node is matched, the first two and nothing added, so the distance can
    be set beside them."""
    from hertzforge.core.correlate import NoNodeMatched
    from hertzforge.core.shapes import ShapeSet

    from .ops_modal_plan import _report_rows

    source, onto = params['source'], params['onto']
    if params.get('direction') == 'reversed':
        if not isinstance(project[onto], ShapeSet):
            raise refused(f'only a shape set is projected the other way; {onto} is not one')
        source, onto = onto, source
    # the modes are the projected set's: the ones picked under it, every one when blank
    modes = parse_optional_picks(params.get('modes', ''), project[source].num_shapes, 'mode')
    try:
        added = project.project_onto_basis(source, onto=onto, modes=modes,
                                           distance=params['distance'] or None)
    except NoNodeMatched as error:
        defined = project.geometry_for(source)[1].units_defined
        return [node_matches(project, source, onto, error.report),
                v.reading(f'No node of {onto} matched, so nothing was added',
                          _report_rows(error.report, defined))]
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    report = project[added].projection_report
    found = project.geometry_for(added)
    reading = v.reading(f'{added}: projection',
                        _report_rows(report, project.geometry_for(source)[1].units_defined))
    # the matches first: they are what the distance is set by, as it is typed
    drawn = [_shape_view(project, added, found[0])] if found is not None else []
    return [node_matches(project, source, onto, report), reading, *drawn]


FIRST = Param('first', 'Rows', 'enum', options_from='objects:shape_set')
SECOND = Param('second', 'Columns', 'enum', options_from='objects:shape_set')


def mac_views(project: Any, first: str, second: str, row_modes: list[int] | None = None,
              column_modes: list[int] | None = None) -> list[dict]:
    """The MAC of `first`'s modes against `second`'s (`mac`), limited to
    the `row_modes` and `column_modes` picked (indices from 0) when
    given, its axes ticked with those modes' frequencies."""
    from .mode_pairs import mode_axis, paired, picked_modes

    rows, columns = project[first], project[second]
    if not rows.num_shapes or not columns.num_shapes:
        raise refused(f'{first if not rows.num_shapes else second} has no modes')
    try:
        values = rows.auto_mac() if first == second else project.comparison_mac(first, second)
    except ValueError as error:
        raise refused(str(error)) from None
    row_modes = picked_modes(rows, row_modes)
    column_modes = picked_modes(columns, column_modes)
    values = np.asarray(values)[np.ix_(row_modes, column_modes)]
    title = f'Auto-MAC of {first}' if first == second else f'MAC of {first} Against {second}'
    matrix = v.matrix(title, mode_axis(first, rows, row_modes),
                      mode_axis(second, columns, column_modes), values, 'MAC', (0.0, 1.0))
    geometry = _geometry(project)
    return paired(None if geometry is None else project[geometry], matrix, rows, row_modes,
                  columns, column_modes)


def mac(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.mac`: the Modal Assurance Criterion of one shape set's
    modes against another's (`Project.comparison_mac`, so across two
    geometries after projection, as the selection and Match Modes read it),
    or a set against itself, of the modes picked under each (every mode when
    none is). With a geometry the matrix selects the pair it shows, the
    column mode overlaid on the row mode."""
    from .mode_pairs import modes_param

    first, second = params['first'], params['second']
    return mac_views(project, first, second,
                     modes_param(project[first], params.get('first_modes', '')),
                     modes_param(project[second], params.get('second_modes', '')))


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
             workspace: Workspace = 'signals', picks: bool = True,
             **fixed: Any) -> Operation:
    """A project operation that runs one core verb on an object it
    applies to and draws what it added, offered in the module `workspace`;
    on the records picked under it, unless `picks` is False for a verb that
    changes the object itself."""
    return Operation(id=f'project.{verb}', title=title, workspace=workspace, summary=summary,
                     params=(_source(verb), *params, *((RECORDS,) if picks else ())),
                     outputs=('waterfall',),
                     run=_verb(verb, **fixed))


#: the modes of a shape set an action reads, filled from what is picked under the set in the
#: tree (`action_table.PICKED`): blank for every mode
PICKED_SET_MODES = Param('modes', 'Modes (blank: all)', 'string', default='')


def transform(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.transform`: the records picked under the source through the
    modes picked under the shape set (`Project.transform`), every record
    or mode of an object picked whole."""
    source, shapes = params['source'], params['shapes']
    records = parse_optional_picks(params.get('records', ''), project[source].num_records,
                                   'record')
    modes = parse_optional_picks(params.get('modes', ''), project[shapes].num_shapes, 'mode')
    return _derived(project, lambda: project.transform(source, shapes, records=records,
                                                       modes=modes))


MODAL_VERB_OPERATIONS = (
    Operation(
        id='project.transform', title='Transform to Modal Coordinates', workspace='modal',
        summary='A record\'s channels through a shape set to modal coordinates, one record '
                'per mode; with a geometry\'s rigid-body shapes this is the virtual point '
                'transformation.',
        params=(_source('transform'), PARTNER, RECORDS, PICKED_SET_MODES),
        outputs=('waterfall',), run=transform),
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
        id='project.set_projection', title='DOF Mapping', workspace='modal',
        summary='How the DOFs of two shape sets on two geometries are mapped onto each other: '
                'each DOF of one matched to the nearest node of the other\'s geometry within '
                'a distance, along its own direction. The MAC and the matches under it compare '
                'the sets on those DOFs, Reduce takes one set\'s modes at them, and Expand '
                'fits the other there and takes the fit to every DOF of the first; the project '
                'keeps it. For a shape set and a geometry it is not on, the set\'s nodes '
                'matched to the geometry\'s.',
        params=(PAIR_FIRST, PAIR_SECOND, PAIR_DIRECTION, MATCH),
        # previewed as it opens and as it is typed, so the matches show before anything is kept
        outputs=('shape3d', 'reading'), run=set_projection, edits=True, previews=True),
    Operation(
        id='project.project_onto_basis', title='Project onto Basis DOFs',
        workspace='modal',
        summary='A shape set sampled at another set\'s DOFs, at a channel table\'s '
                'sensors, or at every node of another geometry, by the nearest node '
                'within the tolerance and its direction.',
        params=(_source('project_onto_basis', 'Shape set'), ONTO, DIRECTION, MATCH,
                PICKED_SET_MODES),
        outputs=('shape3d', 'reading'), run=onto_basis, previews=True),
    Operation(
        id='project.expand_modes', title='Expand to Model DOFs', workspace='modal',
        summary='A test\'s modes expanded to every DOF of a model through the model\'s '
                'modes (SEREP): each test mode picked fitted at the test DOFs as a combination '
                'of the model modes picked, with how well it fits. Previewed as the picks '
                'change.',
        params=(_source('expand_modes', 'Test shape set'),
                Param('onto', 'Model shape set', 'enum', options_from='objects:shape_set'),
                EXPANDED_MODES, BASIS_MODES, MATCH, EXPAND_KEEP),
        outputs=('shape3d', 'reading'), run=expand_modes, previews=True, commits='keep',
        opens_previewed=True, edits_when=lambda params: params['keep']),
)


RESCALE_BY = Param('by', 'Given as', 'enum', default='factor', options=(
    Option('factor', 'Factor'), Option('sensitivities', 'Sensitivities')))
RESCALE_FACTOR = Param('factor', 'Factor', 'number', default=1.0, step=0.1,
                       shown_with=('by', 'factor'))
RECORDED = Param('recorded', 'Recorded with', 'number', default=100.0, min=0.0, step=1.0,
                 unit='mV/unit', shown_with=('by', 'sensitivities'))
CORRECT = Param('correct', 'Correct', 'number', default=100.0, min=0.0, step=1.0,
                unit='mV/unit', shown_with=('by', 'sensitivities'))
RESCALE_KEEP = Param('keep', 'Add the rescaled record (else the preview)', 'boolean',
                     default=False)
#: the picked records a rescale preview draws, each as recorded and rescaled
RESCALE_DRAWN = 8


def _largest(rows: np.ndarray) -> float:
    finite = np.abs(rows[np.isfinite(rows)])
    return float(finite.max()) if finite.size else 0.0


def rescale(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.rescale`: some or all of a time history's records times a
    factor, typed or the recorded sensitivity over the correct one
    (`Project.rescale`). Without `keep`, the picked records drawn as
    recorded and rescaled, the rescaled dashed in each one's colour, and
    the factor, how many records it applies to and the largest value
    before and after; with it, `<name> Rescaled` added."""
    from hertzforge.core.rescale import sensitivity_factor
    from hertzforge.decimate import peak_decimate

    name = params['source']
    history = project[name]
    text = params['records'].strip()
    records = parse_picks(text, history.num_records, 'record') if text else None
    try:
        factor = (sensitivity_factor(params['recorded'], params['correct'])
                  if params['by'] == 'sensitivities' else float(params['factor']))
        if params['keep']:
            return _derived(project, lambda: project.rescale(name, factor, records=records))
        made = history.rescale(factor, records)
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    picked = sorted(set(records)) if records is not None else list(range(history.num_records))
    dim = str(history.ordinate_dim[picked[0]])
    alike = [i for i in picked if str(history.ordinate_dim[i]) == dim]
    unit = unit_text(dim)
    drawn = alike[:RESCALE_DRAWN]
    lines = []
    for i in drawn:
        label = history.record_label(i)
        lines.append(v.series(label, 'measured', *peak_decimate(
            history.abscissa, history.ordinate[i].real, 4000)))
        lines.append(v.series(f'{label} rescaled', 'measured', *peak_decimate(
            made.abscissa, made.ordinate[i].real, 4000), follows=len(lines) - 1))
    title = f'{name}: {len(drawn)} of {len(picked)} picked records as recorded and rescaled'
    if len(alike) < len(picked):
        title += f' ({len(picked) - len(alike)} of another quantity left out)'
    before = _largest(history.ordinate[alike].real)
    after = _largest(made.ordinate[alike].real)
    rows = [v.row('Factor', factor, '', 'info'),
            v.row('Records', f'{len(picked)} of {history.num_records}', None, 'info'),
            v.row('Largest, as recorded', before, unit, 'info'),
            v.row('Largest, rescaled', after, unit, 'info')]
    return [v.spectrum(title, v.axis('Time', 's', 'linear'), v.axis('Amplitude', unit, 'linear'),
                       lines, link='time'),
            v.reading(f'Rescale of {name}', rows)]


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
        id='project.rescale', title='Rescale', workspace='signals',
        summary='The picked records of a time history, or all of them, times a factor, typed '
                '(negative flips the sign) or the sensitivity recorded with over the correct '
                'one, previewed as recorded and rescaled. The channel table is not changed.',
        params=(_source('rescale'), RESCALE_BY, RESCALE_FACTOR, RECORDED, CORRECT, RECORDS,
                RESCALE_KEEP),
        outputs=('spectrum', 'reading', 'waterfall'), run=rescale, commits='keep'),
    Operation(
        id='project.compute_spectra', title='Compute Spectra', workspace='signals',
        summary='Raw linear spectra: each capture of a time history transformed whole, with '
                'no window and no amplitude scaling, and averaged as a complex mean. A sine '
                'of amplitude A on a line reads A times N over 2; the Amplitude Spectrum '
                'reads amplitudes. The history\'s averaging is not read.',
        params=(_source('compute_spectra'), RECORDS), outputs=('waterfall',),
        run=compute_spectra),
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
             'A PSD read as a requirement, which a test is controlled to.', workspace='random',
             picks=False),
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
        params=(_source('detect_shocks'), RECORDS), outputs=('reading',),
        run=detect_shocks, edits=True),
    Operation(
        id='project.extract_sine', title='Extract Sine', workspace='sine',
        summary='The level of each tone of the project\'s sine specification '
                'in a time history, as a sine-level set per tone.',
        params=(_source('extract_sine'), SINE_SPECIFICATION, RECORDS), outputs=('waterfall',),
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
        summary='Chosen records of any data object as curves on one plot, numbered from 1 '
                'as "1-3, 7"; the first eight when none are named. A specification\'s '
                'warning and abort limits are drawn with it.',
        params=(DATA, RECORDS, COMPONENT, LIMITS), outputs=('spectrum',), run=plot),
    Operation(
        id='project.compute_psds', title='Compute PSDs', workspace='random',
        summary='PSDs from a time history\'s averages, added to the project.',
        params=(HISTORY, RECORDS), outputs=('waterfall',), run=compute_psds),
    Operation(
        id='project.compute_frfs', title='Compute FRFs', workspace='modal',
        summary='FRFs from a time history, one for each response and drive, '
                'added to the project.',
        params=(FRF_HISTORY, METHOD, RECORDS), outputs=('waterfall',), run=compute_frfs),
    Operation(
        id='project.fit_modes', title='Fit Modes', workspace='modal',
        summary='Modes fitted to an FRF set within a band, added to the '
                'project as a shape set and drawn on its geometry.',
        params=(FRF, LOW, HIGH, COUNT, RECORDS), outputs=('shape3d', 'reading'),
        run=fit_modes),
    Operation(
        id='project.mac', title='MAC', workspace='modal',
        summary='One shape set\'s modes against another\'s, or a set against '
                'itself; with a geometry, a cell shows that pair overlaid.',
        params=(FIRST, SECOND, FIRST_MODES, SECOND_MODES), outputs=('matrix', 'shape3d'),
        run=mac),
    Operation(
        id='project.mode_shapes', title='Mode Shapes', workspace='modal',
        summary='A shape set\'s modes on a geometry, one translation per node.',
        params=(SHAPES, GEOMETRY), outputs=('shape3d',), run=mode_shapes),
    *VERB_OPERATIONS,
    *MODAL_VERB_OPERATIONS,
)
