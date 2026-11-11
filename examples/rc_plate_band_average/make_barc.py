"""The study's target: the BARC's response to a random force, made a specification.

`hertzforge.demo.barc.solid_project` is the BARC of bricks with twelve forces at
its box's bottom corners, sixteen triaxial accelerometers on the removable
component, and an uncorrelated force CPSD over 20 to 4000 Hz, its modes solved
to 10 kHz at 1 % damping and projected onto the drives and the accelerometers.
The force CPSD through the drives' modes gives the modal responses, those
through the accelerometers' modes give the accelerometers' CPSD, and that is
made the specification, with warning and abort limits about each ASD
(`WARNING_DB`, `ABORT_DB`; the cross terms have none). It is written as a random
specification in g^2/Hz (`SPEC_OUT`) for `run_study.py`.

    python examples/rc_plate_band_average/make_barc.py
"""

from pathlib import Path

import numpy as np

from hertzforge.demo import barc
from hertzforge.units import SYSTEMS

SPEC_OUT = Path(__file__).with_name('barc_response_spec.npz')
#: the specification's warning and abort limits about each ASD, dB
WARNING_DB = 3.0
ABORT_DB = 6.0


def add_limits(spec):
    """Warning and abort limits `WARNING_DB` and `ABORT_DB` about each ASD of
    the Specification `spec`; NaN on its cross terms, which have none."""
    auto = np.array([a == b for a, b in zip(spec.response_dof, spec.reference_dof)])
    asd = np.where(auto[:, None], np.real(spec.ordinate), np.nan)
    spec.limits = {f'{kind}_{side}': asd * 10 ** (sign * db / 10)
                   for kind, db in (('warning', WARNING_DB), ('abort', ABORT_DB))
                   for side, sign in (('lower', -1), ('upper', 1))}


def main():
    project = barc.solid_project(solved=True)
    modal = project.compute_modal_responses('BARC Force CPSD', 'BARC Modes @ Force DOFs')
    response = project.expand(modal, 'BARC Modes @ Response DOFs', name='BARC Response CPSD')
    project.convert_to_specification(response)
    spec = project.rename(response, 'BARC Response Specification')
    add_limits(project[spec])
    project.export(spec, SPEC_OUT, unit_system=SYSTEMS['in-slinch-lbf-s (g)'],
                   format='rattlesnake_specification')
    print(f'{spec} written to {SPEC_OUT.name}')


if __name__ == '__main__':
    main()
