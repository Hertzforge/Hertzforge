"""Seeds: an analysis object in the open project fills part of a test's
setup.

A seed is a conversion (`io.setup_convert`) from an object the project
holds to the part of a test's configuration that object describes,
with where it came from. It fills a draft and nothing else: the shell
applies its `sets` to the draft's configuration (each `path` a dotted
member of the configure request, its `value` put there whole), shows
the operator every member that changes before the draft takes it, and
the draft then goes through the same `configure` checks as one typed in.

A protective number (an abort band, a ceiling) taken from an object is
never among the `sets`: it is listed under `protective`, and the shell
fills it only when the operator has typed it. A seed whose module needs
a file stored on the control service first (a TWR reference) names the
request under `imports`; the control service checks what it stores as
it checks a file the operator picks.

Each seed's origin names the object and its SHA-256 (`checksum`), and
what it filled, so a saved setup says which version of which object it
took and the Arm line says so.

The seeds, in the order a person meets them:

========================  ==================================  =====================
module                    object                              fills
========================  ==================================  =====================
random                    Specification (autospectra)         the profile
sine                      SineSweepSpecification (one tone)   the profile and test
srs                       ShockSpecification                  the target and bands
classical_shock           TransientSpecification or           the pulse
                          TimeHistory that is a classical
                          pulse
twr                       TimeHistory or                      the reference
                          TransientSpecification
mimo_random               Specification with cross terms      the reference and its
                          (through `reference_convert`)       control bands
mixed                     Specification (a floor, with its    the parts
                          narrowbands), and the tones a
                          SineSweepSpecification beside it
                          in an object group holds
========================  ==================================  =====================
"""

from __future__ import annotations

import copy
from collections.abc import Mapping
from typing import Any

from . import setup_convert as sc

#: the modules a seed fills, and what each fills, in words
FILLS = {'random': 'the profile', 'sine': 'the profile and test',
         'srs': 'the target and bands', 'classical_shock': 'the pulse',
         'twr': 'the reference', 'mixed': 'the parts', 'mimo_random': 'the reference'}


class SeedRefused(ValueError):
    """An object that cannot seed a module, with the reason."""


def checksum(obj: Any) -> str:
    """The object's SHA-256: of everything saving it writes, in a fixed
    order, so the same object gives the same digest in any session."""
    import h5py

    from . import native

    with h5py.File(f'seed-{id(obj)}', 'w', driver='core', backing_store=False) as f:
        native.save_into(obj, f)
        return native._tree_digest(f)


def _kinds(module: str) -> tuple[type, ...]:
    from ..core.data import ShockSpecification, Specification, TimeHistory
    from ..core.sine import SineSweepSpecification

    return {'random': (Specification,), 'mixed': (Specification,),
            'mimo_random': (Specification,),
            'sine': (SineSweepSpecification,), 'srs': (ShockSpecification,),
            'classical_shock': (TimeHistory,), 'twr': (TimeHistory,)}.get(module, ())


def _offered(obj: Any, module: str) -> bool:
    from ..core.data import ShockSpecification, Specification

    if not isinstance(obj, _kinds(module)):
        return False
    if module in ('random', 'mixed', 'mimo_random'):
        # a shock specification is a specification of another kind
        return isinstance(obj, Specification) and not isinstance(obj, ShockSpecification)
    return True


def _tones_beside(project: Any, name: str) -> str | None:
    """The sine specification an object group holds with `name`, the
    tones of a mixed specification as the run converter writes one."""
    from ..core.sine import SineSweepSpecification

    for group in project.object_groups:
        members = group.get('members') or []
        if name in members:
            for other in members:
                if other != name and isinstance(project.get(other), SineSweepSpecification):
                    return other
    return None


def offers(project: Any, module: str) -> list[dict[str, Any]]:
    """What the open project offers `module` as seeds: each object of a
    kind the module takes, with its kind, what it would fill, the
    channels (or records) it could be read at, and, for Mixed Mode, the
    tones beside it. Whether it converts is decided when it is chosen."""
    from .. import project as project_module

    if module not in FILLS:
        return []
    out = []
    for name, obj in project.items():
        if not _offered(obj, module):
            continue
        entry: dict[str, Any] = {'name': name, 'kind': type(obj).__name__,
                                 'fills': FILLS[module],
                                 'holds': project_module.describe(obj),
                                 'channels': [str(d) for d in obj.response_dof]}
        if module == 'mixed':
            tones = _tones_beside(project, name)
            if tones:
                entry['tones'] = tones
        out.append(entry)
    return out


def _put(sets: list[dict[str, Any]], path: str, value: Any) -> None:
    sets.append({'path': path, 'value': value})


def _guard(protective: list[dict[str, Any]], path: str, value: Any, words: str) -> None:
    protective.append({'path': path, 'value': value, 'words': words})


