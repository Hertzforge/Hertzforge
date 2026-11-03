"""Operation descriptors: the typed form of the schema's `operation`.

The shell builds an operation's panel from its descriptor, so the
descriptor is also where a request's params are checked: `resolve` fills
defaults and refuses a value the panel could not have sent, naming the
param and what it accepts.
"""

from __future__ import annotations

import math
from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Any, Literal

from .protocol import bad_request, refused

ParamType = Literal['number', 'integer', 'enum', 'boolean', 'string']
Workspace = Literal['acquire', 'analyze', 'modal', 'model', 'specify',
                    'simulate', 'report']
DatasetKind = Literal['random_run', 'modal_model', 'modal_run']
#: what an operation runs on: a catalog dataset of one kind, or the
#: connection's own project
Accepts = DatasetKind | Literal['project']
ViewKind = Literal['spectrum', 'reading', 'shape3d', 'waterfall', 'matrix']

#: marks a param with no default: the request must give it
REQUIRED = object()


@dataclass(frozen=True)
class Option:
    """One choice of an enum param: the value sent back and the label shown."""

    value: str | int | float
    label: str

    def to_json(self) -> dict[str, Any]:
        """The option as the descriptor's `options` list carries it."""
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
    #: ('channels'), the open project's objects of some kinds
    #: ('objects:psd' or 'objects:psd|frf', by `projects.kind_of`), or
    #: the objects a core verb applies to ('verb:integrate', by
    #: `Project.verbs`, the table the verbs themselves are offered by)
    options_from: str | None = None
    #: set from the result's view rather than the panel, which does not
    #: show it (a waterfall's quantity, chosen among the groups it drew)
    in_view: bool = False
    #: the setting of the object the operation's `source` param names that
    #: the param opens on, as '<group>.<field>' in the summary's
    #: `settings` ('averaging.frame_length'); the default when the object
    #: has none
    setting: str | None = None

    def __post_init__(self) -> None:
        source = self.options_from
        if source is not None and source != 'channels' and not (
                source.startswith('objects:') and all(self.kinds)
                or source.startswith('verb:') and self.verb):
            raise ValueError(f'options_from {source!r} is "channels", '
                             '"objects:<kind>|<kind>" or "verb:<verb>"')

    @property
    def kinds(self) -> tuple[str, ...]:
        """The object kinds an `objects:` param takes; empty for any other."""
        source = self.options_from or ''
        return (tuple(source.removeprefix('objects:').split('|'))
                if source.startswith('objects:') else ())

    @property
    def verb(self) -> str:
        """The core verb a `verb:` param's objects must take; '' for any
        other."""
        source = self.options_from or ''
        return source.removeprefix('verb:') if source.startswith('verb:') else ''

    def to_json(self) -> dict[str, Any]:
        """The param as the schema's descriptor writes it. Fields left unset
        are omitted rather than sent as null, and so is a required param's
        default."""
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
        if self.in_view:
            out['in_view'] = True
        if self.setting is not None:
            out['setting'] = self.setting
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

    `run(dataset, params)` receives the opened dataset (the connection's
    `Project` when `accepts` is 'project') and the resolved params and
    returns the result's list of views.
    """

    id: str
    title: str
    workspace: Workspace
    accepts: Accepts
    summary: str
    params: Sequence[Param]
    outputs: Sequence[ViewKind]
    run: Callable[[Any, dict[str, Any]], list[dict[str, Any]]] = field(
        repr=False, compare=False)
    #: a project operation that changes an object in place (marks shocks
    #: on a history), so the project has unsaved changes even when no
    #: object was added or removed
    edits: bool = False

    def to_json(self) -> dict[str, Any]:
        """The descriptor `catalog.list` sends for this operation; `run`
        stays behind."""
        return {'id': self.id, 'title': self.title,
                'workspace': self.workspace, 'accepts': self.accepts,
                'summary': self.summary,
                'params': [param.to_json() for param in self.params],
                'outputs': list(self.outputs)}

    def resolve(self, given: Any, channels: Sequence[str] = (),
                objects: Mapping[str, str] | None = None,
                verbs: Mapping[str, Collection[str]] | None = None
                ) -> dict[str, Any]:
        """The params to run with: each given value checked, each missing
        one its default. An `options_from` param with no default takes the
        first of its choices: the dataset's channels, the names in
        `objects` (name to kind, the project's order) of the kinds it
        takes, or the names in `verbs` (name to the verbs that apply to
        it) its verb applies to."""
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
            choices = (channels if param.options_from == 'channels'
                       else [name for name, kind in (objects or {}).items()
                             if kind in param.kinds] if param.kinds
                       else [name for name, applies in (verbs or {}).items()
                             if param.verb in applies] if param.verb
                       else None)
            if param.name in given:
                out[param.name] = param.check(given[param.name], choices)
            elif param.default is not REQUIRED:
                out[param.name] = param.default
            elif choices:
                out[param.name] = choices[0]
            elif param.kinds:
                kinds = ' or '.join(kind.replace('_', ' ') for kind in param.kinds)
                raise refused(f'{self.title} needs a {kinds} ({param.label}), '
                              'and the project has none')
            elif param.verb:
                raise refused(f'{self.title} applies to none of the project\'s '
                              f'objects ({param.label})')
            else:
                raise bad_request(f'{self.id} needs param "{param.name}" '
                                  f'({param.label})')
        return out
