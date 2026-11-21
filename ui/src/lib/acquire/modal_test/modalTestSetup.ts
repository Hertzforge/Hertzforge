/**
 * The Modal Test setup as the operator edits it, and its conversion to and from the protocol's
 * `ModalTestConfig` (schema/PROTOCOL.md, "Modal Test"). Pure functions only: the control service
 * resolves the plan (each measurement's DOFs from the channel table's node, direction and
 * analysis role and the roving points), the windows in samples, the trigger level in the hammer
 * row's unit, the time constant and the damping it adds, and refuses what it cannot run. This
 * module carries the operator's choices and nothing else.
 *
 * The DOFs are not part of the setup: they are the channel table's own columns (node, direction,
 * analysis role), edited through the table and sent with it.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. A band edge left empty is not sent, and the service takes its default.
 */

import type { ChannelRow, ReportItem } from '$lib/protocol/types';
import type { ModalAcceptMode, ModalEstimator, ModalMode, ModalPoint, ModalPolarity, ModalSource, ModalTestConfig } from '$lib/protocol/modal_test';
import { defaultRecord, type FieldErrors, type RecordDraft } from '../setup';

export interface ModalImpactDraft {
	trigger_level_pct: number;
	/** The hammer's sign: the trigger watches the force rising through the level in it. */
	polarity: ModalPolarity;
	pretrigger_pct: number;
	holdoff_blocks: number;
	double_hit_pct: number;
	tip_warn_db: number;
	accept: ModalAcceptMode;
	force_window: { on: boolean; width_pct: number; fall_pct: number };
	exponential_window: { on: boolean; end_pct: number };
	auto_advance: boolean;
	/** The force target is set when on; its peak in the hammer row's unit. */
	target: { on: boolean; peak: number; tolerance_pct: number };
}

export interface ModalShakerDraft {
	drives: { row: number; level_vrms: number }[];
	source: ModalSource;
	burst_pct: number;
	ramp_s: number;
	ceiling_v: number;
	delay_frames: number;
	overlap_pct: number;
	seed: number;
}

export interface ModalRovingDraft {
	on: boolean;
	/** The table indices of the channels that rove. */
	channels: number[];
	/** The points typed or pasted, or filled by the service from a start, a step and a count. */
	how: 'list' | 'fill';
	text: string;
	fill: { start: number; step: number; count: number };
}

export interface ModalDraft {
	title: string;
	mode: ModalMode;
	fs_hz: number;
	block: number;
	/** NaN: not sent, the service's default. */
	band: { lo_hz: number; hi_hz: number };
	estimator: ModalEstimator;
	averages: number;
	/** The decay check's threshold, percent; NaN: not sent, the service's default. */
	decay_warn_pct: number;
	roving: ModalRovingDraft;
	impact: ModalImpactDraft;
	shaker: ModalShakerDraft;
	capture: { run_name: string };
	metadata: { key: string; label: string; value: string }[];
	record: RecordDraft;
}

export const MODAL_BLOCKS = [256, 512, 1024, 2048, 4096, 8192, 16384, 32768, 65536];

export function defaultImpact(): ModalImpactDraft {
	return {
		trigger_level_pct: 10,
		polarity: 'positive',
		pretrigger_pct: 5,
		holdoff_blocks: 1,
		double_hit_pct: 5,
		tip_warn_db: 20,
		accept: 'auto',
		force_window: { on: true, width_pct: 10, fall_pct: 10 },
		exponential_window: { on: false, end_pct: 10 },
		auto_advance: true,
		target: { on: false, peak: NaN, tolerance_pct: 20 }
	};
}

export function defaultShaker(): ModalShakerDraft {
	return { drives: [], source: 'burst_random', burst_pct: 50, ramp_s: 1, ceiling_v: 1, delay_frames: 2, overlap_pct: 50, seed: 1 };
}

