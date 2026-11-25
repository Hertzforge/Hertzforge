"""Infer Geometry: the sensors of time histories placed and turned
by the rigid motion they measured, shown before the geometry is made."""

from __future__ import annotations

from typing import Any

import numpy as np

from hertzforge.core.motion_geometry import CLEAR, MOTIONS, POOR_MISFIT
from hertzforge.drawing.viz.geometry import dof_arrow_solids

from . import views as v
from .descriptors import Operation, Param
from .ops_project import MORE, MORE_RECORDS, RECORDS, _more_picks, _source, more_histories
from .ops_scan import _colors
from .parsing import parse_picks
from .protocol import refused

LOW = Param('low', 'Band from', 'number', default=0.0, unit='Hz', min=0.0)
HIGH = Param('high', 'Band to (0 with from 0: chosen)', 'number', default=0.0, unit='Hz',
             min=0.0)
KNOWN = Param('known', 'Geometry holding some of the nodes', 'enum', default='',
              options_from='objects:geometry')
SPAN = Param('span', 'Span of the two farthest sensors [m] (0: not known)', 'number',
             default=0.0, unit='m', min=0.0, length=True)
MIRROR = Param('mirror', 'The other mirror image', 'boolean', default=False)
KEEP = Param('keep', 'Create the geometry', 'boolean', default=False)

#: a one-axis sensor's line, which it may sit anywhere along (sRGB)
SLIDE = (139, 148, 158)
#: the amplitudes drawn are cut off this far under the strongest, so a motion
#: that is exactly zero still has a place on a log axis
FLOOR = 1e-6


def _settings(params: dict[str, Any], project: Any) -> dict[str, Any]:
    """The keywords `Project.motion_sensors` takes, from the form's values."""
    name = params['source']
    more = more_histories(params)
    text = params['records'].strip()
    records = parse_picks(text, project[name].num_records, 'record') if text else None
    picks = _more_picks(project, name, records, more, params.get('more_records', ''))
    low, high = params['low'], params['high']
    return {'band': (low, high) if low or high else None, 'records': picks or None,
            'known': params['known'] or None, 'span': params['span'] or None,
            'mirror': params['mirror']}


def _scene(title: str, recovered: Any) -> dict:
    """The sensors where they were placed: a point each, colored by how
    much of its motion the fit leaves unexplained, an arrow along each
    channel the way the data has it reading, and through a one-axis
    sensor the line it may sit anywhere along."""
    sensors = recovered.sensors
    centers = np.array([one.position for one in sensors])
    reach = float(np.linalg.norm(np.ptp(centers, axis=0))) or 1.0
    view = v.shape3d(title, 'm' if recovered.length_unit else 'undefined', centers, [],
                     node_ids=np.array([one.node for one in sensors]))
    view['points'] = [{'xyz': np.ascontiguousarray(centers, np.float32),
                       'colors': _colors(np.array([one.misfit for one in sensors]),
                                         POOR_MISFIT / 2),
                       'size': 8.0}]
    rows = {one.node: row for row, one in enumerate(sensors)}
    drawn, labels = [], []
    for channel in recovered.motion.channels:
        # as the data has it: a sensor that is left-handed as labeled is drawn left-handed,
        # and a channel left out of its node's sensor has no arrow
        along = recovered.reads_along(channel)
        if along is not None:
            drawn.append((False, (rows[channel.node], along, channel.axis)))
            labels.append(channel.dof)
    view['arrows'] = dof_arrow_solids(centers, drawn, 0.08 * reach, labels)
    ends = [end for one in sensors if one.slides
            for end in (one.position - 0.25 * reach * one.frame[one.axes[0]],
                        one.position + 0.25 * reach * one.frame[one.axes[0]])]
    if ends:
        view['segments'] = [{'xyz': np.ascontiguousarray(ends, np.float32), 'color': list(SLIDE)}]
    return view


def _motions(title: str, motion: Any, chosen: bool) -> dict:
    """The evidence the band rests on: the amplitude of the eight
    strongest motions in each third-octave band, as densities relative to
    the strongest, stepped across their bands, with the band read marked.
    The band is not dragged: a drag stores a setting on the object, and
    this band is the form's, kept with nothing.
    Six stand above the rest where the structure moves as a rigid body."""
    edges = np.asarray(motion.bands, dtype=float)
    density = motion.values / np.sqrt(np.diff(edges))[:, None]
    top = float(density.max()) or 1.0
    centers = np.sqrt(edges[:-1] * edges[1:])
    lines = [v.series(f'Motion {k + 1}', 'measured' if k < MOTIONS else 'other', centers,
                      np.maximum(density[:, k] / top, FLOOR), 'steps', edges)
             for k in range(density.shape[1])]
    low, high = motion.band
    return v.spectrum(title, v.axis('Frequency', 'Hz', 'log'),
                      v.axis('Amplitude density, of the strongest', None, 'log'), lines,
                      regions=[v.region(low, high, 'span', 'Band, chosen' if chosen else 'Band')])


