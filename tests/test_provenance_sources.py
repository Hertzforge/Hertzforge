"""A derived object's recipe follows what happened to its source and to
the names it holds: values, removal, renaming and import."""

import dataclasses
import shutil
from pathlib import Path

import numpy as np
import pytest

import hertzforge as hz
from hertzforge import project as project_module
from hertzforge.core.report import Report
from hertzforge.core.shapes import ShapeSet
from hertzforge.core.shocks import Shock
from hertzforge.core.truncate import Truncation
from hertzforge.project import retarget


def history(unit='g'):
    rng = np.random.default_rng(0)
    t = np.arange(4096) / 1024.0
    th = hz.TimeHistory(t, rng.normal(size=(2, 4096)), ['1X+', '2X+'])
    if unit:
        th.define_units(unit)
    return th


def shape_set():
    return ShapeSet(frequency=[10.0, 20.0], damping=[0.01, 0.01],
                    coordinate=['1X+', '2X+'], shape_matrix=np.eye(2),
                    modal_mass=[1.0, 1.0], mass_unit='kg')


def other_psd():
    psd = hz.Psd(np.arange(1, 9) * 1.0, np.ones((1, 8)) + 0j, ['9X+'], ['9X+'])
    psd.define_units('g')
    return psd


# ---- a change to the source's values shows ----------------------------------

def test_a_refreshed_filter_marks_the_psd_made_from_it():
    p = hz.Project('p')
    p.add('th', history())
    p['th'].filtering = p['th'].suggest_filtering()
    filtered = p.filter_data('th')
    psd = p.compute_psds(filtered)
    p['th'].filtering = dataclasses.replace(p['th'].filtering, high=50.0, low=None)
    assert p.refresh_stale() == [filtered, psd]
    assert p.stale() == {}
    assert np.allclose(p[filtered].compute_psds().ordinate, p[psd].ordinate)


def test_defining_the_source_units_marks_the_psd():
    p = hz.Project('p')
    p.add('th', history(None))
    psd = p.compute_psds('th')
    p['th'].define_units('g')
    assert 'values' in p.stale()[psd]
    p.refresh(psd)
    assert p.stale() == {}
    assert p[psd].ordinate_dim[0] != 'unknown'


def test_a_unit_with_no_scale_still_marks_the_psd():
    p = hz.Project('p')
    p.add('th', history(None))
    psd = p.compute_psds('th')
    p['th'].define_units('m/s^2')
    assert psd in p.stale()


def test_deleting_a_source_record_marks_the_psd():
    p = hz.Project('p')
    p.add('th', history())
    psd = p.compute_psds('th')
    p['th'].delete_records([1])
    assert psd in p.stale()
    p.refresh(psd)
    assert p[psd].num_records == 1


def test_an_srs_sees_the_values():
    p = hz.Project('p')
    p.add('th', history())
    p['th'].shocks = (Shock(0.0, 1.0),)
    made = p.compute_srs('th', per_octave=3, q=10.0)
    assert p.stale() == {}
    p['th'].ordinate[0] *= 2.0
    assert made in p.stale()


def test_a_truncation_sees_the_values():
    p = hz.Project('p')
    p.add('th', history())
    p['th'].truncation = Truncation(0.5, 2.0)
    made = p.truncate_data('th')
    assert p.stale() == {}
    p['th'].ordinate[0] *= 2.0
    assert made in p.stale()


def test_correcting_a_coordinate_through_the_project_is_not_a_change():
    p = hz.Project('p')
    p.add('th', history())
    p.compute_psds('th')
    p.rename_dof('th', '1X+', '7X+')
    assert p.stale() == {}


@pytest.mark.parametrize('suffix', ['.hforge', '.mat', '.h5'])
def test_a_saved_project_opens_with_nothing_stale(tmp_path, suffix):
    p = hz.Project('p')
    p.add('th', history())
    p.compute_psds('th')
    path = tmp_path / f'p{suffix}'
    p.save(path)
    assert hz.Project.open(path).stale() == {}


def test_a_long_record_mapped_from_its_file_opens_with_nothing_stale(tmp_path, monkeypatch):
    import hertzforge.io.native as native
    monkeypatch.setattr(native, 'MAP_FROM_BYTES', 1024)
    monkeypatch.setattr(project_module, '_DIGEST_WHOLE_BYTES', 4096)
    p = hz.Project('p')
    p.add('th', history())
    psd = p.compute_psds('th')
    path = tmp_path / 'p.hforge'
    p.save(path)
    opened = hz.Project.open(path)
    assert native._mapping_of(opened['th'].ordinate) is not None
    assert opened.stale() == {}
    opened['th'].delete_records([1])
    assert psd in opened.stale()


def test_a_record_saved_before_the_values_were_kept_compares_settings_only():
    p = hz.Project('p')
    p.add('th', history())
    psd = p.compute_psds('th')
    del p.provenance[psd]['source_print']
    p['th'].ordinate[0] *= 2.0
    assert p.stale() == {}


# ---- a chosen start is not a change of settings ------------------------------

def test_a_chosen_start_flag_is_not_a_change():
    p = hz.Project('p')
    p.add('th', history())
    p['th'].averaging = p['th'].suggest_averaging()
    psd = p.compute_psds('th')
    p.provenance[psd]['state'][1].pop('start_set')
    p['th'].averaging = dataclasses.replace(p['th'].averaging, start_set=True)
    assert p.stale() == {}


