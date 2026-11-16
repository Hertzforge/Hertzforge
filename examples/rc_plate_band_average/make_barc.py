"""The study's target: the BARC's response to a random force, made a specification.

`hertzforge.demo.barc.solid_project` is the BARC of bricks with twelve forces at
its box's bottom corners, sixteen triaxial accelerometers on the removable
component, and an uncorrelated force CPSD over 20 to 4000 Hz, its modes solved
to 10 kHz at 1 % damping and projected onto the drives and the accelerometers.
The force CPSD through the drives' modes gives the modal responses, those
through the accelerometers' modes give the accelerometers' CPSD, and that is
made the specification, with warning and abort limits about each ASD
(`barc.specification`; the cross terms have none). It is written as a random
specification in g^2/Hz (`SPEC_OUT`) for `run_study.py`.

    python examples/rc_plate_band_average/make_barc.py
"""

from pathlib import Path

from hertzforge.demo import barc
from hertzforge.units import SYSTEMS

SPEC_OUT = Path(__file__).with_name('barc_response_spec.npz')


def main():
    project = barc.solid_project(solved=True)
    spec = barc.specification(project)
    project.export(spec, SPEC_OUT, unit_system=SYSTEMS['in-slinch-lbf-s (g)'],
                   format='rattlesnake_specification')
    print(f'{spec} written to {SPEC_OUT.name}')


if __name__ == '__main__':
    main()
