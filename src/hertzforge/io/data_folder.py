"""The data folder control runs are kept in, and deleting a run from it.

The control service writes each run's folder under its data folder
(`docs/run-folder.md`): the folder `HERTZFORGE_DATA_ROOT` names, else
`Hertzforge_Data` in the home folder. `data_root` resolves it by the same
rule, so the analysis service, started beside the control service with the
same environment, names the same folder.

`delete_run` is the one place a run is deleted. It removes a run folder,
and with it the project file the folder was converted to (`<run name>.hforge`
inside it, `run_convert.default_output`), and only ever a folder that is a
run folder inside the data folder, reached without a link, whose run has
ended and whose project file is not open. `plan_delete` makes every check
and deletes nothing, so a confirmation can name what would go.
"""

from __future__ import annotations

import json
import os
import shutil
from collections.abc import Iterable
from dataclasses import dataclass

from . import run_folder

#: the environment variable that names the data folder, as the control
#: service reads it
DATA_ROOT_ENV = 'HERTZFORGE_DATA_ROOT'

#: the data folder's name in the home folder when the variable is unset
DATA_ROOT_DIR_NAME = 'Hertzforge_Data'


def data_root() -> str:
    """The data folder, as the control service resolves it: the folder
    `HERTZFORGE_DATA_ROOT` names, used as it is (an empty value is unset),
    else `Hertzforge_Data` in the home folder, else in the working folder
    when there is no home folder.

    Returns
    -------
    str
        The folder's absolute path. It need not exist yet.
    """
    named = os.environ.get(DATA_ROOT_ENV, '')
    if named:
        return os.path.abspath(named)
    home = os.path.expanduser('~')
    if not home or home == '~':
        return os.path.abspath(DATA_ROOT_DIR_NAME)
    return os.path.join(home, DATA_ROOT_DIR_NAME)


def data_root_source() -> str:
    """Where the data folder comes from: `environment` when
    `HERTZFORGE_DATA_ROOT` names it, `default` otherwise."""
    return 'environment' if os.environ.get(DATA_ROOT_ENV, '') else 'default'


class RunDeleteRefused(Exception):
    """A run that is not deleted, and why: `code` names the refusal, the
    message says it in words a person can act on. Nothing was deleted."""

    #: every refusal `plan_delete` makes
    CODES = ('not_found', 'not_a_run_folder', 'outside_data_folder', 'link',
             'link_inside', 'incomplete', 'open')

    def __init__(self, code: str, message: str) -> None:
        if code not in self.CODES:
            raise ValueError(f'unknown refusal {code!r}')
        super().__init__(message)
        self.code: str = code
        self.message: str = message


@dataclass(frozen=True)
class RunDelete:
    """What deleting a run removes: the run folder, and the project file
    it was converted to, which lies inside it."""

    #: the run folder, by its real path
    folder: str
    #: the converted project file (`<run name>.hforge` in the folder)
    project_file: str
    #: whether the run has been converted, so the file is there to delete
    project_file_exists: bool
    #: whether its `run.json` says the run ended cleanly (`complete: true`);
    #: false only when the caller established the run is not live
    complete: bool = True


def _is_link(path: str) -> bool:
    return os.path.islink(path) or os.path.isjunction(path)


def _inside(path: str, root: str) -> str | None:
    """`path` relative to `root` when `path` lies inside it, the parts
    compared as the file system compares them; None otherwise, and for
    `root` itself."""
    try:
        rel = os.path.relpath(path, root)
    except ValueError:      # another drive, on Windows
        return None
    if rel in (os.curdir, '') or rel == os.pardir or rel.startswith(os.pardir + os.sep):
        return None
    if os.path.normcase(os.path.join(root, rel)) != os.path.normcase(path):
        return None
    return rel


def _links_inside(folder: str) -> str | None:
    """The first link or junction anywhere under `folder`, never followed;
    None when there is none."""
    pending = [folder]
    while pending:
        with os.scandir(pending.pop()) as entries:
            for entry in entries:
                if entry.is_symlink() or os.path.isjunction(entry.path):
                    return entry.path
                if entry.is_dir(follow_symlinks=False):
                    pending.append(entry.path)
    return None


