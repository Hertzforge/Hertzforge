"""A test's setup, held in a project: what one control module is
configured with, saved so it can be opened again.

The control service is configured by one request per test module
(`random.configure`, `sine.configure`, `mimo_random.setup`, ...), whose
configuration is the whole setup: the profile or target, the schedule,
the limits, the control settings and the operator's words. A
`TestSetup` keeps that configuration exactly as the service accepts it,
with the module it is for, the protocol it was written under, the
channel table it was made with, a title and notes, and where it came
from. It is saved in the project file as the `test_setup` kind
(`docs/hforge-format.md`).

A run's record holds the same configuration (`RunRecord.config`), so a
setup is made from a run with `TestSetup.from_run`, and opening a
setup from a run and opening a saved one are the same act.

Opening a setup never configures anything here. The shell fills a
test's draft with it, and the draft goes through the same configure
call and the same checks as one typed in.
"""

from __future__ import annotations

import copy
from collections.abc import Mapping
from typing import Any

#: the test modules a setup can be for, by the name a run's manifest
#: gives its test (`RunRecord.test`)
MODULES = ('random', 'sine', 'mixed', 'classical_shock', 'srs', 'twr', 'explorer',
           'recorder', 'mimo_random', 'mimo_shock', 'playback')

#: each module's configure request on the control service, and the member
#: of its params that holds the configuration
CONFIGURE = {'random': ('random.configure', 'config'),
             'sine': ('sine.configure', 'config'),
             'mixed': ('mixed.configure', 'config'),
             'classical_shock': ('shock.configure', 'config'),
             'srs': ('srs.configure', 'config'),
             'twr': ('twr.configure', 'config'),
             'explorer': ('explorer.configure', 'config'),
             'recorder': ('recorder.configure', 'config'),
             'mimo_random': ('mimo_random.setup', 'setup'),
             'mimo_shock': ('mimo_shock.configure', 'config'),
             'playback': ('playback.configure', 'config')}

#: the protocol a setup is written under when it does not say
PROTOCOL = 'sds/1'

#: where a setup came from: typed on a test's screen, opened from a
#: saved setup, taken from a run's record, or seeded from an analysis
#: object in a project
ORIGIN_KINDS = ('typed', 'setup', 'run', 'seed')

#: what each kind of origin must name
_ORIGIN_NEEDS = {'typed': (), 'setup': ('name',), 'run': (), 'seed': ('object', 'checksum')}


def origin(kind: str = 'typed', **members: Any) -> dict[str, Any]:
    """An origin, checked: `kind` and what that kind names.

    Parameters
    ----------
    kind : str
        One of `ORIGIN_KINDS`.
    **members
        For 'setup', `name` (the saved setup's name). For 'run',
        `run_uid` and/or `run_id`, and `name` (the run record's name in
        the project) when there is one. For 'seed', `object` (the
        analysis object's name), `checksum` (its SHA-256, hex) and
        `fills` (what part of the setup it filled), which a seed's
        builder sets.

    Returns
    -------
    dict
        The origin, with members that are None left out.

    Raises
    ------
    ValueError
        For an unknown kind or a missing member.
    """
    if kind not in ORIGIN_KINDS:
        raise ValueError(f'a setup\'s origin is one of {", ".join(ORIGIN_KINDS)}, '
                         f'not {kind!r}')
    out: dict[str, Any] = {'kind': kind}
    out.update({key: value for key, value in members.items() if value is not None})
    missing = [key for key in _ORIGIN_NEEDS[kind] if not out.get(key)]
    if kind == 'run' and not (out.get('run_uid') or out.get('run_id')):
        missing.append('run_uid or run_id')
    if missing:
        raise ValueError(f'a {kind!r} origin names its {" and ".join(missing)}')
    return out


#: `origin`, under a name a parameter does not hide
_checked = origin


