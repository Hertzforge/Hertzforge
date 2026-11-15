"""`runs.*`: the data folder control runs are kept in, and deleting a run.

The verbs are the analysis service's because a run's converted project file
is its: it knows which file is open and which files an open project maps,
and a run holding one is not deleted. The checks and the delete itself are
`hertzforge.io.data_folder`'s, the one place a run is deleted.
"""

from __future__ import annotations

import os
import tempfile
from collections.abc import Iterable, Mapping
from typing import Any

from .protocol import ProtocolError, bad_request, not_found, refused

RUN_METHODS = ('runs.data_folder', 'runs.delete')


def _writable(folder: str) -> str | None:
    """None when a file can be made in `folder`, else why not."""
    try:
        with tempfile.TemporaryFile(dir=folder):
            pass
    except OSError as error:
        return error.strerror or str(error)
    return None


def data_folder() -> dict[str, Any]:
    """`runs.data_folder`: where runs are kept, as this service resolves
    it, whether it exists, and whether a run's record can be written there
    now (and when not, why)."""
    from hertzforge.io.data_folder import data_root, data_root_source

    path = data_root()
    out: dict[str, Any] = {'path': path, 'source': data_root_source(),
                           'exists': os.path.isdir(path)}
    if out['exists']:
        why = _writable(path)
        out['writable'] = why is None
        if why is not None:
            out['reason'] = why
    return out


def delete(params: Mapping[str, Any], open_files: Iterable[str]) -> dict[str, Any]:
    """`runs.delete`: the run folder `folder` names deleted with its
    converted project file; with `dry_run`, only what would be deleted,
    after the same checks. A run that did not end cleanly is deleted only
    with `not_live`, the caller's word that the control service reports no
    live run recording into it. A refusal says why in its message and
    deletes nothing."""
    from hertzforge.io import native
    from hertzforge.io.data_folder import RunDeleteRefused, delete_run, plan_delete

    unknown = sorted(set(params) - {'folder', 'dry_run', 'not_live'})
    if unknown:
        raise bad_request(f'runs.delete takes "folder", "dry_run" and "not_live", '
                          f'not {unknown}')
    folder = params.get('folder')
    if not isinstance(folder, str) or not folder:
        raise bad_request('runs.delete needs "folder", the run folder\'s path')
    dry_run = params.get('dry_run', False)
    if not isinstance(dry_run, bool):
        raise bad_request('runs.delete "dry_run" is true or false')
    not_live = params.get('not_live', False)
    if not isinstance(not_live, bool):
        raise bad_request('runs.delete "not_live" is true or false')
    held = [*open_files, *native.mapped_files()]
    try:
        plan = (plan_delete if dry_run else delete_run)(folder, open_files=held,
                                                       not_live=not_live)
    except RunDeleteRefused as error:
        if error.code == 'not_found':
            raise not_found(error.message) from None
        raise refused(error.message) from None
    except OSError as error:
        raise ProtocolError('refused', f'the run was not wholly deleted: {error}') from None
    return {'folder': plan.folder, 'project_file': plan.project_file,
            'project_file_exists': plan.project_file_exists, 'complete': plan.complete,
            'deleted': not dry_run}


def request(method: str, params: Any, open_files: Iterable[str]) -> dict[str, Any]:
    """Answer one `runs.*` request."""
    if not isinstance(params, Mapping):
        raise bad_request(f'{method} params must be an object')
    if method == 'runs.data_folder':
        if params:
            raise bad_request('runs.data_folder takes no params')
        return data_folder()
    return delete(params, open_files)