def plan_delete(folder: str | os.PathLike, root: str | os.PathLike | None = None,
                open_files: Iterable[str | os.PathLike] = (),
                not_live: bool = False) -> RunDelete:
    """What deleting the run in `folder` would remove, after every check
    `delete_run` makes; nothing is deleted.

    Parameters
    ----------
    folder : str or os.PathLike
        The run folder, or its `run.json`.
    root : str or os.PathLike, optional
        The data folder; `data_root()` when omitted.
    open_files : iterable of path-like
        Files open now (the open project's file, the files its arrays are
        mapped from): a run holding one is not deleted.
    not_live : bool
        True when the caller has established, from the control service, that
        no live run is recording into `folder`: only then is a run whose
        `run.json` does not say it is complete (one that did not end
        cleanly) deleted. False, the default, refuses it, as a run that may
        still be going.

    Returns
    -------
    RunDelete
        The folder and its project file.

    Raises
    ------
    RunDeleteRefused
        `not_found` when there is no such folder; `outside_data_folder`
        when it is not inside the data folder (the data folder itself
        included); `link` when the folder, or a folder on the way to it
        from the data folder, is a link or a junction; `not_a_run_folder`
        when it holds no `run.json` of the run folder schema;
        `link_inside` when anything inside it is a link or a junction;
        `incomplete` when its `run.json` does not say the run is complete
        (the run is still going, or did not end cleanly) and `not_live` is
        not true; `open` when a file inside it is open.
    """
    named = os.path.abspath(os.path.expanduser(os.fspath(folder)))
    if os.path.basename(named) == run_folder.MANIFEST and not os.path.isdir(named):
        named = os.path.dirname(named)
    root = os.path.abspath(os.fspath(root) if root is not None else data_root())
    real_root = os.path.realpath(root)
    # inside the data folder as named, or as the links on the way to it resolve
    rel = _inside(named, root)
    if rel is None:
        rel = _inside(named, real_root)
    if rel is None:
        raise RunDeleteRefused(
            'outside_data_folder',
            f'{named} is not inside the data folder {root}; only a run in the data '
            'folder is deleted here')
    # walk down from the data folder: no step may be a link out of it
    here = real_root
    for part in rel.split(os.sep):
        here = os.path.join(here, part)
        if _is_link(here):
            raise RunDeleteRefused(
                'link', f'{here} is a link; a link is never followed out of the data '
                'folder, so the run is not deleted')
    if not os.path.isdir(here):
        raise RunDeleteRefused('not_found', f'there is no folder at {named}')
    # the data folder's real place cannot have changed between the two looks
    if _inside(os.path.realpath(here), real_root) is None:
        raise RunDeleteRefused('outside_data_folder',
                               f'{named} resolves outside the data folder {root}')
    manifest_path = os.path.join(here, run_folder.MANIFEST)
    try:
        with open(manifest_path, encoding='utf-8') as f:
            manifest = json.load(f)
        schema = str(manifest.get('schema', ''))
    except (OSError, ValueError, AttributeError):
        manifest, schema = None, ''
    name, _, number = schema.partition('/')
    if manifest is None or name != run_folder.SCHEMA_NAME or not number.isdigit():
        raise RunDeleteRefused(
            'not_a_run_folder',
            f'{named} is not a run folder: it holds no {run_folder.MANIFEST} of the run '
            f'folder schema ({run_folder.SCHEMA_NAME}/<number>)')
    inner = _links_inside(here)
    if inner is not None:
        raise RunDeleteRefused(
            'link_inside', f'{inner} is a link; a run folder holds none, so this one is '
            'not deleted')
    complete = manifest.get('complete') is True
    if not complete and not_live is not True:
        raise RunDeleteRefused(
            'incomplete',
            f'the run in {named} is not complete: its {run_folder.MANIFEST} does not say it '
            'ended cleanly, and it has not been established that no live run is recording '
            'into it, so it is not deleted; a run that did not end cleanly is deleted once '
            'the control service reports that it is not live')
    for path in open_files:
        held = os.path.realpath(os.fspath(path))
        if held == here or _inside(held, here) is not None:
            raise RunDeleteRefused(
                'open', f'{held} is open; open another project first, then delete the run')
    project_file = os.path.join(here, os.path.basename(here) + '.hforge')
    return RunDelete(here, project_file, os.path.isfile(project_file), complete)


def delete_run(folder: str | os.PathLike, root: str | os.PathLike | None = None,
               open_files: Iterable[str | os.PathLike] = (),
               not_live: bool = False) -> RunDelete:
    """Delete the run in `folder`: its folder, and the project file it was
    converted to inside it, after the checks of `plan_delete`.

    Parameters
    ----------
    folder, root, open_files, not_live
        As `plan_delete` takes them.

    Returns
    -------
    RunDelete
        What was deleted.

    Raises
    ------
    RunDeleteRefused
        As `plan_delete` refuses; nothing is deleted then.
    OSError
        When the operating system refuses part of the delete; what it
        could not delete is left.
    """
    plan = plan_delete(folder, root, open_files, not_live)
    if plan.project_file_exists:
        os.remove(plan.project_file)
    shutil.rmtree(plan.folder)
    return plan
