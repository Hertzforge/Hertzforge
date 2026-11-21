/**
 * The MIMO Mixed Mode setup as the operator edits it, and its conversion to the wire's
 * `MimoMixedConfig` (`$lib/protocol/mimoMixed`). Pure functions only: the service computes every
 * number about the test (the reference matrix, the grid, the prediction and the budget forecast),
 * and this module only carries the operator's choices.
 *
 * The channels are the channel table's: every enabled control row is a control unless it is taken
 * out; every enabled drive row is a drive, in the table's order. The floor's reference is MIMO
 * Random's: one breakpoint table per control and one coherence and phase between every pair, built
 * by the service, or a reference file imported by path. Each tone and narrowband states its numbers
 * per control; the lists follow the controls as rows are switched on and off.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. Each drive's ceilings have no default anywhere: the setup asks for them.
 */

import type { BandDb, ChannelRow, ReportItem } from '$lib/protocol/types';
import type { LawEntry, MimoBuildBreakpointsParams, MimoReference } from '$lib/protocol/mimo';
import {
	MIMO_MIXED_DEFAULTS,
	MIMO_MIXED_MAX_BANDS,
	MIMO_MIXED_MAX_CONTROLS,
	MIMO_MIXED_MAX_DRIVES,
	MIMO_MIXED_MAX_LIMITS,
	MIMO_MIXED_MAX_TONES,
	type MimoMixedBandLaw,
	type MimoMixedBandPart,
	type MimoMixedCeilingRule,
	type MimoMixedConfig,
	type MimoMixedFillRule,
	type MimoMixedPart,
	type MimoMixedReportItem,
	type MimoMixedTonePart,
	type PerControl
} from '$lib/protocol/mimoMixed';
import { lawWireParams, type LawDraft } from '../law/lawForm';
import { breakpointsWire, defaultTable } from '../mimo/mimoSetup';
import { newSchedule, scheduleOf, type ScheduleDraft, type WindowDraft } from '../mixed/mixedSetup';
import { defaultRecord, followTitle, titleNumber, type BreakpointDraft, type FieldErrors, type LevelDraft, type RecordDraft } from '../setup';

// ---------------------------------------------------------------------------------------------
// The draft

export interface MmixControlDraft {
	/** The channel table row's `index`: an enabled control row. */
	channel: number;
	/** The floor's PSD at this control, full level. */
	rows: BreakpointDraft[];
	alarm_db: BandDb;
	abort_db: BandDb;
}

export interface MmixDriveDraft {
	channel: number;
	/** Volts rms on the random part; NaN until given. */
	ceiling_vrms: number;
	/** Volts peak on the summed drive, the budget the floor and the tones share; NaN until given. */
	peak_v: number;
	/** Null: no rms abort of its own. */
	abort_vrms: number | null;
}

export interface MmixLimitDraft {
	channel: number;
	/** The PSD ceiling at full level, units squared per hertz. */
	ceiling: number;
	abort_margin_db: number;
}

export interface MmixToneDraft {
	id: string;
	name: string;
	enabled: boolean;
	window: WindowDraft;
	schedule: ScheduleDraft;
	/** Peak amplitude at full level, g, one per control. */
	amp_g: number[];
	/** False: the tone law chooses the phases. */
	phases_stated: boolean;
	/** Degrees against the first control, one per control (the first is 0). */
	phase_deg: number[];
	alarm_db: BandDb;
	abort_db: BandDb;
}

export interface MmixBandDraft {
	id: string;
	name: string;
	enabled: boolean;
	window: WindowDraft;
	schedule: ScheduleDraft;
	width: { by: 'hz' | 'share'; hz: number; share: number };
	/** `raise`: the floor lifted by dB per control. `added`: a density per control and one coherence. */
	statement: { kind: 'raise' | 'added'; db: number[]; level_g2_hz: number[]; coherence: number };
	/** False: the floor's bands. */
	own_bands: boolean;
	alarm_db: BandDb;
	abort_db: BandDb;
}

export interface MmixDraft {
	title: string;
	controls: MmixControlDraft[];
	/** Enabled control rows the operator took out of the controls, by table index. */
	notControls: number[];
	drives: MmixDriveDraft[];
	limits: MmixLimitDraft[];
	reference: {
		method: 'breakpoints' | 'import';
		importPath: string;
		unit: string;
		/** One squared coherence and one phase between every pair of controls. */
		coherence: number;
		phase_deg: number;
		acceptRepair: boolean;
	};
	tones: MmixToneDraft[];
	bands: MmixBandDraft[];
	exclusion_hz: number;
	tracking_hz: number;
	estimation: { fs_hz: number; n_pts: number; frames_per_loop: number };
	identification: { level_db: number; frames: number; coherence_floor: number; condition_limit: number; review: boolean };
	schedule: { mode: 'auto' | 'manual'; levels: LevelDraft[]; full_level_s: number };
	control: {
		part_ramp_s: number;
		floor_settle_s: number;
		level_ramp_s: number;
		shutdown_ramp_s: number;
		soft_start_db: number;
		crest_factor: number;
		cap_up_db: number;
		cap_down_db: number;
		law_budget: number;
		law_misses: number;
	};
	protections: { envelope_abort_lines: number; divergence_loops: number; overload_fraction: number };
	laws: { floor: LawDraft; tones: LawDraft; bands: MimoMixedBandLaw; ceiling_rule: MimoMixedCeilingRule; fill_rule: MimoMixedFillRule };
	record: RecordDraft;
}

