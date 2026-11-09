/**
 * The Recorder setup as the operator edits it, and its conversion to and from the protocol's
 * `RecorderConfig` (schema/PROTOCOL.md, "Recorder Configuration"). Pure functions only: the service
 * computes every number about the run (the station's block, batch and group delay, the analysis
 * grid, the pre-window and lengths in frames, the bytes a second, the free-space floor and the time
 * left above it), and this module only carries the operator's choices.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. A field the wire takes as null (the hysteresis, the length, the count,
 * the overlap, the FRF pair) is null here too. The trigger row and level are sent only in `level`
 * mode, where the wire requires them; in the other modes they stay on this screen.
 */

import type { ChannelRow, ReportItem } from '$lib/protocol/types';
import type { RecorderAveraging, RecorderConfig, RecorderSlope, RecorderTriggerMode, RecorderWindow } from '$lib/protocol/recorder';
import type { FieldErrors } from '../setup';

// ---------------------------------------------------------------------------------------------
// The draft

export interface RecorderDraft {
	title: string;
	acquisition: { fs_hz: number };
	trigger: {
		mode: RecorderTriggerMode;
		/** A recorded row's table index; null until chosen. */
		row: number | null;
		/** In the row's unit; NaN until given. */
		level: number;
		slope: RecorderSlope;
		/** Null: 1% of the row's range. */
		hysteresis: number | null;
		pre_s: number;
		/** Null: until cut or the run's end. */
		record_s: number | null;
		/** Null: until the run ends. */
		count: number | null;
		holdoff_s: number;
	};
	analysis: {
		block: number;
		window: RecorderWindow;
		/** Null: the window's own. */
		overlap_pct: number | null;
		averaging: RecorderAveraging;
		averages: number;
		decimation: number;
		frf: { reference: number; response: number } | null;
	};
	capture: { run_name: string; keep_free_gb: number };
	metadata: { key: string; label: string; value: string }[];
}

/** The analysis blocks the service takes: powers of two from 256 to 65536 frames. */
export const BLOCK_CHOICES = [256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536];

/** The decimations: powers of two from 1 to 256. */
export const DECIMATION_CHOICES = [1, 2, 4, 8, 16, 32, 64, 128, 256];

export const MODE_CHOICES: { id: RecorderTriggerMode; label: string; help: string }[] = [
	{ id: 'start', label: 'From the Start', help: "The run's first frame begins a recording, with no pre-window." },
	{ id: 'manual', label: 'Manual', help: 'Record begins a recording; armed, it begins with the pre-window.' },
	{ id: 'level', label: 'Level', help: 'A row crossing a level begins each recording, with the pre-window before it.' }
];

export const WINDOW_CHOICES: { id: RecorderWindow; label: string }[] = [
	{ id: 'hann', label: 'Hann' },
	{ id: 'flattop', label: 'Flat top' },
	{ id: 'rectangular', label: 'Rectangular' },
	{ id: 'blackman_harris', label: 'Blackman-Harris' }
];

export const AVERAGING_CHOICES: { id: RecorderAveraging; label: string }[] = [
	{ id: 'linear', label: 'Linear' },
	{ id: 'exponential', label: 'Exponential' },
	{ id: 'peak_hold', label: 'Peak hold' }
];

export function defaultRecorderSetup(): RecorderDraft {
	return {
		title: 'Recorder run',
		acquisition: { fs_hz: 12800 },
		trigger: { mode: 'manual', row: null, level: NaN, slope: 'rising', hysteresis: null, pre_s: 1, record_s: null, count: 1, holdoff_s: 0 },
		analysis: { block: 2048, window: 'hann', overlap_pct: null, averaging: 'linear', averages: 8, decimation: 1, frf: null },
		capture: { run_name: 'recorder', keep_free_gb: 1 },
		metadata: [{ key: 'run_number', label: 'Run Number', value: '' }]
	};
}

const isMode = (v: unknown): v is RecorderTriggerMode => v === 'start' || v === 'manual' || v === 'level';

