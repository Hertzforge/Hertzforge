"""The operating system's open and save dialogs, shown by the service itself.

A browser never hands a page the path of a file it picks, only its contents, and the
service reads a file where it is. `hertzforge` with no command serves the interface to a
browser on the same machine, so the service can show the system's own dialog there and
answer with the paths chosen (`files.choose`): AppleScript's on macOS, Windows Forms
through PowerShell on Windows, and zenity or kdialog on Linux. Where none can be shown
(no desktop session, over SSH, no dialog program), `DialogUnavailable` says so and the
shell asks for a typed path instead.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from collections.abc import Callable

#: what `project.open` reads as a whole project, as the shell's `PROJECT_SUFFIXES`; a
#: run folder's `run.json` opens too
PROJECT_TYPES = ('hforge', 'vdyn', 'mat', 'h5', 'hdf5', 'escdf', 'json')

PURPOSES = ('open', 'import', 'save')

TITLES = {'open': 'Open Project', 'import': 'Import Files', 'save': 'Save Project'}


class DialogUnavailable(Exception):
    """No dialog can be shown where the service runs; the message says why."""


def _applescript_text(text: str) -> str:
    return '"' + text.replace('\\', '\\\\').replace('"', '\\"') + '"'


def _powershell_text(text: str) -> str:
    return "'" + text.replace("'", "''") + "'"


def command(purpose: str, name: str = '', platform: str = sys.platform,
            which: Callable[[str], str | None] = shutil.which,
            environ: dict[str, str] | None = None) -> list[str]:
    """The program and arguments that show the dialog for `purpose` ('open' one
    project, 'import' several files, 'save' a project as `name`) and print each path
    chosen on a line of its own. `DialogUnavailable` where none can be shown."""
    if purpose not in PURPOSES:
        raise ValueError(f'a dialog is for one of {", ".join(PURPOSES)}, not {purpose!r}')
    title = TITLES[purpose]
    env = os.environ if environ is None else environ
    if platform == 'darwin':
        if which('osascript') is None:
            raise DialogUnavailable('osascript is not on this Mac')
        prompt = _applescript_text(title)
        if purpose == 'open':
            types = ', '.join(_applescript_text(t) for t in PROJECT_TYPES)
            script = [f'POSIX path of (choose file with prompt {prompt} of type {{{types}}})']
        elif purpose == 'import':
            script = [f'set picked to choose file with prompt {prompt} with multiple selections allowed',
                      'set out to ""',
                      'repeat with one in picked',
                      'set out to out & POSIX path of one & linefeed',
                      'end repeat',
                      'out']
        else:
            default = _applescript_text(name or 'Project.hforge')
            script = [f'POSIX path of (choose file name with prompt {prompt} default name {default})']
        # the dialog comes up in front of the browser, not behind it
        args = ['osascript', '-e', 'activate']
        for line in script:
            args += ['-e', line]
        return args
    if platform == 'win32':
        shell = which('powershell') or which('pwsh')
        if shell is None:
            raise DialogUnavailable('PowerShell is not on this computer')
        if purpose == 'save':
            dialog = ('$d = New-Object System.Windows.Forms.SaveFileDialog; '
                      f'$d.FileName = {_powershell_text(name or "Project.hforge")}; '
                      "$d.Filter = 'Hertzforge project|*.hforge|All files|*.*'")
        else:
            patterns = ';'.join(f'*.{t}' for t in PROJECT_TYPES)
            kinds = (f"'Projects|{patterns}|All files|*.*'" if purpose == 'open'
                     else "'All files|*.*'")
            dialog = ('$d = New-Object System.Windows.Forms.OpenFileDialog; '
                      f'$d.Filter = {kinds}; '
                      f'$d.Multiselect = ${"true" if purpose == "import" else "false"}')
        script = ('Add-Type -AssemblyName System.Windows.Forms; '
                  f'{dialog}; $d.Title = {_powershell_text(title)}; '
                  # an owner kept on top brings the dialog in front of the browser
                  '$o = New-Object System.Windows.Forms.Form -Property @{TopMost = $true}; '
                  "if ($d.ShowDialog($o) -eq 'OK') { $d.FileNames }")
        return [shell, '-NoProfile', '-STA', '-Command', script]
    if not (env.get('DISPLAY') or env.get('WAYLAND_DISPLAY')):
        raise DialogUnavailable('there is no desktop session here to show a dialog in')
    if which('zenity'):
        args = ['zenity', '--file-selection', f'--title={title}']
        if purpose == 'open':
            args.append('--file-filter=Projects | ' + ' '.join(f'*.{t}' for t in PROJECT_TYPES))
        elif purpose == 'import':
            args += ['--multiple', '--separator=\n']
        else:
            args += ['--save', f'--filename={name or "Project.hforge"}']
        return args
    if which('kdialog'):
        if purpose == 'open':
            return ['kdialog', '--title', title, '--getopenfilename', '.',
                    ' '.join(f'*.{t}' for t in PROJECT_TYPES)]
        if purpose == 'import':
            return ['kdialog', '--title', title, '--getopenfilename', '.', '--multiple',
                    '--separate-output']
        return ['kdialog', '--title', title, '--getsavefilename', name or 'Project.hforge']
    raise DialogUnavailable('neither zenity nor kdialog is installed to show a dialog')


def paths_from(output: str) -> list[str]:
    """The paths a dialog printed, one a line; blank lines left out."""
    return [line.strip() for line in output.splitlines() if line.strip()]


def choose(purpose: str, name: str = '',
           run: Callable[..., subprocess.CompletedProcess] = subprocess.run,
           **where: object) -> list[str]:
    """Show the dialog for `purpose` and wait for it: the paths chosen, or none when
    it was cancelled. `where` reaches `command` (the platform, `which`, the
    environment). `DialogUnavailable` when no dialog can be shown, or it fails."""
    args = command(purpose, name, **where)  # type: ignore[arg-type]
    try:
        done = run(args, capture_output=True, text=True)
    except OSError as error:
        raise DialogUnavailable(f'the dialog could not start: {error}') from None
    if done.returncode == 0:
        return paths_from(done.stdout)
    # every one of them exits 1 when the person cancels; AppleScript also says -128
    if done.returncode == 1 and (args[0] != 'osascript' or '-128' in done.stderr):
        return []
    raise DialogUnavailable(f'the dialog failed: {(done.stderr or "").strip() or done.returncode}')

