"""The study's plant: the RC plate's modes at its channels, as a modal state space.

`hertzforge.demo.rc_plate.project` is the BARC's removable component bolted to a
free aluminum plate, twelve forces at the plate's corners and the sixteen
triaxial accelerometers the BARC's specification is written at, its modes
solved to 10 kHz at 1 % damping and projected onto every channel. They are
written as a state space (`SS_OUT`) for `run_study.py`: the twelve forces as
inputs, every channel an output in its own unit (lbf, g), the modes below
`NYQUIST`, the rigid-body modes lifted to 1 Hz. The project is saved beside it
for `make_example.py`.

    python examples/rc_plate_band_average/make_rc_plate.py [--export]

`--export` writes only the state-space file, from the saved `rc_plate.hforge`.
"""

import sys
from pathlib import Path

from hertzforge import Project
from hertzforge.demo import rc_plate

OUT = Path(__file__).with_name('rc_plate.hforge')
SS_OUT = Path(__file__).with_name('rc_plate_state_space.npz')
#: half the controller's 16 384 Hz sample rate: a mode above it aliases in the
#: simulation. It keeps the modes to twice the specification's 4 kHz top; at 4096
#: the seven modes from 4256 to 5784 Hz were cut and their residual was missing
#: near the top of the band
NYQUIST = 8192.0


def export(project):
    return project.export_state_space('RC Plate Modes @ Test DOFs', 'RC Plate Channels', SS_OUT,
                                      maximum_frequency=NYQUIST, rigid_frequency=1.0)


def main():
    project = rc_plate.project(solved=True)
    project.save(OUT)
    print(f'{OUT.name}, {SS_OUT.name}: {export(project).summary()}')


if __name__ == '__main__':
    if '--export' in sys.argv:
        print(f'{SS_OUT.name}: {export(Project.open(OUT)).summary()}')
    else:
        main()
