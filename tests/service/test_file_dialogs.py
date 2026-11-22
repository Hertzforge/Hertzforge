"""`files.choose`: the system's open and save dialogs, shown by the service for a browser
on its own machine (`service.dialogs`). The dialogs themselves cannot run here; what each
platform is asked to run, how its answer is read, and who may ask are tested."""

from __future__ import annotations

import asyncio
import subprocess

import pytest
from websockets.asyncio.client import connect

from hertzforge.service import dialogs, frames
from hertzforge.service.server import Service, start

ALL = {'osascript': '/usr/bin/osascript', 'powershell': 'C:/ps.exe', 'zenity': '/usr/bin/zenity',
       'kdialog': '/usr/bin/kdialog'}


def which(*present):
    return lambda name: ALL[name] if name in present else None


def test_macos_asks_for_one_project_several_files_or_where_to_save():
    opened = dialogs.command('open', platform='darwin', which=which('osascript'))
    assert opened[:3] == ['osascript', '-e', 'activate']
    assert 'of type {"hforge", "vdyn", "mat", "h5", "hdf5", "escdf", "json"}' in opened[-1]
    imported = ' '.join(dialogs.command('import', platform='darwin', which=which('osascript')))
    assert 'with multiple selections allowed' in imported and 'POSIX path of one' in imported
    saved = dialogs.command('save', 'My "Plate".hforge', platform='darwin', which=which('osascript'))
    assert saved[-1].endswith('default name "My \\"Plate\\".hforge")')


def test_windows_shows_the_forms_dialogs_on_a_single_threaded_apartment():
    args = dialogs.command('import', platform='win32', which=which('powershell'))
    assert args[:4] == ['C:/ps.exe', '-NoProfile', '-STA', '-Command']
    assert '$d.Multiselect = $true' in args[4] and 'OpenFileDialog' in args[4]
    saved = dialogs.command('save', "Kevin's.hforge", platform='win32', which=which('powershell'))[4]
    assert "SaveFileDialog" in saved and "$d.FileName = 'Kevin''s.hforge'" in saved


def test_linux_takes_zenity_then_kdialog_and_needs_a_desktop():
    desktop = {'DISPLAY': ':0'}
    args = dialogs.command('import', platform='linux', which=which('zenity', 'kdialog'), environ=desktop)
    assert args[:2] == ['zenity', '--file-selection'] and '--multiple' in args
    args = dialogs.command('save', 'p.hforge', platform='linux', which=which('kdialog'), environ=desktop)
    assert args == ['kdialog', '--title', 'Save Project', '--getsavefilename', 'p.hforge']
    with pytest.raises(dialogs.DialogUnavailable, match='no desktop session'):
        dialogs.command('open', platform='linux', which=which('zenity'), environ={})
    with pytest.raises(dialogs.DialogUnavailable, match='neither zenity nor kdialog'):
        dialogs.command('open', platform='linux', which=which(), environ=desktop)


def _ran(code, out='', err=''):
    return lambda args, **_kw: subprocess.CompletedProcess(args, code, out, err)


def test_the_paths_chosen_come_back_one_a_line_and_a_cancel_as_none():
    mac = {'platform': 'darwin', 'which': which('osascript')}
    assert dialogs.choose('import', run=_ran(0, '/a/one.uff\n/a/two.csv\n\n'), **mac) == [
        '/a/one.uff', '/a/two.csv']
    assert dialogs.choose('open', run=_ran(1, err='execution error: User canceled. (-128)'), **mac) == []
    with pytest.raises(dialogs.DialogUnavailable, match='Not authorized'):
        dialogs.choose('open', run=_ran(1, err='Not authorized to send Apple events'), **mac)
    linux = {'platform': 'linux', 'which': which('zenity'), 'environ': {'DISPLAY': ':0'}}
    assert dialogs.choose('open', run=_ran(1), **linux) == []

    def missing(args, **_kw):
        raise FileNotFoundError(args[0])

    with pytest.raises(dialogs.DialogUnavailable, match='could not start'):
        dialogs.choose('open', run=missing, **linux)


async def _ask(service, *params):
    server = await start(service, port=0)
    port = server.sockets[0].getsockname()[1]
    out = []
    try:
        async with connect(f'ws://127.0.0.1:{port}') as socket:
            await socket.recv()
            for ident, one in enumerate(params, 1):
                await socket.send(frames.encode_text({'type': 'request', 'id': ident,
                                                      'method': 'files.choose', 'params': one}))
                out.append(frames.decode(await socket.recv()))
    finally:
        server.close()
        await server.wait_closed()
        service.close()
    return out


def test_only_a_service_started_for_a_browser_here_shows_them(monkeypatch):
    asked = []
    monkeypatch.setattr(dialogs, 'choose', lambda purpose, name: asked.append((purpose, name))
                        or ['/d/Plate.hforge'])
    served = asyncio.run(_ask(Service(), {'purpose': 'open'}))
    assert served[0]['error']['code'] == 'refused'
    assert 'type the path instead' in served[0]['error']['message'] and not asked
    app = asyncio.run(_ask(Service(dialogs=True), {'purpose': 'save', 'name': 'Plate.hforge'},
                           {'purpose': 'print'}))
    assert app[0]['result'] == {'paths': ['/d/Plate.hforge']}
    assert asked == [('save', 'Plate.hforge')]
    assert app[1]['error']['code'] == 'bad_request'


def test_a_dialog_that_cannot_be_shown_is_refused_with_the_reason(monkeypatch):
    def unavailable(_purpose, _name):
        raise dialogs.DialogUnavailable('there is no desktop session here to show a dialog in')

    monkeypatch.setattr(dialogs, 'choose', unavailable)
    answer = asyncio.run(_ask(Service(dialogs=True), {'purpose': 'import'}))[0]
    assert answer['error']['code'] == 'refused'
    assert answer['error']['message'] == ('No file dialog could be shown: there is no desktop '
                                          'session here to show a dialog in. Type the path instead.')