/** A draft read back from storage, every part filled from the defaults where it is missing. */
export function mergeRecorderSetup(saved: Partial<RecorderDraft> | null): RecorderDraft {
	const base = defaultRecorderSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as RecorderDraft;
	if (typeof out.title !== 'string') out.title = base.title;
	out.acquisition = { ...base.acquisition, ...(saved.acquisition ?? {}) };
	out.trigger = { ...base.trigger, ...(saved.trigger ?? {}) };
	if (!isMode(out.trigger.mode)) out.trigger.mode = base.trigger.mode;
	out.analysis = { ...base.analysis, ...(saved.analysis ?? {}) };
	out.capture = { ...base.capture, ...(saved.capture ?? {}) };
	if (!Array.isArray(out.metadata)) out.metadata = base.metadata;
	return out;
}

// ---------------------------------------------------------------------------------------------
// To and from the protocol

/**
 * The configuration the wire takes: every field explicit, the trigger row and level only in
 * `level` mode (where they are required; the service would read them nowhere else).
 */
export function toRecorderConfig(d: RecorderDraft): RecorderConfig {
	const t = d.trigger;
	const a = d.analysis;
	const trigger: NonNullable<RecorderConfig['trigger']> = { mode: t.mode };
	if (t.mode === 'level') {
		if (t.row !== null) trigger.row = t.row;
		trigger.level = t.level;
	}
	Object.assign(trigger, { slope: t.slope, hysteresis: t.hysteresis, pre_s: t.pre_s, record_s: t.record_s, count: t.count, holdoff_s: t.holdoff_s });
	return {
		title: d.title.trim(),
		acquisition: { fs_hz: d.acquisition.fs_hz },
		trigger,
		analysis: {
			block: a.block,
			window: a.window,
			overlap_pct: a.overlap_pct,
			averaging: a.averaging,
			averages: a.averages,
			decimation: a.decimation,
			frf: a.frf ? { reference: a.frf.reference, response: a.frf.response } : null
		},
		capture: { run_name: d.capture.run_name.trim(), keep_free_gb: d.capture.keep_free_gb },
		metadata: d.metadata.map((m) => (m.label ? { key: m.key, label: m.label, value: m.value } : { key: m.key, value: m.value }))
	};
}

/**
 * The draft for a configuration the service holds (another screen applied it, or this one before a
 * reload). Fields the configuration leaves out take the wire's defaults.
 */
export function fromRecorderConfig(c: RecorderConfig): RecorderDraft {
	const t = c.trigger ?? {};
	const a = c.analysis ?? {};
	return {
		title: c.title ?? '',
		acquisition: { fs_hz: c.acquisition?.fs_hz ?? 12800 },
		trigger: {
			mode: t.mode ?? 'manual',
			row: t.row ?? null,
			level: t.level ?? NaN,
			slope: t.slope ?? 'rising',
			hysteresis: t.hysteresis ?? null,
			pre_s: t.pre_s ?? 1,
			record_s: t.record_s ?? null,
			count: t.count === undefined ? 1 : t.count,
			holdoff_s: t.holdoff_s ?? 0
		},
		analysis: {
			block: a.block ?? 2048,
			window: a.window ?? 'hann',
			overlap_pct: a.overlap_pct ?? null,
			averaging: a.averaging ?? 'linear',
			averages: a.averages ?? 8,
			decimation: a.decimation ?? 1,
			frf: a.frf ? { reference: a.frf.reference, response: a.frf.response } : null
		},
		capture: { run_name: c.capture?.run_name ?? 'recorder', keep_free_gb: c.capture?.keep_free_gb ?? 1 },
		metadata: (c.metadata ?? []).map((m) => ({ key: m.key, label: m.label ?? '', value: m.value }))
	};
}

// ---------------------------------------------------------------------------------------------
// The run's rows

/**
 * The rows a run records and the ones it leaves alone, as PROTOCOL names them: every enabled input
 * row (control, monitor or reference) in table order, each a column of every recording; the
 * enabled drive rows are not opened and not recorded.
 */
