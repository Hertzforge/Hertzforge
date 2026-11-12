"""The Specify workspace's tools on the connection's project: a test
specification derived from measured PSDs or SRSs.
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
from .op_progress import hook
from .ops_project import RECORDS, chosen_records
from .protocol import refused
from .waterfalls import axes

SOURCE = Param('source', 'Measured', 'enum', options_from='objects:psd|srs')
STATISTIC = Param('statistic', 'Limit', 'enum', default='tolerance', options=(
    Option('tolerance', 'Tolerance limit'), Option('distribution_free', 'Distribution-free'),
    Option('mean_k', 'Mean plus k sigma'), Option('envelope', 'Envelope')))
COVERAGE = Param('coverage', 'Coverage', 'number', default=0.95, min=0.5, max=0.999, step=0.01)
CONFIDENCE = Param('confidence', 'Confidence', 'number', default=0.5, min=0.5, max=0.999,
                   step=0.05)
MARGIN = Param('margin_db', 'Margin', 'number', default=0.0, unit='dB', step=1.0)
DOF = Param('dof', 'Written at DOF', 'string', default='')
BREAKPOINTS = Param('max_breakpoints', 'Breakpoints, at most', 'integer', default=12, min=2,
                    max=60, step=1)
K = Param('k', 'k', 'number', default=2.0, min=-10.0, max=20.0, step=0.1)
SIGMA = Param('sigma_db', 'Std. dev. stated', 'number', default=0.0, unit='dB', min=0.0,
              max=30.0, step=0.5)
OVERSHOOT = Param('max_overshoot_db', 'Overshoot', 'number', default=3.0, unit='dB',
                  min=0.0, max=40.0, step=0.5)
RMS = Param('rms', 'RMS', 'enum', default='exceed',
            options=(Option('exceed', 'Exceed'), Option('conserve', 'Conserve')))


def limit_settings(params: dict[str, Any]) -> dict[str, Any]:
    """The arguments `Project.field_limit` and `Project.derive_specification`
    both take, from the params both offer, each limit given only what it
    reads: Derive Specification writes the envelope Field Limit shows."""
    statistic = params['statistic']
    settings: dict[str, Any] = {
        'statistic': statistic, 'coverage': params['coverage'],
        'max_overshoot_db': params['max_overshoot_db'],
        'max_breakpoints': params['max_breakpoints'], 'rms': params['rms']}
    if statistic in ('tolerance', 'distribution_free'):
        settings['confidence'] = params['confidence']
    if statistic == 'mean_k':
        settings['k'] = params['k']
    if statistic in ('tolerance', 'mean_k') and params['sigma_db'] > 0.0:
        settings['sigma_db'] = params['sigma_db']
    return settings


def derive_specification(project: Any, params: dict[str, Any]) -> list[dict]:
    """`project.derive_specification`: the limit over the chosen records,
    the straight-line envelope over it and the margin added, as a
    specification (`Project.derive_specification`). The records are one
    quantity: blank, the largest group of the source's records; the plot
    draws them under the specification derived."""
    import hertzforge as hz
    from hertzforge.drawing.viz.waterfall import waterfall_arrays, waterfall_group

    name = params['source']
    data = project[name]
    named = chosen_records(params['records'], data.num_records) if params['records'].strip() \
        else None
    records, _left = waterfall_group(data, named)
    if not records:
        raise refused(f'{name} has no records to derive from')
    try:
        added = project.derive_specification(
            name, records, **limit_settings(params), margin_db=params['margin_db'],
            dof=params['dof'].strip() or None, progress=hook())
    except (ValueError, TypeError) as error:
        raise refused(str(error)) from None
    measured = waterfall_arrays(data, records, unit_system=hz.SI, exponents=False)
    derived = waterfall_arrays(project[added], unit_system=hz.SI, exponents=False)
    lines = []
    for record, (x, y) in zip(measured['drawn'], measured['curves'], strict=True):
        kept = np.isfinite(x) & np.isfinite(y)
        lines.append(v.series(data.record_label(record), 'measured', x[kept], y[kept]))
    for (x, y) in derived['curves'][:1]:
        kept = np.isfinite(x) & np.isfinite(y)
        lines.append(v.series(added, 'reference', x[kept], y[kept]))
    return [v.spectrum(f'{added} over {len(records)} records of {name}',
                       *axes(data, measured['drawn'][0], 'magnitude', measured), lines)]


OPERATIONS = (
    Operation(
        id='project.derive_specification', title='Derive Specification',
        workspace='specify',
        summary='A test specification from measured PSDs or SRSs: the statistical '
                'limit over the records, the straight-line envelope over it, and a '
                'margin. Records "1-8"; blank for the largest group of one quantity. '
                'A set over several DOFs is written at the DOF named.',
        params=(SOURCE, RECORDS, STATISTIC, COVERAGE, CONFIDENCE, K, SIGMA, OVERSHOOT, RMS,
                MARGIN, BREAKPOINTS, DOF),
        outputs=('spectrum',), run=derive_specification),
)
