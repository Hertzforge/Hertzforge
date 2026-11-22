"""Files a browser sends by their contents, for `project.import` and `project.open`.

A browser gives the shell a dropped file's contents and its name, never its path, so the shell
sends the contents in pieces (`project.upload`) and then names the upload in place of a path.
Each upload is written to a folder of this connection's own, under the file's own name so the
readers recognize its format and their messages name it, and stays there until the connection
closes: a project opened from one maps its large arrays from that file.
"""

from __future__ import annotations

import os
import shutil
import tempfile
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

import numpy as np

from .protocol import bad_request, refused

#: the start of the name of each connection's folder of received files, followed by the
#: service's process id
PREFIX = 'hertzforge-received-'

#: the most a piece may hold, under the service's frame limit (`server.MAX_INCOMING`) with room
#: for the header
CHUNK_LIMIT = 8 * 2**20


@dataclass
class Upload:
    """One file being received: its name, the size it was announced at, where it is written and
    how much of it has come."""

    name: str
    size: int
    path: str
    received: int = 0

    @property
    def complete(self) -> bool:
        """Whether every byte announced has come."""
        return self.received == self.size


class Uploads:
    """The files one connection has been sent, by the id `project.upload` answered."""

    def __init__(self) -> None:
        self.root: str | None = None
        self.held: dict[str, Upload] = {}

    def receive(self, params: Mapping[str, Any]) -> dict[str, Any]:
        """`project.upload`: one piece of a file, `data` (u8) at `offset`. The first piece, at 0
        and with no `upload`, starts the file and names it (`name`, the file's own name) and its
        `size` in bytes; each later piece names the upload the first answered and comes at the
        offset received so far. Answers the upload's id and the bytes received."""
        data = params.get('data')
        if not isinstance(data, np.ndarray) or data.dtype != np.uint8 or data.ndim != 1:
            raise bad_request('project.upload "data" is the piece of the file, a u8 array')
        if data.size > CHUNK_LIMIT:
            raise bad_request(f'project.upload sends a file in pieces of at most '
                              f'{CHUNK_LIMIT} bytes, not {data.size}')
        offset = params.get('offset')
        if isinstance(offset, bool) or not isinstance(offset, int):
            raise bad_request('project.upload "offset" is where the piece starts in the file, '
                              'in bytes')
        ident = params.get('upload')
        if ident is None:
            upload = self._start(params)
        elif not isinstance(ident, str) or ident not in self.held:
            raise bad_request(f'project.upload: no upload {ident!r} is under way; start one '
                              'with the first piece, at offset 0')
        else:
            upload = self.held[ident]
        if offset != upload.received:
            raise bad_request(f'{upload.name}: {upload.received} bytes have come, so the next '
                              f'piece starts there, not at {offset}')
        if upload.received + data.size > upload.size:
            raise bad_request(f'{upload.name} was announced as {upload.size} bytes; this piece '
                              'runs past its end')
        with open(upload.path, 'ab') as out:
            out.write(data.tobytes())
        upload.received += data.size
        ident = next(key for key, held in self.held.items() if held is upload)
        return {'upload': ident, 'received': upload.received}

    def _start(self, params: Mapping[str, Any]) -> Upload:
        name, size = params.get('name'), params.get('size')
        if not isinstance(name, str) or not name.strip():
            raise bad_request('project.upload starts with "name", the file\'s own name')
        if os.path.basename(name) != name or name in ('.', '..'):
            raise bad_request(f'project.upload "name" is a file\'s name, not a path: {name!r}')
        if isinstance(size, bool) or not isinstance(size, int) or size < 0:
            raise bad_request('project.upload starts with "size", the file\'s length in bytes')
        if params.get('offset') != 0:
            raise bad_request(f'{name}: the first piece starts at offset 0')
        if self.root is None:
            sweep()
            self.root = tempfile.mkdtemp(prefix=f'{PREFIX}{os.getpid()}-')
        # a folder for each, so two files of one name are two uploads
        folder = tempfile.mkdtemp(dir=self.root)
        ident = os.path.basename(folder)
        upload = Upload(name, size, os.path.join(folder, name))
        open(upload.path, 'wb').close()
        self.held[ident] = upload
        return upload

    def taken(self, ident: Any) -> Upload:
        """The upload `project.import` or `project.open` names, refused until all of it has
        come."""
        if not isinstance(ident, str) or ident not in self.held:
            raise bad_request(f'no upload {ident!r} was received on this connection')
        upload = self.held[ident]
        if not upload.complete:
            raise refused(f'{upload.name}: {upload.received} of {upload.size} bytes have come; '
                          'send the rest first')
        return upload

    def close(self) -> None:
        """The connection closed: every file it was sent goes."""
        if self.root is not None:
            shutil.rmtree(self.root, ignore_errors=True)
        self.root = None
        self.held.clear()


def sweep(folder: str | None = None) -> list[str]:
    """Remove the folders of received files that a service no longer running left behind, as
    one stopped before its connections closed does, and answer them. A folder is a service's
    while the process its name carries is running."""
    folder = folder or tempfile.gettempdir()
    gone = []
    for name in os.listdir(folder):
        pid = name[len(PREFIX):].split('-', 1)[0] if name.startswith(PREFIX) else ''
        if pid.isdigit() and not _running(int(pid)):
            shutil.rmtree(os.path.join(folder, name), ignore_errors=True)
            gone.append(name)
    return gone


def _running(pid: int) -> bool:
    if pid == os.getpid():
        return True
    if os.name == 'nt':
        return _running_on_windows(pid)
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except (PermissionError, OSError):
        # another user's, or one that cannot be asked: left alone
        return True
    return True


def _running_on_windows(pid: int) -> bool:
    """Whether process `pid` is running, asked of Windows itself: there `os.kill(pid, 0)` does
    not ask, it ends the process."""
    import ctypes

    kernel = ctypes.windll.kernel32  # type: ignore[attr-defined]
    handle = kernel.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not handle:
        # no such process, or one this user may not ask about (ERROR_ACCESS_DENIED), left alone
        return kernel.GetLastError() == 5
    try:
        code = ctypes.c_ulong()
        if not kernel.GetExitCodeProcess(handle, ctypes.byref(code)):
            return True
        return code.value == 259  # STILL_ACTIVE
    finally:
        kernel.CloseHandle(handle)