export function defaultModalSetup(): ModalDraft {
	return {
		title: 'Modal test',
		mode: 'impact',
		fs_hz: 2048,
		block: 2048,
		band: { lo_hz: NaN, hi_hz: NaN },
		estimator: 'H1',
		averages: 5,
		decay_warn_pct: NaN,
		roving: { on: false, channels: [], how: 'list', text: '', fill: { start: 1, step: 1, count: 3 } },
		impact: defaultImpact(),
		shaker: defaultShaker(),
		capture: { run_name: 'modal_test' },
		metadata: [{ key: 'run_number', label: 'Run Number', value: '' }],
		record: defaultRecord()
	};
}

/** A draft read back from storage, every part filled from the defaults where it is missing. */
export function mergeModalSetup(saved: Partial<ModalDraft> | null): ModalDraft {
	const base = defaultModalSetup();
	if (!saved || typeof saved !== 'object') return base;
	const s = saved as Partial<ModalDraft>;
	const imp = { ...base.impact, ...(s.impact ?? {}) } as ModalImpactDraft;
	imp.force_window = { ...base.impact.force_window, ...(s.impact?.force_window ?? {}) };
	imp.exponential_window = { ...base.impact.exponential_window, ...(s.impact?.exponential_window ?? {}) };
	imp.target = { ...base.impact.target, ...(s.impact?.target ?? {}) };
	const sh = { ...base.shaker, ...(s.shaker ?? {}) } as ModalShakerDraft;
	if (!Array.isArray(sh.drives)) sh.drives = [];
	const rov = { ...base.roving, ...(s.roving ?? {}) } as ModalRovingDraft;
	rov.fill = { ...base.roving.fill, ...(s.roving?.fill ?? {}) };
	if (!Array.isArray(rov.channels)) rov.channels = [];
	return {
		...base,
		...s,
		band: { ...base.band, ...(s.band ?? {}) },
		roving: rov,
		impact: imp,
		shaker: sh,
		capture: { ...base.capture, ...(s.capture ?? {}) },
		metadata: Array.isArray(s.metadata) ? s.metadata : base.metadata,
		record: { ...base.record, ...(s.record ?? {}) }
	};
}

// ---------------------------------------------------------------------------------------------
// The points

/**
 * The points typed or pasted: node numbers separated by spaces, commas, semicolons, tabs or new
 * lines, each followed by a direction when one channel roves (`4 Z+`, `4Z+`, or `4<TAB>Z+` pasted
 * from a sheet). A direction is an axis then a sign, as the analysis side spells it. Returns the
 * points and, for each word that cannot be read, why.
 */
export function parsePoints(text: string): { points: ModalPoint[]; problems: string[] } {
	const points: ModalPoint[] = [];
	const problems: string[] = [];
	for (const word of text.split(/[\s,;]+/).filter((w) => w.length > 0)) {
		const both = /^(\d+)([XYZxyz][+-])$/.exec(word);
		if (both) {
			points.push({ node: Number(both[1]), direction: both[2].toUpperCase() });
		} else if (/^\d+$/.test(word)) {
			points.push({ node: Number(word) });
		} else if (/^[XYZxyz][+-]$/.test(word) && points.length && !points[points.length - 1].direction) {
			points[points.length - 1].direction = word.toUpperCase();
		} else {
			problems.push(`"${word}" is not a node number, or a node and a direction such as 4 Z+.`);
		}
	}
	return { points, problems };
}

/** The points written back as text, one a line. */
export function pointsText(points: ModalPoint[]): string {
	return points.map((p) => (p.direction ? `${p.node} ${p.direction}` : `${p.node}`)).join('\n');
}

// ---------------------------------------------------------------------------------------------
// To and from the wire

const given = (v: number) => Number.isFinite(v);

/**
 * The configuration the wire takes: the mode's own settings, the roving plan when on, a band edge
 * only when given, the run record's words only when given.
 */
