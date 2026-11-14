"""A control run's folder is a way in: `Project.open` and `import_file`
take the folder or its `run.json` as the project file it converts to,
converting when there is none or the run changed since and keeping a
file saved since; `hertzforge convert-run` converts one and says what the
run lacked; the service opens one and lists its run record."""

from __future__ import annotations

import os
import time

import run_folders as rf

from hertzforge import Project
from hertzforge.io import import_file, run_convert


def _edit_manifest(folder, **changes):
    import json

    path = os.path.join(folder, 'run.json')
    with open(path) as f:
        manifest = json.load(f)
    for key, value in changes.items():
        if value is None:
            manifest.pop(key)
        else:
            manifest[key] = value
    with open(path, 'w') as f:
        json.dump(manifest, f)



def test_a_run_folder_opens_and_imports_as_its_converted_file(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    target = os.path.join(folder, 'run.hforge')
    opened = Project.open(folder)
    assert opened.file == target and 'Run Record' in opened
    assert isinstance(import_file(os.path.join(folder, 'run.json')), Project)
    project = Project('analysis')
    project.add('Table', run_convert.channel_table_from_rows(rf.random_rows()))
    added = project.import_file(folder)
    assert added == ['Channel Table', *list(opened)[1:]]
    (entry,) = project.data_index.records()
    assert entry['path'] == target


def test_an_open_keeps_a_file_saved_since_and_converts_a_run_that_changed(tmp_path):
    folder = rf.random_run(str(tmp_path / 'run'))
    project = Project.open(folder)
    project.name = 'renamed and saved'
    project.save(project.file)
    assert Project.open(folder).name == 'renamed and saved'
    later = time.time() + 5
    os.utime(os.path.join(folder, 'run.json'), (later, later))
    assert Project.open(folder).name == 'Golden random'


def test_the_command_line_converts_a_run(tmp_path, capsys):
    from hertzforge.__main__ import main

    folder = rf.unfinished_run(str(tmp_path / 'cut'))
    assert main(['convert-run', folder, '-o', str(tmp_path / 'out')]) == 0
    said = capsys.readouterr().out
    assert said.startswith(f'wrote {tmp_path / "out.hforge"}: 9 objects')
    assert 'the run lacked: the run did not end cleanly: there is no end.json' in said
    _edit_manifest(folder, source=None)
    assert main(['convert-run', folder]) == 1
    assert 'no source label' in capsys.readouterr().out


def test_the_service_lists_and_opens_a_converted_run(tmp_path):
    from hertzforge.service.projects import OpenProject, open_file, summary

    folder = rf.random_run(str(tmp_path / 'run'))
    state = open_file({'path': folder})
    listed = {entry['name']: entry for entry in summary(state)['objects']}
    assert listed['Run Record']['kind'] == 'run_record'
    assert listed['Run Record']['holds'] == 'random run, completed, 6 log entries'
    assert listed['inputs']['source'] == 'SIM'
    assert summary(OpenProject(state.project))['file'] == os.path.join(folder, 'run.hforge')
