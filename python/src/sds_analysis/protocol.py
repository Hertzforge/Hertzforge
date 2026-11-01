"""Names the sds/1 protocol fixes (`schema/PROTOCOL.md`)."""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _version

PROTOCOL = 'sds/1'
SERVICE = 'analysis'

try:
    VERSION = _version('sds-analysis')
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
        self.code = code
        self.message = message


def bad_request(message: str) -> ProtocolError:
    return ProtocolError('bad_request', message)


def not_found(message: str) -> ProtocolError:
    return ProtocolError('not_found', message)


def refused(message: str) -> ProtocolError:
    return ProtocolError('refused', message)


def hello() -> dict:
    return {'type': 'hello', 'protocol': PROTOCOL, 'service': SERVICE,
            'version': VERSION}