export function recorderRows(rows: ChannelRow[]): { recorded: ChannelRow[]; drives: ChannelRow[] } {
	const on = rows.filter((r) => r.enabled);
	return { recorded: on.filter((r) => r.role !== 'drive'), drives: on.filter((r) => r.role === 'drive') };
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

/** The setup's sections, in the screen's order. */
export type RecorderSection = 'station' | 'trigger' | 'analysis' | 'capture';

export const RECORDER_SECTIONS: { id: RecorderSection; label: string }[] = [
	{ id: 'station', label: 'Station & Channels' },
	{ id: 'trigger', label: 'Rate & Trigger' },
	{ id: 'analysis', label: 'Analysis' },
	{ id: 'capture', label: 'Records' }
];

/** The table's findings, located on the station's section. */
const TABLE_REASONS = ['no_input_channel', 'drive_channel_present'];

/**
 * The editor's key for a service finding. A configuration item's `field` is already the path
 * inside the configuration; a refusal's `param` (a shape error) carries the request's `config.`
 * head, which goes. The FRF pair's two rows share the pair's key.
 */
export function recorderItemKey(item: Pick<ReportItem, 'field' | 'channel'> & { reason: string }): string {
	if (TABLE_REASONS.includes(item.reason)) return 'station.channels';
	if (item.field) {
		const path = item.field.replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
		if (/^analysis\.frf(\.|$)/.test(path)) return 'analysis.frf';
		if (/^metadata(\.|$)/.test(path)) return 'metadata';
		return path;
	}
	if (item.channel !== undefined && item.channel !== null) return 'station.channels';
	return 'setup';
}

export function recorderByKey(items: { field?: string | null; channel?: number | null; reason: string; message: string }[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = recorderItemKey(it as Pick<ReportItem, 'field' | 'channel'> & { reason: string });
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

/** The section a key belongs to. */
export function recorderSectionOf(key: string): RecorderSection {
	if (/^station|^table|^channel/.test(key)) return 'station';
	if (/^(capture|metadata|title)(\.|$)/.test(key)) return 'capture';
	if (/^analysis(\.|$)/.test(key)) return 'analysis';
	return 'trigger';
}

// ---------------------------------------------------------------------------------------------
// What the editor can say before anything is sent

const NUMBER = 'Enter a number.';

function add(map: FieldErrors, key: string, message: string) {
	(map[key] ??= []).push(message);
}

function need(map: FieldErrors, key: string, v: number | null) {
	if (v !== null && !Number.isFinite(v)) add(map, key, NUMBER);
}

/**
 * Fields that hold no number, a level trigger without its row, its level or its length, a count
 * that is not whole, an FRF pair not given in full, a run folder not named. The ranges, the level
 * and the hysteresis against the row's range, the powers of two, the pair on one row and the
 * table's inputs are the service's to judge in the dry run.
 */
export function localErrors(d: RecorderDraft): FieldErrors {
	const out: FieldErrors = {};
	const t = d.trigger;
	need(out, 'acquisition.fs_hz', d.acquisition.fs_hz);
	need(out, 'trigger.pre_s', t.pre_s);
	need(out, 'trigger.record_s', t.record_s);
	need(out, 'trigger.hysteresis', t.hysteresis);
	need(out, 'trigger.holdoff_s', t.holdoff_s);
	if (t.count !== null && !Number.isInteger(t.count)) add(out, 'trigger.count', Number.isFinite(t.count) ? 'A whole number of recordings.' : NUMBER);
	if (t.mode === 'level') {
		if (t.row === null) add(out, 'trigger.row', 'Choose the row the level is read on.');
		if (!Number.isFinite(t.level)) add(out, 'trigger.level', "Give the level, in the row's unit.");
		if (t.record_s === null) add(out, 'trigger.record_s', 'A level recording needs a length, so the trigger can re-arm after it.');
	}
	const a = d.analysis;
	need(out, 'analysis.overlap_pct', a.overlap_pct);
	need(out, 'analysis.averages', a.averages);
	if (a.frf && !(Number.isInteger(a.frf.reference) && Number.isInteger(a.frf.response))) add(out, 'analysis.frf', 'Choose both rows of the pair.');
	if (!d.capture.run_name.trim()) add(out, 'capture.run_name', 'Name the run folder.');
	need(out, 'capture.keep_free_gb', d.capture.keep_free_gb);
	return out;
}
