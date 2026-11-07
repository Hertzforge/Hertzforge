"""The Specify workspace's tools on the connection's project: a test
specification derived from measured PSDs or SRSs, as Visual Dynamics'
Derive Specification writes it (`core.field_limit`).
"""

from __future__ import annotations

from typing import Any

import numpy as np

from . import views as v
from .descriptors import Operation, Option, Param
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
            name, records, statistic=params['statistic'], coverage=params['coverage'],
            confidence=params['confidence'], margin_db=params['margin_db'],
            max_breakpoints=params['max_breakpoints'], dof=params['dof'].strip() or None)
    except ValueError as error:
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
        params=(SOURCE, RECORDS, STATISTIC, COVERAGE, CONFIDENCE, MARGIN, BREAKPOINTS, DOF),
        outputs=('spectrum',), run=derive_specification),
)
