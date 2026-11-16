/**
 * The Playback setup as the operator edits it, and its conversion to and from the protocol's
 * `PlaybackConfig` (schema/PROTOCOL.md, "Playback Configuration"). Pure functions only: the
 * control service checks the drive file and computes every number about the run (each channel's
 * peak, ends, velocity and displacement, the gain, the rolls in frames, the stop), and this module
 * only carries the operator's choices.
 *
 * The setup names a drive folder the analysis side wrote, by its full path and the SHA-256 of its
 * samples, and maps each of its channels to a drive row of the channel table. Choosing and
 * converting a file is a step that makes that folder; the file's path and the signals picked are
 * kept in the folder's own sidecar (`drive.json`), which the run's record holds, so they are not
 * part of the setup.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent.
 */

import type { ChannelRow, ReportItem } from '$lib/protocol/types';
import type { PlaybackConfig } from '$lib/protocol/playback';
import { defaultRecord, type FieldErrors, type RecordDraft } from '../setup';

export interface PlaybackDraft {
	title: string;
	/** The drive folder and its pinned SHA-256; empty until a file is converted or a setup opened. */
	drive: { folder: string; sha256: string };
	/** The drive row (table index) each file channel plays on; null until chosen. */
	map: (number | null)[];
	level_db: number;
	ceiling_v: number;
	pre_roll_s: number;
	post_roll_s: number;
	capture: { run_name: string };
	metadata: { key: string; label: string; value: string }[];
	record: RecordDraft;
}

export function defaultPlaybackSetup(): PlaybackDraft {
	return {
		title: 'Playback run',
		drive: { folder: '', sha256: '' },
		map: [],
		level_db: -12,
		ceiling_v: NaN,
		pre_roll_s: 0.5,
		post_roll_s: 2,
		capture: { run_name: 'playback' },
		metadata: [{ key: 'run_number', label: 'Run Number', value: '' }],
		record: defaultRecord()
	};
}

/** A draft read back from storage, every part filled from the defaults where it is missing. */
export function mergePlaybackSetup(saved: Partial<PlaybackDraft> | null): PlaybackDraft {
	const base = defaultPlaybackSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as PlaybackDraft;
	if (typeof out.title !== 'string') out.title = base.title;
	out.drive = { ...base.drive, ...(saved.drive ?? {}) };
	if (!Array.isArray(out.map)) out.map = [];
	out.capture = { ...base.capture, ...(saved.capture ?? {}) };
	if (!Array.isArray(out.metadata)) out.metadata = base.metadata;
	out.record = { ...base.record, ...(saved.record ?? {}) };
	return out;
}

/** The SHA-256 the service takes: 64 hexadecimal digits. */
export const isSha256 = (s: string) => /^[0-9a-fA-F]{64}$/.test(s);

/**
 * The configuration the wire takes: every field explicit, the run record's words only when given
 * (the service reads an absent one as empty). A channel with no drive row yet is sent as no row,
 * which `localErrors` names first.
 */
export function toPlaybackConfig(d: PlaybackDraft): PlaybackConfig {
	const r = d.record;
	const [document, clause] = [r.document.trim(), r.clause.trim()];
	return {
		title: d.title.trim(),
		drive: { folder: d.drive.folder.trim(), sha256: d.drive.sha256.trim() },
		map: d.map.map((m) => (m === null ? -1 : m)),
		level_db: d.level_db,
		ceiling_v: d.ceiling_v,
		pre_roll_s: d.pre_roll_s,
		post_roll_s: d.post_roll_s,
		capture: { run_name: d.capture.run_name.trim() },
		metadata: d.metadata.map((m) => (m.label ? { key: m.key, label: m.label, value: m.value } : { key: m.key, value: m.value })),
		...(r.operator.trim() ? { operator: r.operator.trim() } : {}),
		...(r.article.trim() ? { article: r.article.trim() } : {}),
		...(r.notes.trim() ? { notes: r.notes.trim() } : {}),
		...(document || clause ? { specification: { document, clause } } : {}),
		record: { streams: r.streams }
	};
}

/**
 * The draft for a configuration the service holds (another screen applied it, or a setup or a run
 * opened). Fields the configuration leaves out take the wire's defaults.
 */
export function fromPlaybackConfig(c: PlaybackConfig): PlaybackDraft {
	return {
		title: c.title ?? '',
		drive: { folder: c.drive?.folder ?? '', sha256: c.drive?.sha256 ?? '' },
		map: (c.map ?? []).map((m) => (Number.isInteger(m) && m >= 0 ? m : null)),
		level_db: c.level_db ?? -12,
		ceiling_v: c.ceiling_v ?? NaN,
		pre_roll_s: c.pre_roll_s ?? 0.5,
		post_roll_s: c.post_roll_s ?? 2,
		capture: { run_name: c.capture?.run_name ?? 'playback' },
		metadata: (c.metadata ?? []).map((m) => ({ key: m.key, label: m.label ?? '', value: m.value })),
		record: {
			operator: c.operator ?? '',
			article: c.article ?? '',
			notes: c.notes ?? '',
			document: c.specification?.document ?? '',
			clause: c.specification?.clause ?? '',
			streams: c.record?.streams ?? true
		}
	};
}

