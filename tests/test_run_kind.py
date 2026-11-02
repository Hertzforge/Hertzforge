"""What kind of test a Rattlesnake file holds, and what that makes the project.

The controller writes its own `EnvironmentType` into every save, so a
run says whether it was modal, random, shock or several at once. That
is a question the user would otherwise answer from the Project menu,
and the file answers it better: it was written by the thing that ran
the test.

Two of the kinds have a hertzforge project to switch to. A shock run and a
mixed-mode run are named but have nowhere to go yet, and these hold
that they are named rather than mistaken for something else.
"""

from __future__ import annotations

import numpy as np
import pytest
from conftest import fixture_path

from hertzforge import io
from hertzforge.io.rattlesnake import environment_kinds, project_type, run_kind

# Rattlesnake's EnvironmentType, by name
RANDOM, TRANSIENT, SINE, TIME, MODAL = 1, 2, 3, 4, 6


def written(path, *environments, channels=2, samples=512):
    """A Rattlesnake-shaped file declaring the given (name, type) pairs."""
    import netCDF4

    with netCDF4.Dataset(path, 'w', format='NETCDF4') as ds:
        ds.sample_rate = 256.0
        ds.createDimension('response_channels', channels)
        ds.createDimension('time_samples', samples)
        ds.createVariable('time_data', 'f8',
                          ('response_channels', 'time_samples'))[...] = 0.0
        group = ds.createGroup('channels')
        for name, values in (('node_number',
                              [str(100 + i) for i in range(channels)]),
                             ('node_direction', ['Z+'] * channels),
                             ('unit', ['m/s^2'] * channels)):
            group.createVariable(name, str, ('response_channels',))[:] = \
                np.array(values, dtype=object)
        if environments:
            ds.createDimension('num_environments', len(environments))
            names = ds.createVariable('environment_names', str,
                                      ('num_environments',))
            types = ds.createVariable('environment_types', int,
                                      ('num_environments',))
            for i, (label, kind) in enumerate(environments):
                names[i] = label
                types[i] = kind
                ds.createGroup(label)
    return str(path)


@pytest.mark.parametrize('name, kind, wanted', [
    ('modal.nc4', 'modal', 'Modal Test'),
    ('modal_spectra.nc4', 'modal', 'Modal Test'),
    ('random.nc4', 'random', 'Random Vibration'),
    ('random_spectra.nc4', 'random', 'Random Vibration'),
])
def test_a_run_says_what_it_was(name, kind, wanted):
    path = fixture_path('plate', name)
    assert run_kind(path) == kind
    assert project_type(path) == wanted


def test_the_type_comes_from_the_controller_not_the_group_name(tmp_path):
    """An environment's group is named by whoever set the test up, so a
    group called 'Random' is a random environment only by convention —
    and here is one that is not."""
    path = written(tmp_path / 'misleading.nc4', ('Random', MODAL))
    assert environment_kinds(path) == {'Random': 'modal'}
    assert run_kind(path) == 'modal'
    assert project_type(path) == 'Modal Test'


def test_a_transient_run_is_recognized_as_one(tmp_path):
    """A transient run replicates a *waveform*, which is what the
    controller can do; a shock test meets an *SRS*, which it cannot. The
    two were one project type here, and calling a transient run a shock
    test is exactly what makes the difference hard to see."""
    path = written(tmp_path / 'transient.nc4', ('Drop', TRANSIENT))
    assert run_kind(path) == 'transient'
    assert project_type(path) == 'Transient'


def test_random_and_a_sine_sweep_together_are_mixed_mode(tmp_path):
    """Two environments driving the article at once is a kind of its
    own, not whichever of them the file happens to list first. Random
    with a sweep under it is the one mixed run that is a project type
    of its own (Brandon, 2026-09-24), whatever order the file lists
    the two in; every half's specification imports either way."""
    for order in ((('Background', RANDOM), ('Sweep', SINE)),
                  (('Sweep', SINE), ('Background', RANDOM))):
        path = written(tmp_path / f'mixed_{order[0][0]}.nc4', *order)
        assert run_kind(path) == 'mixed'
        assert project_type(path) == 'Random and Sine'


def test_any_other_mixed_run_takes_its_leading_half(tmp_path):
    """A random with a transient beside it has no type of its own, so
    the project answers with the leading half by MIXED_PRECEDENCE —
    random first, the established workflow."""
    path = written(tmp_path / 'odd.nc4', ('Background', RANDOM),
                   ('Drop', TRANSIENT))
    assert run_kind(path) == 'mixed'
    assert project_type(path) == 'Random Vibration'


def test_a_sine_sweep_alone_is_not_mixed_mode(tmp_path):
    path = written(tmp_path / 'sweep.nc4', ('Sweep', SINE))
    assert run_kind(path) == 'sine'


def test_a_replay_beside_a_shaker_run_does_not_make_it_mixed(tmp_path):
    """A time environment records without driving anything, so it says
    nothing about what kind of test this is."""
    path = written(tmp_path / 'replay.nc4',
                   ('Shaker', RANDOM), ('Replay', TIME))
    assert run_kind(path) == 'random'
    assert project_type(path) == 'Random Vibration'


def test_a_replay_on_its_own_is_a_time_run(tmp_path):
    path = written(tmp_path / 'only_replay.nc4', ('Replay', TIME))
    assert run_kind(path) == 'time'
    assert project_type(path) is None


