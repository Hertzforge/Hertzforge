"""Names the sds/1 protocol fixes (`schema/PROTOCOL.md`)."""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _version

PROTOCOL = 'sds/1'
SERVICE = 'analysis'

try:
    VERSION = _version('hertzforge')
except PackageNotFoundError:            # pragma: no cover - source tree only
    VERSION = '0.0.0'

#: the error codes a response may carry
ERROR_CODES = ('bad_request', 'unknown_method', 'not_found', 'refused',
               'internal')


class ProtocolError(Exception):
    """A request answered with an error response. `message` names the cause
    in words a user can act on."""

    def __init__(self, code: str, message: str) -> None:
        if code not in ERROR_CODES:
            raise ValueError(f'unknown error code {code!r}')
        if not message:
            raise ValueError('an error must say what went wrong')
        super().__init__(message)
        self.code: str = code
        self.message: str = message


def bad_request(message: str) -> ProtocolError:
    """An error for a request whose method exists but whose params are
    wrong."""
    return ProtocolError('bad_request', message)


def not_found(message: str) -> ProtocolError:
    """An error for a dataset, handle, channel or operation that does not
    exist."""
    return ProtocolError('not_found', message)


def refused(message: str) -> ProtocolError:
    """An error for a well-formed request the service will not carry out,
    such as an operation run on the wrong kind of dataset."""
    return ProtocolError('refused', message)


def hello() -> dict:
    """The `hello` message the server sends first on every connection."""
    return {'type': 'hello', 'protocol': PROTOCOL, 'service': SERVICE,
            'version': VERSION}