// ---------------------------------------------------------------------------------------------
// The run's rows

/**
 * The rows a run plays and records, as PROTOCOL names them: every enabled input row records, in
 * table order; every enabled drive row is a place a file channel can play, and a drive row no
 * channel maps to plays 0 V.
 */
export function playbackRows(rows: ChannelRow[]): { inputs: ChannelRow[]; drives: ChannelRow[] } {
	const on = rows.filter((r) => r.enabled);
	return { inputs: on.filter((r) => r.role !== 'drive'), drives: on.filter((r) => r.role === 'drive') };
}

/**
 * The map for a file of `channels` channels: the draft's own where it has one, else each channel
 * on the next enabled drive row in table order no channel has taken yet.
 */
export function fitMap(map: (number | null)[], channels: number, drives: ChannelRow[]): (number | null)[] {
	const out = map.slice(0, channels);
	const taken = new Set(out.filter((m): m is number => m !== null));
	const free = drives.map((r) => r.index).filter((i) => !taken.has(i));
	while (out.length < channels) out.push(free.shift() ?? null);
	return out;
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

export type PlaybackSection = 'station' | 'drive' | 'check' | 'level' | 'capture';

export const PLAYBACK_SECTIONS: { id: PlaybackSection; label: string }[] = [
	{ id: 'station', label: 'Station & Channels' },
	{ id: 'drive', label: 'Drive File' },
	{ id: 'check', label: 'Check' },
	{ id: 'level', label: 'Level & Limits' },
	{ id: 'capture', label: 'Records' }
];

/** The table's findings, located on the station's section. */
const TABLE_REASONS = ['no_input_channel', 'no_drive_channel'];
/** The drive file's own findings about its ends and its numbers, located on the check. */
const CHECK_REASONS = ['ends_not_zero', 'drive_not_finite', 'peak_over_ceiling'];

/**
 * The editor's key for a service finding. A configuration item's `field` is the path inside the
 * configuration; a refusal's `param` (a shape error) carries the request's `config.` head, which
 * goes. The drive file's own refusals are the check's.
 */
export function playbackItemKey(item: Pick<ReportItem, 'field' | 'channel'> & { reason: string }): string {
	if (TABLE_REASONS.includes(item.reason)) return 'station.channels';
	if (CHECK_REASONS.includes(item.reason)) return 'check';
	if (item.field) {
		const path = item.field.replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
		if (/^drive(\.|$)/.test(path)) return 'drive';
		if (/^map(\.|$)/.test(path)) return 'map';
		if (/^metadata(\.|$)/.test(path)) return 'metadata';
		return path;
	}
	if (item.channel !== undefined && item.channel !== null) return 'station.channels';
	return 'setup';
}

export function playbackByKey(items: { field?: string | null; channel?: number | null; reason: string; message: string }[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = playbackItemKey(it as Pick<ReportItem, 'field' | 'channel'> & { reason: string });
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

/** The section a key belongs to. */
export function playbackSectionOf(key: string): PlaybackSection {
	if (/^station|^table|^channel/.test(key)) return 'station';
	if (/^check$/.test(key)) return 'check';
	if (/^(drive|map)(\.|$)/.test(key)) return 'drive';
	if (/^(capture|metadata|title|operator|article|notes|specification|record)(\.|$)/.test(key)) return 'capture';
	return 'level';
}

// ---------------------------------------------------------------------------------------------
// What the editor can say before anything is sent

const NUMBER = 'Enter a number.';

function add(map: FieldErrors, key: string, message: string) {
	(map[key] ??= []).push(message);
}

function need(map: FieldErrors, key: string, v: number) {
	if (!Number.isFinite(v)) add(map, key, NUMBER);
}

/**
 * No drive file yet, a channel with no drive row, fields that hold no number, a run folder not
 * named. The ranges, the rows the map names, the rate, the peak against the ceiling and the ends
 * are the control service's to judge in the dry run.
 */
export function localErrors(d: PlaybackDraft): FieldErrors {
	const out: FieldErrors = {};
	if (!d.drive.folder.trim() || !isSha256(d.drive.sha256.trim())) add(out, 'drive', 'Convert a drive file first, or open a setup that names one.');
	else if (!d.map.length) add(out, 'map', 'The drive file names no channel to play.');
	d.map.forEach((m, k) => {
		if (m === null) add(out, 'map', `Choose the drive row file channel ${k} plays on.`);
	});
	need(out, 'level_db', d.level_db);
	if (!Number.isFinite(d.ceiling_v)) add(out, 'ceiling_v', 'Give the drive ceiling, in volts.');
	need(out, 'pre_roll_s', d.pre_roll_s);
	need(out, 'post_roll_s', d.post_roll_s);
	if (!d.capture.run_name.trim()) add(out, 'capture.run_name', 'Name the run folder.');
	if (d.record.clause.trim() && !d.record.document.trim()) add(out, 'specification.document', 'Name the document this clause is from.');
	return out;
}