export const BAND_LAW_WORDS: Record<MimoMixedBandLaw, { label: string; help: string }> = {
	auto: { label: 'Automatic', help: 'The service picks for each band: one servo for a band stated as a raise, one per control for a band stated as an added level.' },
	scalar_servo: { label: 'One Servo per Band', help: 'One gain for the whole band, the same at every control: the band keeps the floor\'s shape between the controls.' },
	per_control: { label: 'One Servo per Control', help: 'A gain for each control: each control is held to its own level in the band.' }
};

export const CEILING_RULE_WORDS: Record<MimoMixedCeilingRule, { label: string; help: string }> = {
	common_scale: { label: 'Common Scale', help: 'When a drive runs out of room, every tone is scaled down by the same factor, so the tones keep their proportions.' },
	in_turn: { label: 'In Turn', help: 'Each tone is judged in order against what the others play, as single-output Mixed Mode does. The outcome depends on the order of the tones.' }
};

export const FILL_RULE_WORDS: Record<MimoMixedFillRule, { label: string; help: string }> = {
	no_error: { label: 'No Error', help: 'On lines a tone or a servoed band occupies, the floor\'s law is shown the reference as measured, so it sees no error there and its drive stays as it was.' },
	model_fill: { label: 'Fill From the Model', help: 'On those lines, the floor\'s law is shown what the model predicts the floor alone would read.' }
};

export function newControl(channel: number): MmixControlDraft {
	return { channel, rows: defaultTable(), alarm_db: [...MIMO_MIXED_DEFAULTS.floor_alarm_db] as BandDb, abort_db: [...MIMO_MIXED_DEFAULTS.floor_abort_db] as BandDb };
}

const fill = (n: number, v: number): number[] => Array.from({ length: n }, () => v);

export function newTone(id: string, n: number, controls: number, freq = 100): MmixToneDraft {
	return {
		id,
		name: `Tone ${n}`,
		enabled: true,
		window: { on_s: 0, off_s: null },
		schedule: newSchedule('fixed', freq),
		amp_g: fill(controls, 1),
		phases_stated: true,
		phase_deg: fill(controls, 0),
		alarm_db: [...MIMO_MIXED_DEFAULTS.tone_alarm_db] as BandDb,
		abort_db: [...MIMO_MIXED_DEFAULTS.tone_abort_db] as BandDb
	};
}

export function newBand(id: string, n: number, controls: number, freq = 250): MmixBandDraft {
	return {
		id,
		name: `Narrowband ${n}`,
		enabled: true,
		window: { on_s: 0, off_s: null },
		schedule: newSchedule('fixed', freq),
		width: { by: 'hz', hz: 20, share: 0.05 },
		statement: { kind: 'raise', db: fill(controls, 10), level_g2_hz: fill(controls, 0.01), coherence: 0 },
		own_bands: false,
		alarm_db: [...MIMO_MIXED_DEFAULTS.floor_alarm_db] as BandDb,
		abort_db: [...MIMO_MIXED_DEFAULTS.floor_abort_db] as BandDb
	};
}

/** An id not yet used: `t1`, `t2` ... or `b1`, `b2` ... */
export function freeId(d: Pick<MmixDraft, 'tones' | 'bands'>, prefix: 't' | 'b'): string {
	const used = new Set(['floor', ...d.tones.map((t) => t.id), ...d.bands.map((b) => b.id)]);
	for (let n = 1; ; n++) if (!used.has(`${prefix}${n}`)) return `${prefix}${n}`;
}

