"""The control service's own record of one run, held in a project.

A run that the control service records leaves a run folder: a manifest,
the channel table it armed with, the setup as accepted and as resolved,
the law it ran, its log, its streams and results, and how it ended
(`hertzforge.io.run_folder`). Converting the folder
(`hertzforge.io.run_convert`) turns the streams and results into the
analysis objects they are, time histories, PSDs, FRFs and the rest, and
keeps everything else here, in one `RunRecord` per run: what the run
was, what it was set to, what happened during it, and the results no
analysis class holds, as named arrays with their description. Nothing
in the folder is lost by the conversion.

A record is saved in the project file as the `run_record` kind
(`docs/hforge-format.md`), so it survives saving, reopening and
importing into another project like any other object, and the run's
objects are linked to it through one object group.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from typing import Any

import numpy as np


class RunRecord:
    """What the control service recorded of one run, beyond its data.

    The members are kept as the folder had them, parsed from JSON, so a
    member this build knows nothing of is carried, not dropped.

    Parameters
    ----------
    manifest : mapping
        The run's manifest (`run.json`): its test, identity, title,
        operator, article, specification, source, station, build, start,
        declared streams and results, and whether it completed.
    table : mapping
        The channel table as armed (`table.json`), every row in the
        control service's spelling, with its revision and hash.
    setup : mapping
        The setup (`setup.json`): `config` as accepted, `resolved` as the
        service resolved it, and `derived`, `feasibility` and
        `seeded_from` when the service wrote them.
    log : sequence of mapping, optional
        The run's log entries, in order.
    end : mapping, optional
        How the run ended (`end.json`); None for a run that did not end
        cleanly.
    law : mapping, optional
        The law's identity (`law/law.json`); None when no law ran.
    law_file : tuple of (str, bytes), optional
        The law file's name and bytes, when the law is a file.
    streams : mapping of str to mapping, optional
        Each stream's description (`stream.json`) by name: its channels,
        rate, first station frame, gaps and shots.
    results : mapping of str to mapping, optional
        Each result's description by name, with the records of a
        repeating one under `records`.
    arrays : mapping of str to mapping of str to numpy.ndarray, optional
        The arrays of each result no analysis class holds, by result and
        array name; a repeating result's arrays have the record first.
    missing : sequence of str, optional
        What the folder lacked when it was converted, in words (a run
        that did not end, a stream cut short).

    Attributes
    ----------
    source : str or None
        'SIM' or 'HARDWARE', from the manifest.
    """

    def __init__(self, manifest: Mapping[str, Any], table: Mapping[str, Any],
                 setup: Mapping[str, Any],
                 log: Sequence[Mapping[str, Any]] = (),
                 end: Mapping[str, Any] | None = None,
                 law: Mapping[str, Any] | None = None,
                 law_file: tuple[str, bytes] | None = None,
                 streams: Mapping[str, Mapping[str, Any]] | None = None,
                 results: Mapping[str, Mapping[str, Any]] | None = None,
                 arrays: Mapping[str, Mapping[str, np.ndarray]] | None = None,
                 missing: Sequence[str] = ()) -> None:
        self.manifest: dict[str, Any] = dict(manifest)
        self.table: dict[str, Any] = dict(table)
        self.setup: dict[str, Any] = dict(setup)
        self.log: list[dict[str, Any]] = [dict(entry) for entry in log]
        self.end: dict[str, Any] | None = None if end is None else dict(end)
        self.law: dict[str, Any] | None = None if law is None else dict(law)
        self.law_file: tuple[str, bytes] | None = (
            None if law_file is None else (str(law_file[0]), bytes(law_file[1])))
        self.streams: dict[str, dict[str, Any]] = {
            name: dict(value) for name, value in (streams or {}).items()}
        self.results: dict[str, dict[str, Any]] = {
            name: dict(value) for name, value in (results or {}).items()}
        self.arrays: dict[str, dict[str, np.ndarray]] = {
            name: {key: np.asarray(value) for key, value in held.items()}
            for name, held in (arrays or {}).items()}
        self.missing: list[str] = [str(m) for m in missing]
        source = self.manifest.get('source')
        self.source: str | None = None if source is None else str(source)

    # ---- what the run was -------------------------------------------------

    @property
    def test(self) -> str:
        """The test the run was ('random', 'recorder', ...)."""
        return str(self.manifest.get('test', ''))

    @property
    def run_uid(self) -> str:
        """The run's identity, unique across restarts and machines."""
        return str(self.manifest.get('run_uid', ''))

    @property
    def run_id(self) -> str:
        """The service's own id for the run, which repeats after a restart."""
        return str(self.manifest.get('run_id', ''))

    @property
    def title(self) -> str:
        """The operator's title for the run, '' when none was given."""
        return str(self.manifest.get('title', '') or '')

    @property
    def complete(self) -> bool:
        """Whether the run ended cleanly: its manifest says so and it has
        an end."""
        return bool(self.manifest.get('complete')) and self.end is not None

    @property
    def outcome(self) -> str:
        """How the run ended ('completed', 'aborted', 'operator_stop'),
        '' for a run that did not end cleanly."""
        if self.end is None:
            return ''
        end = self.end.get('end')
        return str(end.get('outcome', '')) if isinstance(end, Mapping) else ''

    @property
    def config(self) -> dict[str, Any]:
        """The setup exactly as the service accepted it."""
        config = self.setup.get('config')
        return dict(config) if isinstance(config, Mapping) else {}

    @property
    def resolved(self) -> dict[str, Any]:
        """What the service resolved the setup to: profile, limits,
        schedule, control settings."""
        resolved = self.setup.get('resolved')
        return dict(resolved) if isinstance(resolved, Mapping) else {}

    def describe(self) -> str:
        """A few words on the run: its test, its outcome and its log.

        Returns
        -------
        str
            Such as 'random run, completed, 14 log entries'.
        """
        words = [f'{self.test or "unknown"} run']
        words.append(self.outcome or 'did not end cleanly')
        count = len(self.log)
        words.append(f'{count} log entr{"y" if count == 1 else "ies"}')
        return ', '.join(words)

    def __repr__(self) -> str:
        return (f'RunRecord({self.test or "unknown"} run '
                f'{self.run_uid or self.run_id!r}, {self.outcome or "not ended"})')

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, RunRecord):
            return NotImplemented
        if (self.manifest, self.table, self.setup, self.log, self.end, self.law,
                self.law_file, self.streams, self.results, self.missing) != (
                other.manifest, other.table, other.setup, other.log, other.end,
                other.law, other.law_file, other.streams, other.results,
                other.missing):
            return False
        if self.arrays.keys() != other.arrays.keys():
            return False
        for name, held in self.arrays.items():
            theirs = other.arrays[name]
            if held.keys() != theirs.keys():
                return False
            for key, values in held.items():
                if (values.dtype != theirs[key].dtype
                        or not np.array_equal(values, theirs[key], equal_nan=True)):
                    return False
        return True

    __hash__ = None                                      # type: ignore[assignment]