def _random(obj: Any, channel: str | None, sets, protective) -> list[str]:
    profile = sc.random_profile(obj, channel)
    rows = copy.deepcopy(profile['breakpoints'])
    for k, row in enumerate(rows):
        own = row.pop('abort_db', None)
        if own is not None:
            _guard(protective, f'profile.breakpoints.{k}.abort_db', own,
                   f'The abort band of the row at {row["freq_hz"]:g} Hz, dB below and above')
    _put(sets, 'profile.breakpoints', rows)
    if profile['alarm_db'] is not None:
        _put(sets, 'profile.alarm_db', profile['alarm_db'])
    if profile['abort_db'] is not None:
        _guard(protective, 'profile.abort_db', profile['abort_db'],
               'The abort band, dB below and above the profile')
    return profile['notes']


def _sine(obj: Any, sets, protective) -> list[str]:
    profile = sc.sine_profile(obj)
    rows = copy.deepcopy(profile['rows'])
    for k, row in enumerate(rows):
        own = row.pop('abort_db', None)
        if own is not None:
            _guard(protective, f'profile.rows.{k}.abort_db', own,
                   f'The abort band of the row at {row["freq_hz"]:g} Hz, dB below and above')
    _put(sets, 'profile.rows', rows)
    if profile['alarm_db'] is not None:
        _put(sets, 'profile.alarm_db', profile['alarm_db'])
    if profile['abort_db'] is not None:
        _guard(protective, 'profile.abort_db', profile['abort_db'],
               'The abort band, dB below and above the profile')
    # the test's members, one by one: the draft keeps how its run ends,
    # which a specification's one pass at full level does not say
    test = profile['test']
    for key in ('kind', 'band_hz', 'rate', 'start_hz', 'direction', 'freq_hz'):
        if key in test:
            _put(sets, f'test.{key}', test[key])
    if test['kind'] == 'dwell':
        _put(sets, 'test.end', test['end'])
    return profile['notes']


def _srs(obj: Any, channel: str | None, sets, protective) -> list[str]:
    target = sc.srs_target(obj, channel)
    _put(sets, 'target', target['target'])
    for key, value in target['bands'].items():
        _put(sets, f'bands.{key}', value)
    if target['limits']:
        # bands from the specification replace any per-band bands
        _put(sets, 'envelope.per_band', None)
    if target['alarm_db'] is not None:
        _put(sets, 'envelope.alarm_db', target['alarm_db'])
    if target['abort_db'] is not None:
        _guard(protective, 'envelope.abort_db', target['abort_db'],
               'The abort band, dB under and over the target')
    return target['notes']


def _pulse(obj: Any, record: int, sets) -> list[str]:
    import numpy as np

    if not 0 <= record < obj.num_records:
        raise sc.ConversionRefused(f'it has no record {record}')
    if obj.ordinate_dim[record] != 'acceleration':
        raise sc.ConversionRefused(f'{obj.response_dof[record]} is of '
                                   f'{obj.ordinate_dim[record]}, and a classical pulse is of '
                                   'acceleration')
    pulse, off = sc.pulse_of_waveform(np.asarray(obj.abscissa), obj.ordinate[record])
    for key, value in sc.shock_pulse(pulse).items():
        _put(sets, f'pulse.{key}', value)
    return [(f'read as a {pulse.describe("g", 1.0 / sc.G)}, within {100 * off:.2g}% of its '
            'peak at every sample')]


def _mixed(obj: Any, tones: Any, channel: str | None, sets, protective) -> list[str]:
    found = sc.mixed_parts(obj, tones, channel)
    parts = copy.deepcopy(found['parts'])
    for k, part in enumerate(parts):
        own = part.pop('abort_db', None)
        if own is not None:
            _guard(protective, f'parts.{k}.abort_db', own,
                   f'The abort band of {part.get("name") or part["id"]}, dB below and above')
    for k, part in enumerate(parts):
        limits = found['tone_limits'].get(part['id']) or {}
        if 'alarm_db' in limits:
            part['alarm_db'] = limits['alarm_db']
        if 'abort_db' in limits:
            _guard(protective, f'parts.{k}.abort_db', limits['abort_db'],
                   f'The abort band of {part.get("name") or part["id"]}, dB below and above')
    _put(sets, 'parts', parts)
    if found['exclusion_hz'] is not None:
        _put(sets, 'exclusion_hz', found['exclusion_hz'])
    return found['notes']


def _mimo(obj: Any, name: str, sets, protective) -> list[str]:
    found = sc.mimo_reference(obj, 'g', name)
    _put(sets, 'reference', found['reference'])
    for k, dof in enumerate(found['channels']):
        if found['alarm_db'] is not None:
            _put(sets, f'channels.controls.{k}.alarm_db', found['alarm_db'][k])
        if found['abort_db'] is not None:
            _guard(protective, f'channels.controls.{k}.abort_db', found['abort_db'][k],
                   f'The abort band of control {k + 1} ({dof}), dB below and above')
    return found['notes']