/** A floor with one fixed tone, ready to run on the simulator once the drives' ceilings are given. */
export function defaultMmixSetup(): MmixDraft {
	return {
		title: '',
		controls: [],
		notControls: [],
		drives: [],
		limits: [],
		reference: { method: 'breakpoints', importPath: '', unit: 'g', coherence: 0, phase_deg: 0, acceptRepair: false },
		tones: [newTone('t1', 1, 0, 120)],
		bands: [],
		exclusion_hz: MIMO_MIXED_DEFAULTS.exclusion_hz,
		tracking_hz: MIMO_MIXED_DEFAULTS.tracking_hz,
		estimation: { fs_hz: 4096, n_pts: 2048, frames_per_loop: 4 },
		identification: { level_db: -20, frames: 40, coherence_floor: 0.5, condition_limit: 1000, review: true },
		schedule: {
			mode: 'auto',
			levels: [
				{ db: -12, dwell_s: 10 },
				{ db: -6, dwell_s: 10 },
				{ db: 0, dwell_s: 0 }
			],
			full_level_s: 120
		},
		control: { part_ramp_s: 3, floor_settle_s: 30, level_ramp_s: 3, shutdown_ramp_s: 3, soft_start_db: 9, crest_factor: 3, cap_up_db: 3.5, cap_down_db: 6, law_budget: 0.25, law_misses: 3 },
		protections: { envelope_abort_lines: 5, divergence_loops: 5, overload_fraction: 0.9 },
		laws: {
			floor: { id: MIMO_MIXED_DEFAULTS.floor_law, params: {} },
			tones: { id: MIMO_MIXED_DEFAULTS.tone_law, params: {} },
			bands: 'auto',
			ceiling_rule: 'common_scale',
			fill_rule: 'no_error'
		},
		record: defaultRecord()
	};
}

const arr = <T>(v: unknown, fallback: T[]): T[] => (Array.isArray(v) ? (v as T[]) : fallback);
const obj = (v: unknown): Record<string, unknown> => (v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {});
const oneOf = <T extends string>(v: unknown, list: readonly T[], fallback: T): T => (list.includes(v as T) ? (v as T) : fallback);

/** A draft read back from storage, every section filled from the defaults where it is missing. */
export function mergeMmixSetup(saved: Partial<MmixDraft> | null): MmixDraft {
	const base = defaultMmixSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as MmixDraft;
	out.controls = arr<MmixControlDraft>(saved.controls, []).map((c) => ({ ...newControl(c?.channel ?? 0), ...obj(c) }) as MmixControlDraft);
	out.notControls = arr(saved.notControls, []);
	out.drives = arr(saved.drives, []);
	out.limits = arr(saved.limits, []);
	out.reference = { ...base.reference, ...obj(saved.reference) };
	out.tones = arr<MmixToneDraft>(saved.tones, base.tones).map((t, i) => {
		const n = newTone(t?.id ?? `t${i + 1}`, i + 1, 0);
		return { ...n, ...obj(t), window: { ...n.window, ...obj(t?.window) }, schedule: { ...n.schedule, ...obj(t?.schedule) } } as MmixToneDraft;
	});
	out.bands = arr<MmixBandDraft>(saved.bands, []).map((b, i) => {
		const n = newBand(b?.id ?? `b${i + 1}`, i + 1, 0);
		return { ...n, ...obj(b), window: { ...n.window, ...obj(b?.window) }, schedule: { ...n.schedule, ...obj(b?.schedule) }, width: { ...n.width, ...obj(b?.width) }, statement: { ...n.statement, ...obj(b?.statement) } } as MmixBandDraft;
	});
	out.estimation = { ...base.estimation, ...obj(saved.estimation) };
	out.identification = { ...base.identification, ...obj(saved.identification) };
	const s = obj(saved.schedule);
	out.schedule = { ...base.schedule, ...s, levels: arr<LevelDraft>(s.levels, base.schedule.levels) } as MmixDraft['schedule'];
	if (!out.schedule.levels.length) out.schedule.levels = base.schedule.levels;
	out.control = { ...base.control, ...obj(saved.control) };
	out.protections = { ...base.protections, ...obj(saved.protections) };
	const l = obj(saved.laws);
	out.laws = {
		floor: { ...base.laws.floor, ...obj(l.floor) } as LawDraft,
		tones: { ...base.laws.tones, ...obj(l.tones) } as LawDraft,
		bands: oneOf(l.bands, ['auto', 'scalar_servo', 'per_control'] as const, 'auto'),
		ceiling_rule: oneOf(l.ceiling_rule, ['common_scale', 'in_turn'] as const, 'common_scale'),
		fill_rule: oneOf(l.fill_rule, ['no_error', 'model_fill'] as const, 'no_error')
	};
	out.record = { ...base.record, ...obj(saved.record) };
	return followTitle(out, mmixTitle);
}

// ---------------------------------------------------------------------------------------------
// The channels

/** A list fitted to `n` entries: kept where it reaches, the first entry (or `v`) past it. */
export function fitList(list: number[], n: number, v: number): number[] {
	return Array.from({ length: n }, (_, i) => (i < list.length ? list[i] : (list[0] ?? v)));
}

/** Every per-control list fitted to the controls' count. */
export function fitPerControl(d: MmixDraft): void {
	const n = d.controls.length;
	for (const t of d.tones) {
		if (t.amp_g.length !== n) t.amp_g = fitList(t.amp_g, n, 1);
		if (t.phase_deg.length !== n) t.phase_deg = fitList(t.phase_deg, n, 0);
	}
	for (const b of d.bands) {
		if (b.statement.db.length !== n) b.statement.db = fitList(b.statement.db, n, 10);
		if (b.statement.level_g2_hz.length !== n) b.statement.level_g2_hz = fitList(b.statement.level_g2_hz, n, 0.01);
	}
}

