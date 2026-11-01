"""The sds/1 frame codec: messages to WebSocket frames and back.

A message that carries no arrays is a text frame, one UTF-8 JSON object.
A message that carries arrays is a binary frame (`schema/PROTOCOL.md`,
"Frames"):

    b'SDS1' | u32 header length H | JSON header, space-padded so 8 + H is a
    multiple of 8 | payload, each array starting at a multiple of 8

The header is the message plus a top-level "arrays" list; inside the
message each array is the reference {"$array": id}. Encoding is
byte-for-byte what `schema/make_golden.py` writes, so the golden frames
are this codec's test.

Two entry points each way:

- `encode(message, arrays)` frames a message already holding references,
  with the arrays given by id in payload order (the golden generator's
  form).
- `pack(message)` takes a message holding numpy arrays in place, replaces
  each with a reference named a0, a1, ... in traversal order, and frames
  it (text when nothing was replaced).
- `decode(frame)` reads either kind of frame and returns the message with
  every reference replaced by a read-only numpy array.
- `to_wire(message)` returns the message with references in place and the
  arrays split out, which is the form the JSON schema validates.
"""

from __future__ import annotations

import json
import struct
from collections.abc import Mapping
from typing import Any

import numpy as np

MAGIC = b'SDS1'
PREFIX = 8                      # magic plus the u32 header length
ALIGN = 8

#: protocol dtype name -> numpy dtype, little-endian
DTYPES: dict[str, np.dtype] = {
    'f32': np.dtype('<f4'),
    'f64': np.dtype('<f8'),
    'i32': np.dtype('<i4'),
    'u32': np.dtype('<u4'),
    'u8': np.dtype('u1'),
}
_NAMES = {(dt.kind, dt.itemsize): name for name, dt in DTYPES.items()}


class FrameError(ValueError):
    """A frame or message that cannot be read or written as sds/1. The
    message names what is wrong with it."""


def dtype_name(array: np.ndarray) -> str:
    """The protocol name of an array's dtype; refused when the protocol has
    no such dtype (convert explicitly rather than have the codec guess)."""
    name = _NAMES.get((array.dtype.kind, array.dtype.itemsize))
    if name is None:
        raise FrameError(
            f'dtype {array.dtype} has no sds/1 equivalent; the protocol '
            f'carries {", ".join(DTYPES)} only (complex data travels as '
            'two real arrays)')
    return name


def _plain(value: Any) -> Any:
    """A numpy scalar as the Python number JSON can write."""
    if isinstance(value, np.generic):
        return value.item()
    raise TypeError(f'{type(value).__name__} is not JSON serializable')


def _json(message: Mapping[str, Any]) -> bytes:
    # compact separators and ensure_ascii=False, as make_golden.py; NaN and
    # infinity are refused because JSON has no spelling for them
    try:
        return json.dumps(message, ensure_ascii=False, separators=(',', ':'),
                          allow_nan=False, default=_plain).encode('utf-8')
    except ValueError as error:
        raise FrameError(f'message holds a value JSON cannot carry: {error}'
                         ) from None


def encode_text(message: Mapping[str, Any]) -> str:
    """A message with no arrays as the text of one text frame."""
    return _json(message).decode('utf-8')


def encode(message: Mapping[str, Any],
           arrays: Mapping[str, np.ndarray]) -> bytes:
    """Frame `message`, whose array values are {"$array": id} references,
    with `arrays` (id -> array) written in the order given."""
    payload = bytearray()
    entries = []
    for ident, array in arrays.items():
        array = np.asarray(array)
        name = dtype_name(array)
        data = np.ascontiguousarray(array, dtype=DTYPES[name]).tobytes()
        payload += b'\0' * (-len(payload) % ALIGN)
        entries.append({'id': ident, 'dtype': name,
                        'shape': [int(n) for n in array.shape],
                        'offset': len(payload), 'nbytes': len(data)})
        payload += data
    if 'arrays' in message:
        raise FrameError('a message may not set "arrays" itself; the codec '
                         'writes that list')
    header = _json({**message, 'arrays': entries})
    header += b' ' * (-(PREFIX + len(header)) % ALIGN)
    return MAGIC + struct.pack('<I', len(header)) + header + bytes(payload)


def to_wire(message: Mapping[str, Any]) -> tuple[dict[str, Any],
                                                 dict[str, np.ndarray]]:
    """(message with references, {id: array}): every numpy array in the
    message replaced by {"$array": "a<k>"}, k counting in traversal order
    (dict insertion order, list order)."""
    arrays: dict[str, np.ndarray] = {}

    def walk(node: Any) -> Any:
        if isinstance(node, np.ndarray):
            ident = f'a{len(arrays)}'
            arrays[ident] = node
            return {'$array': ident}
        if isinstance(node, Mapping):
            return {str(key): walk(value) for key, value in node.items()}
        if isinstance(node, (list, tuple)):
            return [walk(item) for item in node]
        return node

    return walk(message), arrays