# ---- a removed object's recipe goes with it ----------------------------------

def test_an_object_added_under_a_removed_name_has_no_recipe():
    p = hz.Project('p')
    p.add('th', history())
    psd = p.compute_psds('th')
    p.remove(psd)
    assert psd not in p.provenance
    p.add(psd, other_psd())
    with pytest.raises(ValueError, match='records no derivation'):
        p.refresh(psd)
    assert p[psd].response_dof == ['9X+']


def test_a_removed_step_stays_on_record_while_something_descends_through_it():
    p = hz.Project('p')
    p.add('th', history())
    psd = p.compute_psds('th')
    octave = p.compute_octave(psd)
    p.remove(psd)
    assert p._descends_from(octave, 'th')
    p.remove(octave)
    assert p.provenance == {}


def test_a_name_reused_over_a_kept_step_starts_with_no_recipe():
    p = hz.Project('p')
    p.add('th', history())
    psd = p.compute_psds('th')
    p.compute_octave(psd)
    p.remove(psd)
    p.add(psd, other_psd())
    assert psd not in p.provenance


def test_renaming_onto_a_kept_step_starts_with_no_recipe():
    p = hz.Project('p')
    p.add('th', history())
    psd = p.compute_psds('th')
    p.compute_octave(psd)
    p.remove(psd)
    p.add('other', other_psd())
    p.rename('other', psd)
    assert psd not in p.provenance


# ---- a rename reaches everything that names the object -----------------------

def test_renaming_the_shape_set_keeps_the_transform_comparing():
    p = hz.Project('p')
    p.add('th', history())
    p.add('sh', shape_set())
    made = p.transform('th', 'sh')
    p.rename('sh', 'sh2')
    assert p.provenance[made]['params']['shapes'] == 'sh2'
    p['sh2'].shape_matrix[0, 0] = 2.0
    assert made in p.stale()
    p.refresh(made)
    assert p.stale() == {}


def test_renaming_a_source_keeps_the_derive_recipe():
    p = hz.Project('p')
    p.add('a', other_psd())
    p.add('b', other_psd())
    p['b'].ordinate *= 2.0
    made = p.derive_specification(['a', 'b'], statistic='envelope')
    p.rename('b', 'b2')
    assert p.provenance[made]['params']['sources'] == ['a', 'b2']
    p.refresh(made)


def test_renaming_the_sine_specification_keeps_the_recipe():
    p = hz.Project('p')
    p.add('spec', history())
    p.add('levels', history())
    p.provenance['levels'] = {'verb': 'extract_sine', 'source': 'th',
                              'params': {'specification': 'spec'}, 'state': None}
    p.rename('spec', 'spec2')
    assert p.provenance['levels']['params']['specification'] == 'spec2'


@pytest.mark.parametrize('key', ['floor', 'specification', 'source'])
def test_a_report_block_follows_the_rename(key):
    report = Report()
    report.blocks = [{key: 'old'}]
    retarget(report, {'old': 'new'})
    assert report.blocks[0][key] == 'new'


# ---- an imported project keeps its recipes -----------------------------------

def saved(tmp_path):
    p = hz.Project('saved')
    p.add('th', history())
    p.add('sh', shape_set())
    p.compute_psds('th')
    p.transform('th', 'sh')
    path = tmp_path / 'saved.hforge'
    p.save(path)
    return path


def test_importing_into_an_empty_project_keeps_the_recipes(tmp_path):
    path = saved(tmp_path)
    p = hz.Project('e')
    p.import_file(path)
    assert set(p.provenance) == {'th PSDs', 'th Modal Coordinates'}
    assert p.stale() == {}
    p['th'].delete_records([1])
    assert 'th PSDs' in p.stale()
    p.refresh('th PSDs')


def test_a_name_clash_renames_the_recipe_and_its_source(tmp_path):
    path = saved(tmp_path)
    p = hz.Project('e')
    p.add('th', history())
    p.add('sh', shape_set())
    p.import_file(path)
    assert p.provenance['th PSDs']['source'] == 'th (2)'
    record = p.provenance['th Modal Coordinates']
    assert record['source'] == 'th (2)'
    assert record['params']['shapes'] == 'sh (2)'
    assert p.stale() == {}


def test_a_source_gone_from_the_file_is_not_read_as_a_same_named_object(tmp_path):
    p = hz.Project('saved')
    p.add('th', history())
    psd = p.compute_psds('th')
    p.remove('th')
    path = tmp_path / 'saved.hforge'
    p.save(path)
    here = hz.Project('e')
    here.add('th', history())
    here.import_file(path)
    assert psd not in here.provenance


# ---- older files --------------------------------------------------------------

CORPUS = Path.home() / 'visualdynamics/stressdata/plate_projects'


@pytest.mark.skipif(not (CORPUS / 'random.vdyn').exists(), reason='no corpus here')
@pytest.mark.parametrize('name', ['random.vdyn', 'sysid.vdyn'])
def test_an_older_project_opens_with_nothing_stale(tmp_path, name):
    shutil.copy(CORPUS / name, tmp_path / name)
    assert hz.Project.open(tmp_path / name).stale() == {}
