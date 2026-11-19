"""The example project for hertzforge.com: the study's results in one `.hforge`.

The RC plate's test geometry, channel table and modes at the test DOFs, the BARC
response specification's ASDs, and for each scored run the measured control PSDs
over the last 117 frames, one object per run, linked with the specification so each
compares against it. The full FE models and the specification's cross terms are
left out to keep the file small; `make_rc_plate.py` and `make_barc.py` rebuild them.

    python examples/rc_plate_band_average/make_example.py OUT_DIR example.hforge
"""

import json
import sys
from pathlib import Path

import numpy as np

import hertzforge as hz
from hertzforge.core.data import Psd

HERE = Path(__file__).parent
G = 9.80665
TITLES = {
    'pinv': 'Pseudoinverse',
    'ba_knee': 'BA knee',
    'ba_knee_tfu': 'BA knee + FRF update',
    'ba_r70_nogc': 'BA r70, no gain correction',
    'ba_r70': 'BA r70',
    'ba_r70_tfu': 'BA r70 + FRF update',
    'ba_r70_tfu_coh': 'BA r70 + FRF update, coherence gate',
    'ba_r70_tfu_hold': 'BA r70 + FRF update, capped bands held',
    'ba_cross': 'BA cross terms',
    'ba_cross_tfu': 'BA cross terms + FRF update',
}


def psd(frequency, values, dofs, comment):
    """A PSD of the controls in SI from values in g^2/Hz."""
    return Psd(frequency, values * G**2, response_dof=dofs, reference_dof=dofs,
               ordinate_dim='acceleration**2/frequency', ordinate_unit='m/s^2',
               comment=[comment] * len(dofs))


def main() -> None:
    out, target = Path(sys.argv[1]), Path(sys.argv[2])
    source = hz.Project.open(HERE / 'rc_plate.hforge')
    project = hz.Project('RC Plate Band-Average Control')
    for name in ('RC Plate Test Geometry', 'RC Plate Channels', 'RC Plate Modes @ Test DOFs'):
        project.add(name, source[name])
    plant = np.load(HERE / 'rc_plate_state_space.npz', allow_pickle=True)
    inputs = {str(d) for d in plant['input_dof']}
    dofs = [str(d) for d in plant['channel_dof'] if str(d) not in inputs]
    spec = np.load(HERE / 'barc_response_spec.npz', allow_pickle=True)
    asd = np.real(np.einsum('fii->if', spec['cpsd']))
    project.add('BARC Response ASDs', psd(spec['f'], asd, dofs, 'BARC response specification'))
    project.convert_to_specification('BARC Response ASDs')
    scores = {}
    for line in (out / 'scores.jsonl').read_text().splitlines():
        row = json.loads(line)
        scores[row['run']] = row
    for run, row in scores.items():
        prefix, law = run.split('_', 1)
        plant_name = 'linear' if prefix == 'n' else '8 % softening'
        trace = np.load(out / f'{run}_trace.npz')
        keep = trace['frequency'] <= spec['f'][-1]
        note = f"{TITLES[law]}, {plant_name}: {row['band_db']:.2f} dB band error, {row['drive_lbf2']:.3g} lbf^2 drive"
        project.add(f'{TITLES[law]} ({plant_name})',
                    psd(trace['frequency'][keep], trace['psd_channels'][:, keep], dofs, note))
    project.save(target)
    print(f'{target}: {target.stat().st_size / 1e6:.1f} MB, {len(project.names)} objects')


if __name__ == '__main__':
    main()