class TestSetup:
    """One test module's setup, as the control service accepts it.

    Parameters
    ----------
    module : str
        The test module it is for, one of `MODULES`.
    config : mapping
        The configuration exactly as the module's configure request
        takes it (`CONFIGURE`), parsed from JSON. It is kept whole: a
        member this build knows nothing of is carried, not dropped.
    protocol : str, optional
        The protocol it was written under ('sds/1').
    table : mapping, optional
        The channel table it was made with: `rows` in the control
        service's spelling, its `revision`, and its `sha256` when the
        service gave one. Empty when it was not recorded.
    title : str, optional
        What the setup is called in words.
    notes : str, optional
        The operator's notes on it.
    origin : mapping, optional
        Where it came from (`origin`); typed when not given.
    """

    #: not a test class, whatever its name says
    __test__ = False

    def __init__(self, module: str, config: Mapping[str, Any], protocol: str = PROTOCOL,
                 table: Mapping[str, Any] | None = None, title: str = '', notes: str = '',
                 origin: Mapping[str, Any] | None = None) -> None:
        if module not in MODULES:
            raise ValueError(f'a setup is for one of {", ".join(MODULES)}, not {module!r}')
        if not isinstance(config, Mapping):
            raise TypeError('a setup\'s configuration is a JSON object')
        self.module: str = str(module)
        self.config: dict[str, Any] = copy.deepcopy(dict(config))
        self.protocol: str = str(protocol)
        self.table: dict[str, Any] = copy.deepcopy(dict(table or {}))
        self.title: str = str(title or '')
        self.notes: str = str(notes or '')
        held = dict(origin) if origin is not None else {'kind': 'typed'}
        kind = held.pop('kind', None)
        self.origin: dict[str, Any] = _checked(str(kind), **held)

    @classmethod
    def from_run(cls, record: Any, name: str | None = None) -> TestSetup:
        """The setup a run ran with, from its record (`RunRecord`).

        Parameters
        ----------
        record : RunRecord
            The run's record; its `setup.config` is the configuration
            as the service accepted it.
        name : str, optional
            The record's name in the project, kept in the origin.

        Returns
        -------
        TestSetup
            Its configuration, the table the run armed with, the run's
            title and notes, and the run as its origin.

        Raises
        ------
        ValueError
            When the record names no test this build has a setup for,
            or holds no configuration.
        """
        if record.test not in MODULES:
            raise ValueError(f'the run is a {record.test or "unnamed"!r} test, which has no setup')
        if not record.config:
            raise ValueError('the run\'s record holds no configuration')
        table = {key: record.table[key] for key in ('rows', 'revision', 'sha256')
                 if key in record.table}
        return cls(record.test, record.config, protocol=PROTOCOL, table=table,
                   title=record.title or str(record.config.get('title', '') or ''),
                   notes=str(record.manifest.get('notes', '') or ''),
                   origin=origin('run', run_uid=record.run_uid or None,
                                 run_id=record.run_id or None, name=name))

    @property
    def configure(self) -> tuple[str, dict[str, Any]]:
        """The configure request this setup would be sent with: its
        method and params. Opening a setup never sends it; this is what
        the shell's draft sends once the operator applies it."""
        method, member = CONFIGURE[self.module]
        return method, {member: copy.deepcopy(self.config)}

    def as_dict(self) -> dict[str, Any]:
        """The setup as the analysis service carries it (`project.open_setup`)."""
        return {'module': self.module, 'protocol': self.protocol,
                'config': copy.deepcopy(self.config), 'table': copy.deepcopy(self.table),
                'title': self.title, 'notes': self.notes, 'origin': dict(self.origin)}

    @classmethod
    def from_dict(cls, held: Mapping[str, Any]) -> TestSetup:
        """A setup from what `as_dict` gives (or the shell sends)."""
        return cls(held['module'], held['config'], protocol=held.get('protocol', PROTOCOL),
                   table=held.get('table'), title=held.get('title', ''),
                   notes=held.get('notes', ''), origin=held.get('origin'))

    def describe(self) -> str:
        """A few words on the setup: its module and where it came from.

        Returns
        -------
        str
            Such as 'random setup, from run r-0003'.
        """
        kind = self.origin['kind']
        if kind == 'setup':
            whence = f'from the setup {self.origin["name"]}'
        elif kind == 'run':
            whence = f'from run {self.origin.get("run_id") or self.origin.get("run_uid")}'
        elif kind == 'seed':
            whence = f'seeded from {self.origin["object"]}'
        else:
            whence = 'typed'
        return f'{self.module} setup, {whence}'

    def __repr__(self) -> str:
        return f'TestSetup({self.describe()})'

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, TestSetup):
            return NotImplemented
        return self.as_dict() == other.as_dict()

    __hash__ = None                                      # type: ignore[assignment]