export function toModalConfig(d: ModalDraft): ModalTestConfig {
	const r = d.record;
	const [document, clause] = [r.document.trim(), r.clause.trim()];
	const band: { lo_hz?: number; hi_hz?: number } = {};
	if (given(d.band.lo_hz)) band.lo_hz = d.band.lo_hz;
	if (given(d.band.hi_hz)) band.hi_hz = d.band.hi_hz;
	const out: ModalTestConfig = {
		title: d.title.trim(),
		mode: d.mode,
		fs_hz: d.fs_hz,
		block: d.block,
		...(Object.keys(band).length ? { band } : {}),
		estimator: d.estimator,
		averages: d.averages,
		...(given(d.decay_warn_pct) ? { decay_warn_pct: d.decay_warn_pct } : {}),
		capture: { run_name: d.capture.run_name.trim() },
		metadata: d.metadata.map((m) => (m.label ? { key: m.key, label: m.label, value: m.value } : { key: m.key, value: m.value })),
		...(r.operator.trim() ? { operator: r.operator.trim() } : {}),
		...(r.article.trim() ? { article: r.article.trim() } : {}),
		...(r.notes.trim() ? { notes: r.notes.trim() } : {}),
		...(document || clause ? { specification: { document, clause } } : {}),
		record: { streams: r.streams }
	};
	if (d.roving.on) {
		const f = d.roving.fill;
		out.roving = {
			channels: [...d.roving.channels],
			points: d.roving.how === 'fill' ? { start: f.start, step: f.step, count: f.count } : parsePoints(d.roving.text).points
		};
	}
	if (d.mode === 'impact') {
		const i = d.impact;
		out.impact = {
			trigger_level_pct: i.trigger_level_pct,
			polarity: i.polarity,
			pretrigger_pct: i.pretrigger_pct,
			holdoff_blocks: i.holdoff_blocks,
			double_hit_pct: i.double_hit_pct,
			tip_warn_db: i.tip_warn_db,
			accept: i.accept,
			force_window: { ...i.force_window },
			exponential_window: { ...i.exponential_window },
			auto_advance: i.auto_advance,
			target: i.target.on ? { peak: i.target.peak, tolerance_pct: i.target.tolerance_pct } : null
		};
	} else {
		const s = d.shaker;
		out.shaker = {
			drives: s.drives.map((x) => ({ row: x.row, level_vrms: x.level_vrms })),
			source: s.source,
			burst_pct: s.burst_pct,
			ramp_s: s.ramp_s,
			ceiling_v: s.ceiling_v,
			delay_frames: s.delay_frames,
			overlap_pct: s.overlap_pct,
			seed: s.seed
		};
	}
	return out;
}

/**
 * The draft for a configuration the service holds (another screen applied it, or a setup or a run
 * opened). Fields the configuration leaves out take the wire's defaults.
 */