def pack(message: Mapping[str, Any]) -> str | bytes:
    """A message holding numpy arrays in place as one frame: text when it
    holds none, binary otherwise."""
    wire, arrays = to_wire(message)
    if not arrays:
        return encode_text(wire)
    return encode(wire, arrays)


def decode(frame: str | bytes | bytearray | memoryview) -> dict[str, Any]:
    """Read one frame. Text is a JSON object with no array references;
    binary is checked against the layout and every reference resolved to a
    read-only numpy array (C order, shape as declared)."""
    if isinstance(frame, str):
        return _decode_text(frame)
    return _decode_binary(bytes(frame))


def _decode_text(text: str) -> dict[str, Any]:
    try:
        message = json.loads(text)
    except ValueError as error:
        raise FrameError(f'text frame is not JSON: {error}') from None
    if not isinstance(message, dict):
        raise FrameError('a text frame must hold one JSON object, not '
                         f'{type(message).__name__}')
    if _has_reference(message):
        raise FrameError('a text frame carries no arrays, but this one holds '
                         'an {"$array": ...} reference; arrays travel in '
                         'binary frames')
    return message


def _has_reference(node: Any) -> bool:
    if isinstance(node, dict):
        return _is_reference(node) or any(_has_reference(v)
                                          for v in node.values())
    if isinstance(node, list):
        return any(_has_reference(item) for item in node)
    return False


def _is_reference(node: Any) -> bool:
    return isinstance(node, dict) and set(node) == {'$array'}


def _decode_binary(frame: bytes) -> dict[str, Any]:
    if len(frame) < PREFIX or frame[:4] != MAGIC:
        raise FrameError('binary frame does not start with the magic '
                         f'{MAGIC.decode()}')
    (length,) = struct.unpack_from('<I', frame, 4)
    start = PREFIX + length
    if start > len(frame):
        raise FrameError(f'header length {length} runs past the end of a '
                         f'{len(frame)}-byte frame')
    if start % ALIGN:
        raise FrameError(f'header length {length} leaves the payload at byte '
                         f'{start}, not a multiple of {ALIGN}')
    try:
        message = json.loads(frame[PREFIX:start].decode('utf-8'))
    except (UnicodeDecodeError, ValueError) as error:
        raise FrameError(f'binary frame header is not UTF-8 JSON: {error}'
                         ) from None
    if not isinstance(message, dict):
        raise FrameError('binary frame header must be one JSON object')
    entries = message.pop('arrays', None)
    if not isinstance(entries, list):
        raise FrameError('binary frame header has no "arrays" list')
    payload = memoryview(frame)[start:]
    arrays = {}
    for entry in entries:
        array = _read_array(entry, payload)
        if entry['id'] in arrays:
            raise FrameError(f'array id {entry["id"]!r} is listed twice')
        arrays[entry['id']] = array
    used: set[str] = set()

    def walk(node: Any) -> Any:
        if _is_reference(node):
            ident = node['$array']
            if ident not in arrays:
                raise FrameError(f'reference to array {ident!r}, which the '
                                 'header does not list')
            if ident in used:
                raise FrameError(f'array {ident!r} is referenced twice; each '
                                 'listed array is referenced exactly once')
            used.add(ident)
            return arrays[ident]
        if isinstance(node, dict):
            return {key: walk(value) for key, value in node.items()}
        if isinstance(node, list):
            return [walk(item) for item in node]
        return node

    message = walk(message)
    unused = sorted(set(arrays) - used)
    if unused:
        raise FrameError(f'array(s) {", ".join(unused)} listed but never '
                         'referenced')
    return message


def _read_array(entry: Any, payload: memoryview) -> np.ndarray:
    try:
        ident, name, shape = entry['id'], entry['dtype'], entry['shape']
        offset, nbytes = int(entry['offset']), int(entry['nbytes'])
    except (KeyError, TypeError, ValueError):
        raise FrameError(f'array entry {entry!r} lacks one of id, dtype, '
                         'shape, offset, nbytes') from None
    if name not in DTYPES:
        raise FrameError(f'array {ident!r} has dtype {name!r}; sds/1 carries '
                         f'{", ".join(DTYPES)}')
    if (not isinstance(shape, list)
            or not all(isinstance(n, int) and n >= 0 for n in shape)):
        raise FrameError(f'array {ident!r} shape {shape!r} is not a list of '
                         'non-negative integers')
    dtype = DTYPES[name]
    count = int(np.prod(shape, dtype=np.int64))
    if offset < 0 or offset % ALIGN:
        raise FrameError(f'array {ident!r} offset {offset} is not a '
                         f'non-negative multiple of {ALIGN}')
    if nbytes != count * dtype.itemsize:
        raise FrameError(f'array {ident!r} declares {nbytes} bytes; shape '
                         f'{shape} of {name} needs {count * dtype.itemsize}')
    if offset + nbytes > len(payload):
        raise FrameError(f'array {ident!r} runs to byte {offset + nbytes} of '
                         f'a {len(payload)}-byte payload')
    return np.frombuffer(payload, dtype=dtype, count=count,
                         offset=offset).reshape(shape)
