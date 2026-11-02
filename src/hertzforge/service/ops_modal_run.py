"""Operations on a modal survey from the controller: its FRFs, coherence
and time captures as waterfalls.
"""

from __future__ import annotations

from typing import Any

from .datasets import ModalRun
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


OPERATIONS = (
    Operation(
        id='modal_run.waterfall', title='Waterfall', workspace='analyze',
        accepts='modal_run',
        summary='Every FRF, coherence or time capture of the survey at its own '
                'depth, colored by level; an FRF as magnitude, a part or phase.',
        params=(OF, COMPONENT, PAGE), outputs=('waterfall',), run=waterfall),
)