function plainCopy<T>(v: T): T {
	return JSON.parse(JSON.stringify(v, (_k, x) => (typeof x === 'number' && Number.isNaN(x) ? '__NaN__' : x)), (_k, x) => (x === '__NaN__' ? NaN : x)) as T;
}

/**
 * The draft's channels fitted to the table: every enabled control row a control (unless taken
 * out), each keeping its table, a new one starting from the first control's; every enabled drive
 * row a drive in the table's order (its ceilings kept, new ones to be given); limits only on enabled
 * inputs that are not controls. Returns whether anything changed.
 */
export function fitChannels(d: MmixDraft, rows: ChannelRow[]): boolean {
	if (!rows.length) return false;
	const sig = () => JSON.stringify([d.controls.map((c) => c.channel), d.drives.map((x) => x.channel), d.limits.map((l) => l.channel), d.notControls]);
	const before = sig();
	const on = rows.filter((r) => r.enabled);
	const controlRows = on.filter((r) => r.role === 'control').map((r) => r.index);
	d.notControls = d.notControls.filter((i) => controlRows.includes(i));
	const next = controlRows.filter((i) => !d.notControls.includes(i));
	const byChannel = new Map(d.controls.map((c) => [c.channel, c]));
	const first = d.controls[0];
	d.controls = next.map((ch) => byChannel.get(ch) ?? (first ? { ...plainCopy(first), channel: ch } : newControl(ch)));
	const drives = new Map(d.drives.map((x) => [x.channel, x]));
	d.drives = on.filter((r) => r.role === 'drive').map((r) => drives.get(r.index) ?? { channel: r.index, ceiling_vrms: NaN, peak_v: NaN, abort_vrms: null });
	const inputs = new Set(on.filter((r) => r.role !== 'drive' && !next.includes(r.index)).map((r) => r.index));
	d.limits = d.limits.filter((l) => inputs.has(l.channel));
	fitPerControl(d);
	return before !== sig();
}

export function setControl(d: MmixDraft, rows: ChannelRow[], channel: number, on: boolean): void {
	d.notControls = on ? d.notControls.filter((i) => i !== channel) : [...new Set([...d.notControls, channel])];
	if (on) d.limits = d.limits.filter((l) => l.channel !== channel);
	fitChannels(d, rows);
}

export function limitRows(d: MmixDraft, rows: ChannelRow[]): ChannelRow[] {
	const controls = new Set(d.controls.map((c) => c.channel));
	return rows.filter((r) => r.enabled && r.role !== 'drive' && !controls.has(r.index));
}

export const rowLabel = (rows: readonly ChannelRow[], ch: number): string => {
	const r = rows.find((x) => x.index === ch);
	return r?.description.trim() ? r.description.trim() : `Channel ${ch}`;
};

// ---------------------------------------------------------------------------------------------
// Title

export function mmixTitle(d: MmixDraft): string {
	const parts = [d.tones.length ? `${d.tones.length} ${d.tones.length === 1 ? 'tone' : 'tones'}` : '', d.bands.length ? `${d.bands.length} ${d.bands.length === 1 ? 'narrowband' : 'narrowbands'}` : ''].filter(Boolean);
	const size = d.controls.length && d.drives.length ? ` ${d.controls.length} by ${d.drives.length}` : '';
	return `MIMO Mixed${size}${parts.length ? `, ${parts.join(' and ')} on the floor` : ''}`;
}

export function mmixTitleOf(d: MmixDraft): string {
	return d.title.trim() || mmixTitle(d);
}

// ---------------------------------------------------------------------------------------------
// To the protocol

/** The build request for the floor's reference: one table per control, one coherence and phase for every pair. */
export function breakpointParamsOf(d: MmixDraft, labels: string[]): MimoBuildBreakpointsParams {
	const n = d.controls.length;
	const pairs = [];
	if (d.reference.coherence > 0) for (let r = 0; r < n; r++) for (let c = r + 1; c < n; c++) pairs.push({ row: r, col: c, coherence: d.reference.coherence, phase_deg: d.reference.phase_deg });
	return {
		grid: { fs_hz: d.estimation.fs_hz, n_pts: d.estimation.n_pts },
		unit: d.reference.unit,
		tables: d.controls.map((c, i) => ({ label: labels[i] ?? `Control ${c.channel}`, rows: breakpointsWire(c.rows) })),
		pairs,
		unspecified: 'independent',
		alarm_db: [...MIMO_MIXED_DEFAULTS.floor_alarm_db] as BandDb,
		abort_db: [...MIMO_MIXED_DEFAULTS.floor_abort_db] as BandDb,
		repair: d.reference.acceptRepair,
		preview_pair: [0, Math.min(1, Math.max(0, n - 1))]
	};
}

