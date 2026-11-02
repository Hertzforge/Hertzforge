"""Operation descriptors: the typed form of the schema's `operation`.

The shell builds an operation's panel from its descriptor, so the
descriptor is also where a request's params are checked: `resolve` fills
defaults and refuses a value the panel could not have sent, naming the
param and what it accepts.
"""

from __future__ import annotations

import math
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Literal

from .protocol import bad_request

ParamType = Literal['number', 'integer', 'enum', 'boolean', 'string']
Workspace = Literal['acquire', 'analyze', 'modal', 'model', 'specify',
                    'simulate', 'report']
DatasetKind = Literal['random_run', 'modal_model']
ViewKind = Literal['spectrum', 'reading', 'shape3d']

#: marks a param with no default: the request must give it
REQUIRED = object()


@dataclass(frozen=True)
class Option:
    value: str | int | float
    label: str

    def to_json(self) -> dict[str, Any]:
        return {'value': self.value, 'label': self.label}


@dataclass(frozen=True)
class Param:
    """One input of an operation's panel."""

    name: str
    label: str
    type: ParamType
    default: Any = REQUIRED
    unit: str | None = None
    min: float | None = None
    max: float | None = None
    step: float | None = None
    options: Sequence[Option] = ()
    #: an enum whose options are the opened dataset's channels
    options_from: Literal['channels'] | None = None

    def to_json(self) -> dict[str, Any]:
        out: dict[str, Any] = {'name': self.name, 'label': self.label,
                               'type': self.type}
        if self.unit is not None:
            out['unit'] = self.unit
        if self.default is not REQUIRED and self.default is not None:
            out['default'] = self.default
        for key in ('min', 'max', 'step'):
            value = getattr(self, key)
            if value is not None:
                out[key] = value
        if self.options:
            out['options'] = [option.to_json() for option in self.options]
        if self.options_from is not None:
            out['options_from'] = self.options_from
        return out

    def check(self, value: Any, choices: Sequence[Any] | None) -> Any:
        """`value` as this param's type, or a bad_request naming why not.
        `choices` are the allowed values of an `options_from` enum."""
        what = f'param "{self.name}" ({self.label})'
        if self.type in ('number', 'integer'):
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise bad_request(f'{what} must be a number, not '
                                  f'{json_type(value)}')
            if not math.isfinite(value):
                raise bad_request(f'{what} must be finite, not {value}')
            if self.type == 'integer':
                if float(value) != int(value):
                    raise bad_request(f'{what} must be a whole number, not '
                                      f'{value}')
                value = int(value)
            else:
                value = float(value)
            if self.min is not None and value < self.min:
                raise bad_request(f'{what} is {value}; the least it takes is '
                                  f'{self.min:g}{_unit(self.unit)}')
            if self.max is not None and value > self.max:
                raise bad_request(f'{what} is {value}; the most it takes is '
                                  f'{self.max:g}{_unit(self.unit)}')
            return value
        if self.type == 'boolean':
            if not isinstance(value, bool):
                raise bad_request(f'{what} must be true or false, not '
                                  f'{json_type(value)}')
            return value
        if self.type == 'string':
            if not isinstance(value, str):
                raise bad_request(f'{what} must be a string, not '
                                  f'{json_type(value)}')
            return value
        allowed = (list(choices) if self.options_from is not None
                   else [option.value for option in self.options])
        if value not in allowed:
            shown = ', '.join(repr(a) for a in allowed[:12])
            more = ', ...' if len(allowed) > 12 else ''
            raise bad_request(f'{what} is {value!r}, which is not one of the '
                              f'choices: {shown}{more}')
        return value


def _unit(unit: str | None) -> str:
    return f' {unit}' if unit else ''


def json_type(value: Any) -> str:
    """How a JSON value's type reads in an error message."""
    if value is None:
        return 'null'
    if isinstance(value, bool):
        return 'a boolean'
    if isinstance(value, (int, float)):
        return f'the number {value}'
    if isinstance(value, str):
        return f'the string {value!r}'
    if isinstance(value, list):
        return 'a list'
    if isinstance(value, dict):
        return 'an object'
    return type(value).__name__


@dataclass(frozen=True)
class Operation:
    """An operation's descriptor, plus the function that runs it.

    `run(dataset, params)` receives the opened dataset and the resolved
    params and returns the result's list of views.
    """

    id: str
    title: str
    workspace: Workspace
    accepts: DatasetKind
    summary: str
    params: Sequence[Param]
    outputs: Sequence[ViewKind]
    run: Callable[[Any, dict[str, Any]], list[dict[str, Any]]] = field(
        repr=False, compare=False)

    def to_json(self) -> dict[str, Any]:
        return {'id': self.id, 'title': self.title,
                'workspace': self.workspace, 'accepts': self.accepts,
                'summary': self.summary,
                'params': [param.to_json() for param in self.params],
                'outputs': list(self.outputs)}

    def resolve(self, given: Any,
                channels: Sequence[str] = ()) -> dict[str, Any]:
        """The params to run with: each given value checked, each missing
        one its default. An `options_from: "channels"` param with no default
        takes the dataset's first channel."""
        if not isinstance(given, Mapping):
            raise bad_request(f'op.run "params" must be an object, not '
                              f'{json_type(given)}')
        known = {param.name: param for param in self.params}
        unknown = sorted(set(given) - set(known))
        if unknown:
            raise bad_request(
                f'{self.id} has no param named {", ".join(map(repr, unknown))}'
                f'; it takes {", ".join(map(repr, known)) or "none"}')
        out = {}
        for param in self.params:
            choices = channels if param.options_from == 'channels' else None
            if param.name in given:
                out[param.name] = param.check(given[param.name], choices)
            elif param.default is not REQUIRED:
                out[param.name] = param.default
            elif choices:
                out[param.name] = choices[0]
            else:
                raise bad_request(f'{self.id} needs param "{param.name}" '
                                  f'({param.label})')
        return out