def _table(title: str, recovered: Any) -> dict:
    unit = ' [m]' if recovered.length_unit else ''
    rows = []
    for one in recovered.sensors:
        axes = ' '.join('XYZ'[axis] for axis in one.axes)
        note = '; '.join(filter(None, [f'anywhere along its {axes}' if one.slides else '',
                                       one.note]))
        rows.append([str(one.node), axes, *(f'{value:.6g}' for value in one.position),
                     f'{100 * one.misfit:.2f}', note])
    return v.table(title, ['Node', 'Axes', f'X{unit}', f'Y{unit}', f'Z{unit}', 'Misfit [%]',
                           'Note'], rows)


def _readings(recovered: Any, names: list[str], settings: dict[str, Any]) -> list[dict]:
    motion = recovered.motion
    counts = [sum(len(one.axes) == axes for one in recovered.sensors) for axes in (3, 2, 1)]
    kinds = ', '.join(f'{count} of {word}' for count, word in
                      zip(counts, ('three axes', 'two axes', 'one axis'), strict=True) if count)
    low, high = motion.band
    # placed, but not as labeled: the notes below say which DOFs
    labeled = not recovered.left_out and not any(one.note for one in recovered.sensors)
    rows = [v.row('Sensors', f'{len(recovered.sensors)} placed: {kinds}', None,
                  'pass' if labeled else 'warn'),
            v.row('Histories', ', '.join(names), None, 'info'),
            v.row('Band', f'{low:.4g} to {high:.4g} Hz'
                  + ('' if settings['band'] else ', the lowest in which six motions stand '
                     f'{CLEAR:g} times above the seventh'), None, 'info'),
            v.row('Lines in the band', str(motion.lines), None, 'info'),
            v.row('Motion not explained', f'{100 * recovered.misfit:.2f} %', None,
                  'pass' if recovered.misfit <= POOR_MISFIT else 'warn')]
    pair = recovered.span_nodes
    between = f'nodes {pair[0]} and {pair[1]}' if pair else 'the farthest two'
    if settings['known']:
        rows.append(v.row('Size', f'from {settings["known"]}', None, 'pass'))
    elif settings['span']:
        rows.append(v.row('Size', f'the span typed, between {between}', None, 'pass'))
    else:
        rows.append(v.row('Size', f'not known: {between} are one apart. Give a geometry that '
                          'holds some of the nodes, or the span between those two', None, 'warn'))
    rows.append(v.row('Mirror image',
                      'open: the other image fits the data as well. Four known nodes that '
                      'are not in a plane settle it' if recovered.image_open
                      else f'settled by {settings["known"]}', None,
                      'warn' if recovered.image_open else 'pass'))
    if any(one.slides for one in recovered.sensors):
        rows.append(v.row('One-axis sensors', 'each is somewhere on the line drawn through it; '
                          'its node in the known geometry places it', None, 'info'))
    return rows + v.notes(recovered.notes, 'warn')


def geometry_from_motion(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.geometry_from_motion`: the sensors of one or several time
    histories, placed and turned by the rigid motion in their low band
    (`Project.motion_sensors`). The first view is the sensors where they
    were placed, each channel an arrow; the second the motions in each
    third-octave band, with the band read marked on it; then the sensors
    as a table and what was and was not settled.
    Apply adds the geometry (`Project.geometry_from_motion`); without it
    nothing changes."""
    name = params['source']
    names = [name, *more_histories(params)]
    settings = _settings(params, project)
    try:
        recovered = project.motion_sensors(*names, **settings)
    except (TypeError, ValueError) as error:
        raise refused(str(error)) from None
    title = ', '.join(names)
    readings = _readings(recovered, names, settings)
    if params['keep']:
        try:
            added = project.geometry_from_motion(*names, **settings)
        except (TypeError, ValueError) as error:
            raise refused(str(error)) from None
        readings.append(v.row('Created', added, None, 'pass'))
    else:
        readings.append(v.row('Not created', 'Apply adds the geometry', None, 'info'))
    return [_scene(f'Sensors of {title} from their motion', recovered),
            _motions(f'Motions of {title}', recovered.motion, settings['band'] is None),
            _table(f'Sensors of {title}', recovered),
            v.reading(f'Geometry from the motion of {title}', readings)]


def _kept(params: dict[str, Any]) -> bool:
    return params['keep']


OPERATIONS = (
    Operation(
        id='project.geometry_from_motion', title='Infer Geometry', workspace='signals',
        summary='Where the sensors are and how they are turned, read from the data alone. '
                'Below its first elastic mode a structure moves as a rigid body, and each '
                'channel reads that motion according to where its sensor is and which way it '
                'points. The channels of one node are one sensor. All six rigid motions must '
                'be in the data: pick several histories of the same sensors, each from a test '
                'that shook the structure another way, to pool them. The preview draws the '
                'sensors and the motions in each third-octave band, the band read marked. The '
                'data cannot give the size or tell the layout from its mirror image: a '
                'geometry that holds some of the nodes gives both, or a span the size. Apply '
                'adds a geometry with a node and a coordinate system for each sensor.',
        params=(_source('geometry_from_motion', 'Time history'), LOW, HIGH, KNOWN, SPAN, MIRROR,
                RECORDS, MORE, MORE_RECORDS, KEEP),
        outputs=('shape3d', 'spectrum', 'table', 'reading'),
        run=geometry_from_motion, edits_when=_kept, commits='keep', opens_previewed=True),
)