def test_a_file_that_never_says_answers_nothing(tmp_path):
    """An nc4 assembled by hand, with no environment group that holds
    anything: guessing would be worse than not knowing."""
    path = written(tmp_path / 'silent.nc4')
    assert environment_kinds(path) == {}
    assert run_kind(path) is None
    assert project_type(path) is None


def test_an_older_controllers_save_answers_by_its_groups(tmp_path):
    """A controller from before 2026-04 wrote no `environment_types`,
    and a real random run from one came in with no project type and its
    objects in no Basis (Brandon, 2026-09-19). The group the random
    environment saved — its CPSD settings, its specification — is the
    file's own account of the kind; a bare group is still nothing."""
    import netCDF4

    path = written(tmp_path / 'older.nc4')
    with netCDF4.Dataset(path, 'a') as ds:
        env = ds.createGroup('Random')
        env.cpsd_window = 'hann'
        env.frames_in_cpsd = 10
        ds.createGroup('Notes')
    assert environment_kinds(path) == {'Random': 'random'}
    assert project_type(path) == 'Random Vibration'


def test_an_unknown_type_number_is_not_invented(tmp_path):
    """The controller may grow an environment hertzforge has never heard of."""
    path = written(tmp_path / 'future.nc4', ('Something', 99))
    assert environment_kinds(path) == {}
    assert run_kind(path) is None


def test_any_file_can_be_asked(tmp_path):
    """The caller has a path, not a format. A geometry, a photo and a
    file nothing recognizes all answer None rather than raising: not
    knowing is the ordinary case."""
    assert io.project_type_of(
        fixture_path('plate', 'modal.nc4')) == 'Modal Test'
    plain = tmp_path / 'notes.txt'
    plain.write_text('not a test')
    assert io.project_type_of(plain) is None


def test_an_importer_that_cannot_say_is_not_asked(tmp_path):
    """Most formats have nothing to say — a geometry is a geometry
    whatever test it was drawn for."""
    from hertzforge.io import importers

    answering = [imp.name for imp in importers()
                 if imp.project_type is not None]
    assert answering == ['rattlesnake']


def _run_with_environments(path, environments):
    """A stream file naming `environments` as [(name, code, shape)], the
    group shaped the way that kind's save shapes it: 'modal' carries
    the modal metadata, 'time' the output signal, 'bare' nothing."""
    import netCDF4
    import numpy as np

    with netCDF4.Dataset(path, 'w', format='NETCDF4') as ds:
        ds.sample_rate = 256.0
        ds.createDimension('response_channels', 2)
        ds.createDimension('time_samples', 64)
        ds.createVariable('time_data', 'f8',
                          ('response_channels', 'time_samples'))[...] = \
            np.zeros((2, 64))
        group = ds.createGroup('channels')
        for name, values in (('node_number', ['101', '102']),
                             ('node_direction', ['Z+', 'Z+']),
                             ('unit', ['g', 'g'])):
            var = group.createVariable(name, str, ('response_channels',))
            for i, value in enumerate(values):
                var[i] = value
        ds.createDimension('num_environments', len(environments))
        names = ds.createVariable('environment_names', str, ('num_environments',))
        codes = ds.createVariable('environment_types', int, ('num_environments',))
        for i, (name, code, shape) in enumerate(environments):
            names[i] = name
            codes[i] = code
            env = ds.createGroup(name)
            if shape == 'modal':
                env.frf_technique = 'H1'
                env.num_averages = 4
            elif shape == 'time':
                env.createDimension('output_channels', 1)
                env.createDimension('signal_samples', 8)
                env.createVariable('output_signal', 'f8',
                                   ('output_channels', 'signal_samples'))
            elif shape == 'random':
                env.cpsd_window = 'hann'
    return path


def test_a_newer_controllers_time_environment_is_not_a_modal_one(tmp_path):
    """The controller renumbered its enum on 2026-07-30 (modal 6 → 5,
    time 4 → 6): a random run with a time environment beside it read
    as random plus modal, 'mixed', and got no project type — an orange
    bracket where the Basis should be (Brandon, 2026-09-19). The group
    says its kind; the number is the fallback."""
    from hertzforge.io.rattlesnake import environment_kinds, project_type, run_kind

    path = _run_with_environments(str(tmp_path / 'new.nc4'),
                                  [('Random', 1, 'random'), ('Time', 6, 'time')])
    assert environment_kinds(path) == {'Random': 'random', 'Time': 'time'}
    assert run_kind(path) == 'random', 'a time environment is passive'
    assert project_type(path) == 'Random Vibration'


def test_both_numberings_of_a_modal_run_read_as_modal(tmp_path):
    from hertzforge.io.rattlesnake import project_type

    old = _run_with_environments(str(tmp_path / 'old.nc4'), [('Modal', 6, 'modal')])
    new = _run_with_environments(str(tmp_path / 'new.nc4'), [('Modal', 5, 'modal')])
    assert project_type(old) == 'Modal Test'
    assert project_type(new) == 'Modal Test'


def test_a_group_that_says_nothing_falls_back_to_the_older_numbering(tmp_path):
    """A spectral save's group carries no signature; the number reads as
    the fixtures' controller wrote it."""
    from hertzforge.io.rattlesnake import environment_kinds

    path = _run_with_environments(str(tmp_path / 'bare.nc4'),
                                  [('Modal', 6, 'bare'), ('Playback', 4, 'bare')])
    assert environment_kinds(path) == {'Modal': 'modal', 'Playback': 'time'}