def seed(project: Any, name: str, module: str, *, channel: str | None = None,
         record: int = 0, units: str | None = None, tones: str | None = None
         ) -> dict[str, Any]:
    """The part of `module`'s configuration the object `name` fills.

    Parameters
    ----------
    project : Project
        The open project.
    name : str
        The object.
    module : str
        The test module, as a run's manifest names it.
    channel : str, optional
        The channel to read a specification at, where its channels
        differ (Random, SRS, Mixed Mode).
    record : int, default 0
        The record of a time history (Classical Shock, TWR).
    units : str, optional
        The TWR control channel's unit; 'g' when not given.
    tones : str, optional
        A Mixed Mode floor's tones, a sine specification in the project.

    Returns
    -------
    dict
        `module`; `sets`, each `{path, value}` to put into the draft's
        configuration in order; `protective`, each `{path, value, words}`
        filled only once the operator types it; `imports`, each
        `{method, params, set}` (a request to the control service whose
        stored reference is then put at `set`); `notes`; and `origin`,
        `{kind: 'seed', object, checksum, fills}` (with `also` naming a
        second object a Mixed Mode seed read).

    Raises
    ------
    SeedRefused
        For a module no seed fills, an object the module does not take,
        or one it cannot control to, with the reason.
    """
    if module not in FILLS:
        raise SeedRefused(f'nothing seeds a {module} setup')
    if name not in project:
        raise KeyError(name)
    obj = project[name]
    if not _offered(obj, module):
        raise SeedRefused(f'{name} is a {type(obj).__name__}, which does not seed a '
                          f'{module} setup')
    sets: list[dict[str, Any]] = []
    protective: list[dict[str, Any]] = []
    imports: list[dict[str, Any]] = []
    origin: dict[str, Any] = {'kind': 'seed', 'object': name, 'checksum': checksum(obj),
                              'fills': FILLS[module]}
    try:
        if module == 'random':
            notes = _random(obj, channel, sets, protective)
        elif module == 'sine':
            notes = _sine(obj, sets, protective)
        elif module == 'srs':
            notes = _srs(obj, channel, sets, protective)
        elif module == 'classical_shock':
            notes = _pulse(obj, int(record), sets)
        elif module == 'mimo_random':
            notes = _mimo(obj, name, sets, protective)
        elif module == 'twr':
            ref = sc.twr_reference(obj, int(record), units or 'g')
            imports.append({'method': 'twr.reference.import', 'set': 'reference',
                            'params': {'text': ref['text'], 'fs_hz': ref['fs_hz'],
                                       'units': ref['units'], 'name': name}})
            notes = [(f'{ref["samples"]} samples of {ref["channel"]} at {ref["fs_hz"]:g} Hz, '
                     f'in {ref["units"]}')]
        else:
            other = None
            if tones is not None:
                if tones not in project:
                    raise KeyError(tones)
                other = project[tones]
                origin['also'] = [{'object': tones, 'checksum': checksum(other)}]
            notes = _mixed(obj, other, channel, sets, protective)
    except sc.ConversionRefused as error:
        raise SeedRefused(f'{name}: {error}') from error
    return {'module': module, 'sets': sets, 'protective': protective, 'imports': imports,
            'notes': list(notes), 'origin': origin}


def apply(config: Mapping[str, Any], sets: list[Mapping[str, Any]]) -> dict[str, Any]:
    """`config` with each of `sets` put in, in order: a dotted `path`
    walks objects by member and lists by index, making an object where
    a member is missing, and its `value` replaces what was there whole.
    The shell does the same to its draft (`setupFiles/seed.ts`)."""
    out = copy.deepcopy(dict(config))
    for item in sets:
        keys = str(item['path']).split('.')
        at: Any = out
        for key in keys[:-1]:
            if isinstance(at, list):
                k = int(key)
                while len(at) <= k:
                    at.append({})
                if not isinstance(at[k], (dict, list)):
                    at[k] = {}
                at = at[k]
                continue
            if not isinstance(at.get(key), (dict, list)):
                at[key] = {}
            at = at[key]
        last = keys[-1]
        value = copy.deepcopy(item['value'])
        if isinstance(at, list):
            at[int(last)] = value
        else:
            at[last] = value
    return out


def fingerprint_matches(project: Any, origin: Mapping[str, Any]) -> bool:
    """Whether the object a seed's origin names is still in `project`
    with the same SHA-256."""
    name = origin.get('object')
    return bool(name in project and checksum(project[name]) == origin.get('checksum'))


__all__ = ['FILLS', 'SeedRefused', 'apply', 'checksum', 'fingerprint_matches', 'offers', 'seed']