/** A per-control list on the wire: one number when every control has the same. */
export function perControl(list: readonly number[]): PerControl<number> {
	return list.length && list.every((v) => v === list[0]) ? list[0] : [...list];
}

const band = (b: BandDb): BandDb => [b[0], b[1]];

export function toneOf(t: MmixToneDraft): MimoMixedTonePart {
	const same = t.amp_g.length > 0 && t.amp_g.every((v) => v === t.amp_g[0]);
	return {
		kind: 'tone',
		id: t.id,
		name: t.name,
		enabled: t.enabled,
		window: { on_s: t.window.on_s, off_s: t.window.off_s },
		schedule: scheduleOf(t.schedule),
		amplitude: same ? { kind: 'constant', g_pk: t.amp_g[0] } : t.amp_g.map((g) => ({ kind: 'constant' as const, g_pk: g })),
		phases_stated: t.phases_stated,
		phase_deg: t.phases_stated ? [...t.phase_deg] : [],
		alarm_db: band(t.alarm_db),
		abort_db: band(t.abort_db)
	};
}

export function bandPartOf(b: MmixBandDraft): MimoMixedBandPart {
	const s = b.statement;
	return {
		kind: 'narrowband',
		id: b.id,
		name: b.name,
		enabled: b.enabled,
		window: { on_s: b.window.on_s, off_s: b.window.off_s },
		schedule: scheduleOf(b.schedule),
		width: b.width.by === 'hz' ? { kind: 'hz', hz: b.width.hz } : { kind: 'share', share: b.width.share },
		statement: s.kind === 'raise' ? { kind: 'raise', db: perControl(s.db) } : { kind: 'added', level_g2_hz: perControl(s.level_g2_hz), coherence: s.coherence },
		alarm_db: b.own_bands ? band(b.alarm_db) : null,
		abort_db: b.own_bands ? band(b.abort_db) : null
	};
}

export function partsOf(d: MmixDraft): MimoMixedPart[] {
	const sameBands = (xs: BandDb[]): PerControl<BandDb> => (xs.every((x) => x[0] === xs[0][0] && x[1] === xs[0][1]) ? band(xs[0]) : xs.map(band));
	const floor: MimoMixedPart = { kind: 'floor', id: 'floor', name: 'Floor', enabled: true };
	if (d.controls.length) {
		floor.alarm_db = sameBands(d.controls.map((c) => c.alarm_db));
		floor.abort_db = sameBands(d.controls.map((c) => c.abort_db));
	}
	return [floor, ...d.tones.map(toneOf), ...d.bands.map(bandPartOf)];
}