export function fromModalConfig(c: ModalTestConfig): ModalDraft {
	const base = defaultModalSetup();
	const i = c.impact ?? {};
	const s = c.shaker;
	const pts = c.roving?.points;
	const fill = pts && !Array.isArray(pts) ? pts : null;
	return {
		title: c.title ?? '',
		mode: c.mode === 'shaker' ? 'shaker' : 'impact',
		fs_hz: c.fs_hz ?? base.fs_hz,
		block: c.block ?? 2048,
		band: { lo_hz: c.band?.lo_hz ?? NaN, hi_hz: c.band?.hi_hz ?? NaN },
		estimator: c.estimator ?? 'H1',
		averages: c.averages ?? (c.mode === 'shaker' ? 30 : 5),
		decay_warn_pct: c.decay_warn_pct ?? NaN,
		roving: {
			on: !!c.roving,
			channels: [...(c.roving?.channels ?? [])],
			how: fill ? 'fill' : 'list',
			text: Array.isArray(pts) ? pointsText(pts) : '',
			fill: fill ? { start: fill.start ?? 1, step: fill.step ?? 1, count: fill.count } : { ...base.roving.fill }
		},
		impact: {
			trigger_level_pct: i.trigger_level_pct ?? 10,
			polarity: i.polarity === 'negative' ? 'negative' : 'positive',
			pretrigger_pct: i.pretrigger_pct ?? 5,
			holdoff_blocks: i.holdoff_blocks ?? 1,
			double_hit_pct: i.double_hit_pct ?? 5,
			tip_warn_db: i.tip_warn_db ?? 20,
			accept: i.accept === 'manual' ? 'manual' : 'auto',
			force_window: { on: i.force_window?.on ?? true, width_pct: i.force_window?.width_pct ?? 10, fall_pct: i.force_window?.fall_pct ?? 10 },
			exponential_window: { on: i.exponential_window?.on ?? false, end_pct: i.exponential_window?.end_pct ?? 10 },
			auto_advance: i.auto_advance ?? true,
			target: i.target ? { on: true, peak: i.target.peak, tolerance_pct: i.target.tolerance_pct ?? 20 } : { on: false, peak: NaN, tolerance_pct: 20 }
		},
		shaker: s
			? {
					drives: (s.drives ?? []).map((x) => ({ row: x.row, level_vrms: x.level_vrms ?? 0.05 })),
					source: s.source ?? 'burst_random',
					burst_pct: s.burst_pct ?? 50,
					ramp_s: s.ramp_s ?? 1,
					ceiling_v: s.ceiling_v ?? 1,
					delay_frames: s.delay_frames ?? 2,
					overlap_pct: s.overlap_pct ?? 50,
					seed: s.seed ?? 1
				}
			: defaultShaker(),
		capture: { run_name: c.capture?.run_name ?? 'modal_test' },
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
// The rows

/** The table's enabled input rows by analysis role, and its enabled drive rows. */
export function modalRows(rows: ChannelRow[]): { references: ChannelRow[]; responses: ChannelRow[]; monitors: ChannelRow[]; unassigned: ChannelRow[]; drives: ChannelRow[] } {
	const on = rows.filter((r) => r.enabled);
	const inputs = on.filter((r) => r.role !== 'drive');
	return {
		references: inputs.filter((r) => r.analysis_role === 'reference'),
		responses: inputs.filter((r) => r.analysis_role === 'response'),
		monitors: inputs.filter((r) => r.analysis_role === 'monitor'),
		unassigned: inputs.filter((r) => !r.analysis_role),
		drives: on.filter((r) => r.role === 'drive')
	};
}

/**
 * The draft in a new mode. A shaker's force row does not rove, so a plan that roves a reference row
 * (a roving hammer survey) cannot stay when the mode becomes shaker: it is cleared, and the words
 * say so. Returns the note, or null when the plan stays as it was.
 */
export function setModalMode(d: ModalDraft, mode: ModalMode, rows: ChannelRow[]): string | null {
	if (d.mode === mode) return null;
	d.mode = mode;
	if (mode !== 'shaker' || !d.roving.on) return null;
	const refs = new Set(modalRows(rows).references.map((r) => r.index));
	const moved = d.roving.channels.filter((k) => refs.has(k));
	if (!moved.length) return null;
	const base = defaultModalSetup().roving;
	d.roving = { ...base, fill: { ...base.fill } };
	const which = moved.length === 1 ? `row ${moved[0]}, a reference` : `rows ${moved.join(', ')}, references`;
	return `The roving plan was cleared: it moved ${which}, and a shaker's force row does not rove. In Roving, choose the responses that rove and their points.`;
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

export type ModalSection = 'station' | 'dofs' | 'acquisition' | 'roving' | 'excitation' | 'capture';

export const MODAL_SECTIONS: { id: ModalSection; label: string }[] = [
	{ id: 'station', label: 'Station & Channels' },
	{ id: 'dofs', label: 'DOFs' },
	{ id: 'acquisition', label: 'Mode & Acquisition' },
	{ id: 'roving', label: 'Roving' },
	{ id: 'excitation', label: 'Impact' },
	{ id: 'capture', label: 'Records' }
];

/** The sections, the excitation one named for the mode. */
export function modalSections(mode: ModalMode): { id: ModalSection; label: string }[] {
	return MODAL_SECTIONS.map((s) => (s.id === 'excitation' ? { ...s, label: mode === 'shaker' ? 'Shaker' : 'Impact' } : s));
}

/**
 * The editor's key for a service finding. A configuration item's `field` is the path inside the
 * configuration, with or without its `config.` head; a finding on a row is the DOF panel's.
 */
/** The plan's refusals, about the table's references and responses: the DOF panel's. */
const DOF_REASONS = new Set(['no_reference', 'no_response', 'reference_count']);

export function modalItemKey(item: { field?: string | null; channel?: number | null; reason: string }): string {
	if (DOF_REASONS.has(item.reason)) return 'dofs';
	if (item.field) {
		const path = item.field.replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
		if (path === 'channels' || /^channels\./.test(path)) return 'dofs';
		if (/^metadata(\.|$)/.test(path)) return 'metadata';
		return path;
	}
	if (item.channel !== undefined && item.channel !== null) return 'dofs';
	return 'setup';
}

export function modalByKey(items: { field?: string | null; channel?: number | null; reason: string; message: string }[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = modalItemKey(it);
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

/** The section a key belongs to. */
export function modalSectionOf(key: string): ModalSection {
	if (/^(station|table)/.test(key)) return 'station';
	if (/^dofs/.test(key)) return 'dofs';
	if (/^roving(\.|$)/.test(key)) return 'roving';
	if (/^(impact|shaker)(\.|$)/.test(key)) return 'excitation';
	if (/^(capture|metadata|title|operator|article|notes|specification|record)(\.|$)/.test(key)) return 'capture';
	return 'acquisition';
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
 * Fields that hold no number, points that cannot be read, a roving plan with no channel, a shaker
 * test with no drive, a run folder not named. Every range, the plan's DOFs, the windows against
 * the block and the refusals for several shakers are the control service's to judge in the dry
 * run.
 */
export function localErrors(d: ModalDraft): FieldErrors {
	const out: FieldErrors = {};
	need(out, 'fs_hz', d.fs_hz);
	need(out, 'averages', d.averages);
	if (d.roving.on) {
		if (!d.roving.channels.length) add(out, 'roving.channels', 'Choose the channels that rove.');
		if (d.roving.how === 'list') {
			const p = parsePoints(d.roving.text);
			for (const m of p.problems) add(out, 'roving.points', m);
			if (!p.points.length && !p.problems.length) add(out, 'roving.points', 'Type or paste the points, one a line.');
		} else {
			need(out, 'roving.points.start', d.roving.fill.start);
			need(out, 'roving.points.step', d.roving.fill.step);
			need(out, 'roving.points.count', d.roving.fill.count);
		}
	}
	if (d.mode === 'impact') {
		const i = d.impact;
		need(out, 'impact.trigger_level_pct', i.trigger_level_pct);
		need(out, 'impact.pretrigger_pct', i.pretrigger_pct);
		need(out, 'impact.holdoff_blocks', i.holdoff_blocks);
		need(out, 'impact.double_hit_pct', i.double_hit_pct);
		need(out, 'impact.tip_warn_db', i.tip_warn_db);
		if (i.force_window.on) {
			need(out, 'impact.force_window.width_pct', i.force_window.width_pct);
			need(out, 'impact.force_window.fall_pct', i.force_window.fall_pct);
		}
		if (i.exponential_window.on) need(out, 'impact.exponential_window.end_pct', i.exponential_window.end_pct);
		if (i.target.on) {
			if (!Number.isFinite(i.target.peak)) add(out, 'impact.target.peak', 'Give the target peak force, or take it from a hit.');
			need(out, 'impact.target.tolerance_pct', i.target.tolerance_pct);
		}
	} else {
		const s = d.shaker;
		if (!s.drives.length) add(out, 'shaker.drives', 'Choose at least one drive row.');
		s.drives.forEach((x, k) => need(out, `shaker.drives.${k}.level_vrms`, x.level_vrms));
		need(out, 'shaker.ceiling_v', s.ceiling_v);
		need(out, 'shaker.ramp_s', s.ramp_s);
		need(out, 'shaker.delay_frames', s.delay_frames);
		need(out, 'shaker.seed', s.seed);
		if (s.source === 'random') need(out, 'shaker.overlap_pct', s.overlap_pct);
		if (s.source === 'burst_random' || s.source === 'burst_chirp') need(out, 'shaker.burst_pct', s.burst_pct);
	}
	if (!d.capture.run_name.trim()) add(out, 'capture.run_name', 'Name the run folder.');
	if (d.record.clause.trim() && !d.record.document.trim()) add(out, 'specification.document', 'Name the document this clause is from.');
	return out;
}

/** The service's items, keyed for the editor. */
export function itemsByKey(items: ReportItem[]): FieldErrors {
	return modalByKey(items as unknown as { field?: string | null; channel?: number | null; reason: string; message: string }[]);
}