/** The configuration the draft describes, given the reference the service built or read. */
export function toMmixConfig(d: MmixDraft, reference: MimoReference | null, laws: { floor: LawEntry | null; tones: LawEntry | null }): MimoMixedConfig {
	const r = d.record;
	const [document, clause] = [r.document.trim(), r.clause.trim()];
	const last = d.schedule.levels.length - 1;
	const c = d.control;
	return {
		title: mmixTitleOf(d),
		channels: {
			controls: d.controls.map((x) => ({ channel: x.channel, alarm_db: band(x.alarm_db), abort_db: band(x.abort_db) })),
			drives: d.drives.map((x) => ({ channel: x.channel, ceiling_vrms: x.ceiling_vrms, peak_v: x.peak_v, abort_vrms: x.abort_vrms })),
			limits: d.limits.map((l) => ({ channel: l.channel, ceiling: l.ceiling, abort_margin_db: l.abort_margin_db }))
		},
		reference: reference ?? { kind: 'inline', freqs_hz: [], n: d.controls.length, re: [], im: [] },
		parts: partsOf(d),
		exclusion_hz: d.exclusion_hz,
		tracking_hz: d.tracking_hz,
		estimation: { ...d.estimation },
		identification: { ...d.identification },
		schedule: { mode: d.schedule.mode, levels: d.schedule.levels.map((l, i) => (i === last ? { db: l.db } : { db: l.db, dwell_s: l.dwell_s })), full_level_s: d.schedule.full_level_s },
		control: {
			frf: 'hold',
			cap_up_db: c.cap_up_db,
			cap_down_db: c.cap_down_db,
			law_budget: c.law_budget,
			law_misses: c.law_misses,
			level_ramp_s: c.level_ramp_s,
			shutdown_ramp_s: c.shutdown_ramp_s,
			part_ramp_s: c.part_ramp_s,
			floor_settle_s: c.floor_settle_s,
			soft_start_db: c.soft_start_db,
			crest_factor: c.crest_factor
		},
		protections: { ...d.protections },
		law: {
			floor: { id: d.laws.floor.id, params: lawWireParams(laws.floor, d.laws.floor.params) },
			tones: { id: d.laws.tones.id, params: lawWireParams(laws.tones, d.laws.tones.params) },
			bands: { id: d.laws.bands },
			ceiling_rule: { id: d.laws.ceiling_rule },
			fill_rule: { id: d.laws.fill_rule }
		},
		operator: r.operator,
		article: r.article,
		notes: r.notes,
		specification: document ? { document, clause } : null,
		record: { streams: r.streams }
	};
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

export type MmixSection = 'channels' | 'reference' | 'parts' | 'schedule' | 'laws' | 'limits' | 'forecast';

export const MMIX_SECTIONS: { id: MmixSection; label: string }[] = [
	{ id: 'channels', label: 'Station & Channels' },
	{ id: 'reference', label: 'Floor Reference' },
	{ id: 'parts', label: 'Parts' },
	{ id: 'schedule', label: 'Schedule & Identification' },
	{ id: 'laws', label: 'Laws' },
	{ id: 'limits', label: 'Limits & Protections' },
	{ id: 'forecast', label: 'Prediction & Budget' }
];

/** A refusal's name in words, for the list of refusals. */
export const REFUSAL_WORDS: Record<string, string> = {
	no_floor: 'No floor',
	unknown_part_kind: 'Unknown part kind',
	part_out_of_band: 'Part outside the band',
	parts_too_close: 'Parts too close',
	too_many_tones: 'Too many tones',
	too_many_bands: 'Too many narrowbands',
	duplicate_part_id: 'Two parts with one id',
	follow_unknown_part: 'Follows an unknown part',
	follow_self: 'Follows itself',
	follow_loop: 'Followers in a loop',
	schedule_unsupported: 'Schedule not offered',
	bad_part: 'Part not readable',
	bad_config: 'Setting out of range',
	wrong_control_count: 'Wrong number of values per control',
	band_not_additive: 'Band not additive',
	band_matrix_not_built: 'Band as a full matrix: not built',
	law_refuses_tone: 'Tone law refuses the tone',
	budget_cannot_fit: 'Budget cannot fit',
	frf_update_not_built: 'Model update: not built',
	unknown_band_law: 'Unknown band law',
	unknown_ceiling_rule: 'Unknown ceiling rule',
	unknown_fill_rule: 'Unknown fill rule',
	reference_invalid: 'Reference not accepted',
	law_unknown: 'Law not in the catalog',
	too_many_controls: 'Too many controls',
	too_many_drives: 'Too many drives',
	too_many_limits: 'Too many limits',
	out_of_range: 'Out of range',
	sweep_beyond_tracker: 'Sweep faster than the tracker follows'
};

/** Where a refusal is: its part, control or drive, in words. */
export function refusalWhere(item: Pick<MimoMixedReportItem, 'part' | 'control' | 'drive'>, names: { parts: Record<string, string>; controls: string[]; drives: string[] }): string {
	const out: string[] = [];
	if (item.part) out.push(`part ${names.parts[item.part] ?? item.part}`);
	if (item.control !== undefined && item.control !== null) out.push(`control ${names.controls[item.control] ?? item.control + 1}`);
	if (item.drive !== undefined && item.drive !== null) out.push(`drive ${names.drives[item.drive] ?? item.drive + 1}`);
	return out.join(', ');
}

/** The editor's key for a service finding: by its part, drive, control or field. */
export function mmixItemKey(d: Pick<MmixDraft, 'tones' | 'bands'>, item: MimoMixedReportItem | (Pick<ReportItem, 'field' | 'channel' | 'row'> & { reason: string })): string {
	const it = item as MimoMixedReportItem;
	if (it.reason === 'budget_cannot_fit') return 'forecast';
	if (it.reason === 'frf_update_not_built') return 'control.frf';
	if (it.reason === 'unknown_band_law') return 'laws.bands';
	if (it.reason === 'unknown_ceiling_rule') return 'laws.ceiling_rule';
	if (it.reason === 'unknown_fill_rule') return 'laws.fill_rule';
	if (it.reason === 'reference_invalid') return 'reference';
	if (it.part) {
		const t = d.tones.findIndex((x) => x.id === it.part);
		if (t >= 0) return `tones.${t}`;
		const b = d.bands.findIndex((x) => x.id === it.part);
		if (b >= 0) return `bands.${b}`;
		return 'parts';
	}
	if (it.drive !== undefined && it.drive !== null) return `drives.${it.drive}`;
	const f = (it.field ?? '').replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
	if (f) {
		let m = /^law\.(floor|tones)(?:\.(id|params\.(\w+)))?/.exec(f);
		if (m) return m[3] ? `laws.${m[1]}.params.${m[3]}` : `laws.${m[1]}.id`;
		m = /^channels\.drives\.(\d+)/.exec(f);
		if (m) return `drives.${m[1]}`;
		m = /^channels\.limits\.(\d+)/.exec(f);
		if (m) return `limits.${m[1]}`;
		if (/^channels/.test(f)) return 'channels.controls';
		if (/^reference/.test(f)) return 'reference';
		m = /^schedule\.levels\.(\d+)/.exec(f);
		if (m) return `schedule.levels.${m[1]}.db`;
		if (/^law\./.test(f)) return f.replace(/^law\./, 'laws.');
		return f;
	}
	if (it.control !== undefined && it.control !== null) return `controls.${it.control}`;
	if (it.reason.startsWith('law_')) return 'laws.floor.id';
	return 'setup';
}

export function mmixByKey(d: Pick<MmixDraft, 'tones' | 'bands'>, items: readonly MimoMixedReportItem[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = mmixItemKey(d, it);
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

export function mmixSectionOf(key: string): MmixSection {
	if (/^(reference|controls\.\d+\.rows)(\.|$)/.test(key)) return 'reference';
	if (/^(tones|bands|parts|exclusion_hz|tracking_hz)(\.|$)/.test(key)) return 'parts';
	if (/^(schedule|identification|estimation)(\.|$)/.test(key)) return 'schedule';
	if (/^laws(\.|$)/.test(key)) return 'laws';
	if (/^(limits|protections|control)(\.|$)/.test(key)) return 'limits';
	if (/^forecast(\.|$)/.test(key)) return 'forecast';
	return 'channels';
}

// ---------------------------------------------------------------------------------------------
// What the editor can say before anything is sent

const NUMBER = 'Enter a number.';

function add(map: FieldErrors, key: string, message: string) {
	(map[key] ??= []).push(message);
}

function need(map: FieldErrors, key: string, v: number, whole = false) {
	if (!Number.isFinite(v)) add(map, key, whole ? 'Enter a whole number.' : NUMBER);
	else if (whole && !Number.isInteger(v)) add(map, key, 'Enter a whole number.');
}

function scheduleErrors(out: FieldErrors, s: ScheduleDraft, at: string, ids: Set<string>, self: string) {
	if (s.kind === 'fixed') need(out, `${at}.freq_hz`, s.freq_hz);
	else if (s.kind === 'sweep') {
		need(out, `${at}.band_hz`, s.band_hz[0]);
		need(out, `${at}.band_hz`, s.band_hz[1]);
		need(out, `${at}.rate`, s.law === 'log' ? s.oct_per_min : s.hz_per_s);
	} else {
		if (!s.of || !ids.has(s.of)) add(out, `${at}.of`, 'Choose the part it follows.');
		else if (s.of === self) add(out, `${at}.of`, 'A part cannot follow itself.');
		need(out, `${at}.ratio`, s.ratio);
	}
}

/**
 * Fields that hold no number, no control or no drive, a drive's ceilings not given, too many of
 * anything, a part's id not unique, a follower with no leader. Everything else (the reference, the
 * bands' additivity, the budget, each law's acceptance of the tones) is the service's to judge.
 */
export function localErrors(d: MmixDraft): FieldErrors {
	const out: FieldErrors = {};
	if (!d.controls.length) add(out, 'channels.controls', 'Switch on at least one control row in the channel table.');
	if (d.controls.length > MIMO_MIXED_MAX_CONTROLS) add(out, 'channels.controls', `At most ${MIMO_MIXED_MAX_CONTROLS} controls.`);
	if (!d.drives.length) add(out, 'channels.drives', 'Switch on at least one drive row in the channel table.');
	if (d.drives.length > MIMO_MIXED_MAX_DRIVES) add(out, 'channels.drives', `At most ${MIMO_MIXED_MAX_DRIVES} drives.`);
	if (d.limits.length > MIMO_MIXED_MAX_LIMITS) add(out, 'limits', `At most ${MIMO_MIXED_MAX_LIMITS} limit channels.`);
	d.drives.forEach((x, i) => {
		if (!Number.isFinite(x.ceiling_vrms)) add(out, `drives.${i}.ceiling_vrms`, 'Give the rms ceiling in volts. There is no default: it is your amplifier input.');
		if (!Number.isFinite(x.peak_v)) add(out, `drives.${i}.peak_v`, 'Give the peak ceiling in volts: the floor and the tones share it.');
		else if (Number.isFinite(x.ceiling_vrms) && x.peak_v < x.ceiling_vrms) add(out, `drives.${i}.peak_v`, 'The peak ceiling is at least the rms ceiling.');
		if (x.abort_vrms !== null && !Number.isFinite(x.abort_vrms)) add(out, `drives.${i}.abort_vrms`, NUMBER);
	});
	d.limits.forEach((l, i) => {
		need(out, `limits.${i}.ceiling`, l.ceiling);
		need(out, `limits.${i}.abort_margin_db`, l.abort_margin_db);
	});
	if (d.reference.method === 'breakpoints') {
		d.controls.forEach((c, i) => {
			if (c.rows.length < 2) add(out, `controls.${i}.rows`, 'Give at least two rows.');
			c.rows.forEach((r, k) => {
				need(out, `controls.${i}.rows.${k}.freq_hz`, r.freq_hz);
				need(out, `controls.${i}.rows.${k}.${r.kind === 'psd' ? 'psd_g2_hz' : 'slope_db_oct'}`, r.kind === 'psd' ? r.psd_g2_hz : r.slope_db_oct);
			});
		});
		need(out, 'reference.coherence', d.reference.coherence);
		if (d.reference.coherence < 0 || d.reference.coherence > 1) add(out, 'reference.coherence', 'A squared coherence from 0 to 1.');
		need(out, 'reference.phase_deg', d.reference.phase_deg);
	} else if (!d.reference.importPath.trim()) add(out, 'reference', 'Give the reference file\'s full path, then import it.');
	if (d.tones.length > MIMO_MIXED_MAX_TONES) add(out, 'parts', `At most ${MIMO_MIXED_MAX_TONES} tones.`);
	if (d.bands.length > MIMO_MIXED_MAX_BANDS) add(out, 'parts', `At most ${MIMO_MIXED_MAX_BANDS} narrowbands.`);
	const ids = new Set<string>();
	const all = [...d.tones.map((t, i) => ({ id: t.id, at: `tones.${i}` })), ...d.bands.map((b, i) => ({ id: b.id, at: `bands.${i}` }))];
	for (const p of all) {
		if (!/^[A-Za-z0-9_-]{1,32}$/.test(p.id)) add(out, `${p.at}.id`, 'An id of 1 to 32 letters, digits, _ and -.');
		else if (ids.has(p.id) || p.id === 'floor') add(out, `${p.at}.id`, 'Another part has this id.');
		ids.add(p.id);
	}
	d.tones.forEach((t, i) => {
		const at = `tones.${i}`;
		scheduleErrors(out, t.schedule, `${at}.schedule`, ids, t.id);
		t.amp_g.forEach((g, k) => {
			if (!Number.isFinite(g) || g < 0) add(out, `${at}.amp_g.${k}`, 'A peak amplitude of 0 or more.');
		});
		if (t.phases_stated) t.phase_deg.forEach((p, k) => need(out, `${at}.phase_deg.${k}`, p));
		need(out, `${at}.window.on_s`, t.window.on_s);
	});
	d.bands.forEach((b, i) => {
		const at = `bands.${i}`;
		scheduleErrors(out, b.schedule, `${at}.schedule`, ids, b.id);
		need(out, `${at}.width`, b.width.by === 'hz' ? b.width.hz : b.width.share);
		const list = b.statement.kind === 'raise' ? b.statement.db : b.statement.level_g2_hz;
		list.forEach((v, k) => {
			if (!Number.isFinite(v) || (b.statement.kind === 'added' && v < 0)) add(out, `${at}.statement.${k}`, b.statement.kind === 'raise' ? NUMBER : 'A density of 0 or more.');
		});
		if (b.statement.kind === 'added' && !(b.statement.coherence >= 0 && b.statement.coherence <= 1)) add(out, `${at}.statement.coherence`, 'A squared coherence from 0 to 1.');
		need(out, `${at}.window.on_s`, b.window.on_s);
	});
	need(out, 'exclusion_hz', d.exclusion_hz);
	need(out, 'tracking_hz', d.tracking_hz);
	if (Number.isFinite(d.tracking_hz) && d.tracking_hz <= 0) add(out, 'tracking_hz', 'A bandwidth above 0 Hz.');
	need(out, 'estimation.fs_hz', d.estimation.fs_hz);
	need(out, 'estimation.n_pts', d.estimation.n_pts, true);
	need(out, 'estimation.frames_per_loop', d.estimation.frames_per_loop, true);
	for (const k of ['level_db', 'coherence_floor', 'condition_limit'] as const) need(out, `identification.${k}`, d.identification[k]);
	need(out, 'identification.frames', d.identification.frames, true);
	if (!d.schedule.levels.length) add(out, 'schedule.levels', 'Give at least one level.');
	d.schedule.levels.forEach((l, i) => need(out, `schedule.levels.${i}.db`, l.db));
	need(out, 'schedule.full_level_s', d.schedule.full_level_s);
	for (const [k, v] of Object.entries(d.control)) need(out, `control.${k}`, v as number, k === 'law_misses');
	for (const [k, v] of Object.entries(d.protections)) need(out, `protections.${k}`, v as number, k !== 'overload_fraction');
	if (d.record.clause.trim() && !d.record.document.trim()) add(out, 'specification.document', 'Name the document this clause is from.');
	return out;
}

/** The test's band from the floor's tables: lowest to highest breakpoint over every control. */
export function bandOf(d: Pick<MmixDraft, 'controls'>): [number, number] {
	const f = d.controls.flatMap((c) => c.rows.map((r) => r.freq_hz)).filter(Number.isFinite);
	return f.length ? [Math.min(...f), Math.max(...f)] : [NaN, NaN];
}

export function titleHz(v: number): string {
	return Number.isFinite(v) ? `${titleNumber(v)} Hz` : '';
}
