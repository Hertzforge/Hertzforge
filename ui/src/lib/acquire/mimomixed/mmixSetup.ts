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
	defaultAbortVpk,
	MIMO_MIXED_DEFAULTS,
	MIMO_MIXED_MAX_BANDS,
	MIMO_MIXED_MAX_CONTROLS,
	MIMO_MIXED_MAX_DRIVES,
	MIMO_MIXED_MAX_LIMITS,
	MIMO_MIXED_MAX_TONES,
	type MimoMixedBandBody,
	type MimoMixedBandLaw,
	type MimoMixedCeilingRule,
	type MimoMixedConfig,
	type MimoMixedFillRule,
	type MimoMixedFloorBody,
	type MimoMixedPart,
	type MimoMixedReportItem,
	type MimoMixedSchedule,
	type MimoMixedToneBody,
	type PerControl
} from '$lib/protocol/mimoMixed';
import { lawWireParams, type LawDraft } from '../law/lawForm';
import { breakpointsWire, defaultTable } from '../mimo/mimoSetup';
import { newSchedule, type ScheduleDraft, type WindowDraft } from '../mixed/mixedSetup';
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
	/** Null: the service's, the rms ceiling plus 1 dB. */
	abort_vrms: number | null;
	/** The summed drive's peak abort, V, above the peak ceiling; null: the peak ceiling plus 1 dB. */
	abort_vpk: number | null;
	/** The highest rms the identification may use on this output, V. */
	pretest_max_vrms: number;
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
	/** False: the service's band defaults, 3 dB alarm and 6 dB abort. */
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
	estimation: { fs_hz: number; n_pts: number; frames_per_loop: number; memory_loops: number };
	/** The identification's level search: from `start_vrms` up to its level below the reference, within `tolerance_db`. */
	identification: { below_reference_db: number; frames: number; start_vrms: number; tolerance_db: number; max_steps: number; noise_frames: number };
	/** The gates the identification must pass before the run holds for review. */
	gates: { coherence_threshold: number; condition_threshold: number };
	schedule: { mode: 'auto' | 'manual'; levels: LevelDraft[]; full_level_s: number };
	control: {
		part_ramp_s: number;
		floor_settle_s: number;
		level_ramp_s: number;
		shutdown_ramp_s: number;
		soft_start_db: number;
		soft_start_relax_db: number;
		crest_factor: number;
		cap_up_db: number;
		cap_down_db: number;
	};
	protections: { envelope_abort_lines: number; divergence_loops: number; divergence_floor_db: number; overload_fraction: number; settle_loops: number };
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

/** A part's schedule standing still at `freq`. Every sweep here ends: switched to a sweep, it ends after its sweeps. */
export function mmixSchedule(freq: number): ScheduleDraft {
	return { ...newSchedule('fixed', freq), end_by: 'sweeps' };
}

/** A new drive: its ceilings to be given, its aborts the service's. */
export function newDrive(channel: number): MmixDriveDraft {
	return { channel, ceiling_vrms: NaN, peak_v: NaN, abort_vrms: null, abort_vpk: null, pretest_max_vrms: 0.2 };
}

/** The peak abort a drive runs with: its own, or the peak ceiling plus 1 dB. NaN until the peak ceiling is given. */
export function abortVpkOf(x: Pick<MmixDriveDraft, 'peak_v' | 'abort_vpk'>): number {
	return x.abort_vpk ?? defaultAbortVpk(x.peak_v);
}

export function newTone(id: string, n: number, controls: number, freq = 100): MmixToneDraft {
	return {
		id,
		name: `Tone ${n}`,
		enabled: true,
		window: { on_s: 0, off_s: null },
		schedule: mmixSchedule(freq),
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
		schedule: mmixSchedule(freq),
		width: { by: 'hz', hz: 20, share: 0.05 },
		statement: { kind: 'raise', db: fill(controls, 10), level_g2_hz: fill(controls, 0.01), coherence: 0 },
		own_bands: false,
		alarm_db: [...MIMO_MIXED_DEFAULTS.band_alarm_db] as BandDb,
		abort_db: [...MIMO_MIXED_DEFAULTS.band_abort_db] as BandDb
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
		// Lines 1 Hz apart: the default 2 Hz exclusion around a tone covers the two lines the controller needs.
		estimation: { fs_hz: 4096, n_pts: 4096, frames_per_loop: 8, memory_loops: 4 },
		identification: { below_reference_db: 20, frames: 64, start_vrms: 0.01, tolerance_db: 1, max_steps: 8, noise_frames: 32 },
		gates: { coherence_threshold: 0.5, condition_threshold: 1000 },
		schedule: {
			mode: 'auto',
			levels: [
				{ db: -12, dwell_s: 10 },
				{ db: -6, dwell_s: 10 },
				{ db: 0, dwell_s: 0 }
			],
			full_level_s: 120
		},
		control: { part_ramp_s: 3, floor_settle_s: 30, level_ramp_s: 3, shutdown_ramp_s: 3, soft_start_db: 9, soft_start_relax_db: 6, crest_factor: 3, cap_up_db: 3.5, cap_down_db: 6 },
		protections: { envelope_abort_lines: 10, divergence_loops: 5, divergence_floor_db: 3, overload_fraction: 0.9, settle_loops: 1 },
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
/** `base`'s members, each taken from `saved` where it has it: a member the setup no longer has is dropped. */
const pick = <T extends object>(base: T, saved: unknown): T => {
	const o = obj(saved);
	return Object.fromEntries(Object.entries(base).map(([k, v]) => [k, k in o ? o[k] : v])) as T;
};
/** A sweep saved with no end ends after its sweeps: the controller's sweeps always end. */
const everyEnds = (s: ScheduleDraft): ScheduleDraft => (s.end_by === 'none' ? { ...s, end_by: 'sweeps' } : s);
const oneOf = <T extends string>(v: unknown, list: readonly T[], fallback: T): T => (list.includes(v as T) ? (v as T) : fallback);

/** A draft read back from storage, every section filled from the defaults where it is missing. */
export function mergeMmixSetup(saved: Partial<MmixDraft> | null): MmixDraft {
	const base = defaultMmixSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as MmixDraft;
	out.controls = arr<MmixControlDraft>(saved.controls, []).map((c) => ({ ...newControl(c?.channel ?? 0), ...obj(c) }) as MmixControlDraft);
	out.notControls = arr(saved.notControls, []);
	out.drives = arr<MmixDriveDraft>(saved.drives, []).map((x) => pick(newDrive(x?.channel ?? 0), x));
	out.limits = arr(saved.limits, []);
	out.reference = { ...base.reference, ...obj(saved.reference) };
	out.tones = arr<MmixToneDraft>(saved.tones, base.tones).map((t, i) => {
		const n = newTone(t?.id ?? `t${i + 1}`, i + 1, 0);
		return { ...n, ...obj(t), window: { ...n.window, ...obj(t?.window) }, schedule: everyEnds({ ...n.schedule, ...obj(t?.schedule) }) } as MmixToneDraft;
	});
	out.bands = arr<MmixBandDraft>(saved.bands, []).map((b, i) => {
		const n = newBand(b?.id ?? `b${i + 1}`, i + 1, 0);
		return { ...n, ...obj(b), window: { ...n.window, ...obj(b?.window) }, schedule: everyEnds({ ...n.schedule, ...obj(b?.schedule) }), width: { ...n.width, ...obj(b?.width) }, statement: { ...n.statement, ...obj(b?.statement) } } as MmixBandDraft;
	});
	out.estimation = pick(base.estimation, saved.estimation);
	out.identification = pick(base.identification, saved.identification);
	out.gates = pick(base.gates, saved.gates);
	const s = obj(saved.schedule);
	out.schedule = { ...base.schedule, ...s, levels: arr<LevelDraft>(s.levels, base.schedule.levels) } as MmixDraft['schedule'];
	if (!out.schedule.levels.length) out.schedule.levels = base.schedule.levels;
	out.control = pick(base.control, saved.control);
	out.protections = pick(base.protections, saved.protections);
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
	d.drives = on.filter((r) => r.role === 'drive').map((r) => drives.get(r.index) ?? newDrive(r.index));
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

/**
 * A part's schedule in the controller's form. A fixed part stands for its seconds or cycles (or
 * the whole window); a sweep ends after its sweeps, its duration or whichever comes first.
 */
export function mmixScheduleOf(s: ScheduleDraft): MimoMixedSchedule {
	if (s.kind === 'follow') return { kind: 'follow', of: s.of, ratio: s.ratio };
	if (s.kind === 'fixed') {
		const out: MimoMixedSchedule = { kind: 'fixed', hz: s.freq_hz };
		if (s.dwell.by === 'seconds') out.for_s = s.dwell.seconds;
		if (s.dwell.by === 'cycles') out.for_cycles = s.dwell.cycles;
		return out;
	}
	const target: Extract<MimoMixedSchedule, { kind: 'sweep' }>['target'] =
		s.end_by === 'duration' ? { kind: 'duration', s: s.duration_s } : s.end_by === 'both' ? { kind: 'whichever_first', sweeps: s.sweeps, duration_s: s.duration_s } : { kind: 'sweeps', n: s.sweeps };
	return {
		kind: 'sweep',
		lo_hz: s.band_hz[0],
		hi_hz: s.band_hz[1],
		start_hz: s.start_hz ?? s.band_hz[0],
		direction: s.direction,
		law: s.law === 'log' ? 'log_oct_per_min' : 'linear_hz_per_s',
		rate: s.law === 'log' ? s.oct_per_min : s.hz_per_s,
		target
	};
}

const windowOf = (w: WindowDraft) => (w.off_s === null ? { on_s: w.on_s } : { on_s: w.on_s, off_s: w.off_s });

export function toneOf(t: MmixToneDraft): MimoMixedPart<MimoMixedToneBody> {
	const same = t.amp_g.length > 0 && t.amp_g.every((v) => v === t.amp_g[0]);
	const body: MimoMixedToneBody = {
		kind: 'tone',
		schedule: mmixScheduleOf(t.schedule),
		amplitude: same ? { kind: 'constant', g_pk: t.amp_g[0] } : t.amp_g.map((g) => ({ kind: 'constant' as const, g_pk: g })),
		phases_stated: t.phases_stated,
		alarm_db: band(t.alarm_db),
		abort_db: band(t.abort_db)
	};
	if (t.phases_stated) body.phase_deg = [...t.phase_deg];
	return { id: t.id, name: t.name, enabled: t.enabled, window: windowOf(t.window), body };
}

export function bandPartOf(b: MmixBandDraft): MimoMixedPart<MimoMixedBandBody> {
	const s = b.statement;
	const body: MimoMixedBandBody = {
		kind: 'narrowband',
		schedule: mmixScheduleOf(b.schedule),
		width: b.width.by === 'hz' ? { kind: 'hz', hz: b.width.hz } : { kind: 'share', share: b.width.share },
		statement: s.kind === 'raise' ? { kind: 'raise', db: perControl(s.db) } : { kind: 'added', level_g2_hz: perControl(s.level_g2_hz), coherence: s.coherence }
	};
	// no bands of its own: the members are left out, and the service's 3 and 6 dB hold
	if (b.own_bands) {
		body.alarm_db = band(b.alarm_db);
		body.abort_db = band(b.abort_db);
	}
	return { id: b.id, name: b.name, enabled: b.enabled, window: windowOf(b.window), body };
}

export function partsOf(d: MmixDraft): MimoMixedPart[] {
	const sameBands = (xs: BandDb[]): PerControl<BandDb> => (xs.every((x) => x[0] === xs[0][0] && x[1] === xs[0][1]) ? band(xs[0]) : xs.map(band));
	const body: MimoMixedFloorBody = { kind: 'floor' };
	if (d.controls.length) {
		body.alarm_db = sameBands(d.controls.map((c) => c.alarm_db));
		body.abort_db = sameBands(d.controls.map((c) => c.abort_db));
	}
	return [{ id: 'floor', name: 'Floor', enabled: true, window: { on_s: 0 }, body }, ...d.tones.map(toneOf), ...d.bands.map(bandPartOf)];
}

/**
 * The level schedule in the controller's form: the levels are the rungs, the first is where the
 * run starts; each rung's dwell is its own, the last (full level) taking the full-level time.
 */
export function levelScheduleOf(d: Pick<MmixDraft, 'schedule'>): MimoMixedConfig['schedule'] {
	const levels = d.schedule.levels;
	const dwells = levels.slice(0, -1).map((l) => l.dwell_s);
	return { mode: d.schedule.mode, start_db: levels[0]?.db ?? 0, table_db: levels.map((l) => l.db), dwell_s: dwells.length ? dwells : [0] };
}

/** The configuration the draft describes, given the reference the service built or read. */
export function toMmixConfig(d: MmixDraft, reference: MimoReference | null, laws: { floor: LawEntry | null; tones: LawEntry | null }): MimoMixedConfig {
	const r = d.record;
	const [document, clause] = [r.document.trim(), r.clause.trim()];
	const c = d.control;
	const out: MimoMixedConfig = {
		title: mmixTitleOf(d),
		operator: r.operator,
		article: r.article,
		notes: r.notes,
		controls: d.controls.map((x) => x.channel),
		drives: d.drives.map((x) => {
			const w: MimoMixedConfig['drives'][number] = { channel_index: x.channel, ceiling_vrms: x.ceiling_vrms, peak_v: x.peak_v, pretest_max_vrms: x.pretest_max_vrms };
			if (x.abort_vrms !== null) w.abort_vrms = x.abort_vrms;
			if (x.abort_vpk !== null) w.abort_vpk = x.abort_vpk;
			return w;
		}),
		limits: d.limits.map((l) => ({ channel_index: l.channel, ceiling_psd: [l.ceiling], abort_margin_db: l.abort_margin_db })),
		reference: reference ?? { kind: 'inline', freqs_hz: [], n: d.controls.length, re: [], im: [] },
		parts: partsOf(d),
		exclusion_hz: d.exclusion_hz,
		tracking_hz: d.tracking_hz,
		estimation: { ...d.estimation, window: 'Hann', overlap: 0.5 },
		identification: { ...d.identification },
		gates: { ...d.gates },
		caps: { up_db: c.cap_up_db, down_db: c.cap_down_db },
		protections: { ...d.protections },
		schedule: levelScheduleOf(d),
		full_level_duration_s: d.schedule.full_level_s,
		level_ramp_s: c.level_ramp_s,
		shutdown_ramp_s: c.shutdown_ramp_s,
		part_ramp_s: c.part_ramp_s,
		floor_settle_s: c.floor_settle_s,
		soft_start_db: c.soft_start_db,
		soft_start_relax_db: c.soft_start_relax_db,
		frf_mode: 'Hold',
		crest_factor: c.crest_factor,
		law: {
			floor: { id: d.laws.floor.id, params: lawWireParams(laws.floor, d.laws.floor.params) },
			tones: { id: d.laws.tones.id, params: lawWireParams(laws.tones, d.laws.tones.params) },
			bands: { id: d.laws.bands },
			ceiling_rule: { id: d.laws.ceiling_rule },
			fill_rule: { id: d.laws.fill_rule }
		},
		record: { streams: r.streams }
	};
	if (document) out.specification = { document, clause };
	return out;
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

/** A finding's name in words, for the list of refusals: every name the service gives a setup finding. */
export const REFUSAL_WORDS: Record<string, string> = {
	no_floor: 'No floor',
	more_than_one_floor: 'More than one floor',
	unknown_part_kind: 'Unknown part kind',
	part_out_of_band: 'Part outside the band',
	parts_too_close: 'Parts too close',
	too_many_tones: 'Too many tones',
	too_many_bands: 'Too many narrowbands',
	duplicate_part_id: 'Two parts with one id',
	follow_unknown_part: 'Follows an unknown part',
	follow_self: 'Follows itself',
	follow_loop: 'Followers in a loop',
	follow_no_frequency: 'Follows a part with no frequency',
	segments_not_built: 'Segment schedules: not built',
	bad_part: 'Part not readable',
	bad_config: 'Setting out of range',
	wrong_control_count: 'Wrong number of values per control',
	band_not_additive: 'Band not additive',
	band_matrix_not_built: 'Band as a full matrix: not built',
	law_refuses_tone: 'Tone law does not run this tone',
	budget_cannot_fit: 'Drive budget cannot fit',
	tone_model_not_finite: 'Tone model not finite',
	frf_update_not_built: 'Model update: not built',
	unknown_band_law: 'Unknown band law',
	unknown_ceiling_rule: 'Unknown ceiling rule',
	unknown_fill_rule: 'Unknown fill rule',
	abort_below_ceiling: 'Abort below its ceiling',
	ramp_too_short: 'Ramp too short',
	duplicate_drive: 'One channel as two drives',
	channel_unknown: 'Channel not in the table',
	reference_invalid: 'Reference not accepted',
	law_unknown: 'Law not in the catalog',
	law_wrong_world: 'Law of another kind of test',
	param_unknown: 'Law setting not known',
	param_out_of_range: 'Law setting out of range',
	out_of_range: 'Out of range',
	config_unreadable: 'Setup not readable',
	drive_channel_present: 'Drive channel present'
};

/** What a field path names: a part by its place in the parts list, a control, a drive or a limit by its place. */
function placeOf(field: string | undefined): { list: 'parts' | 'controls' | 'drives' | 'limits'; i: number } | null {
	const m = /^(parts|controls|drives|limits)\[(\d+)\]/.exec(field ?? '');
	return m ? { list: m[1] as 'parts' | 'controls' | 'drives' | 'limits', i: Number(m[2]) } : null;
}

/** Where a finding is: its part, control, drive, limit or channel, in words. `parts` in the wire's order: the floor, the tones, the narrowbands. */
export function refusalWhere(item: Pick<MimoMixedReportItem, 'field' | 'channel'>, names: { parts: string[]; controls: string[]; drives: string[]; limits?: string[] }): string {
	const at = placeOf(item.field);
	if (at) {
		const word = { parts: 'part', controls: 'control', drives: 'drive', limits: 'limit' }[at.list];
		return `${word} ${(names[at.list] ?? [])[at.i] ?? at.i + 1}`;
	}
	if (/^law\.floor/.test(item.field ?? '')) return "the floor's law";
	if (/^law\.tones/.test(item.field ?? '')) return "the tones' law";
	if (item.channel !== undefined && item.channel !== null) return `channel ${item.channel}`;
	return '';
}

/** The editor's key for a service finding the controller names for the whole setup (`field: "config"`). */
const CONFIG_KEYS: Record<string, string> = {
	budget_cannot_fit: 'forecast',
	tone_model_not_finite: 'forecast',
	frf_update_not_built: 'control.frf',
	unknown_band_law: 'laws.bands',
	unknown_ceiling_rule: 'laws.ceiling_rule',
	unknown_fill_rule: 'laws.fill_rule',
	abort_below_ceiling: 'channels.drives',
	ramp_too_short: 'control.part_ramp_s',
	wrong_control_count: 'parts',
	reference_invalid: 'reference'
};

/** The editor's key for a service finding: by its field path (a part, a drive, a control, a limit, a law) or its name. */
export function mmixItemKey(d: Pick<MmixDraft, 'tones' | 'bands'>, item: MimoMixedReportItem | (Pick<ReportItem, 'field' | 'channel' | 'row'> & { reason: string })): string {
	const it = item as MimoMixedReportItem;
	const field = it.field ?? '';
	if (field === 'config' || !field) {
		if (CONFIG_KEYS[it.reason]) return CONFIG_KEYS[it.reason];
		if (field === 'config') return 'parts';
		if (it.reason.startsWith('law_')) return 'laws.floor.id';
		if (it.channel !== undefined && it.channel !== null) return 'channels';
		return 'setup';
	}
	const at = placeOf(field);
	if (at?.list === 'parts') {
		const t = at.i - 1;
		if (t >= 0 && t < d.tones.length) return `tones.${t}`;
		const b = t - d.tones.length;
		if (b >= 0 && b < d.bands.length) return `bands.${b}`;
		return 'parts';
	}
	if (at) return at.list === 'controls' ? 'channels.controls' : `${at.list}.${at.i}`;
	const f = field.replace(/\[(\d+)\]/g, '.$1');
	const m = /^law\.(floor|tones)(?:\.(id|params\.(\w+)))?/.exec(f);
	if (m) return m[3] ? `laws.${m[1]}.params.${m[3]}` : `laws.${m[1]}.id`;
	if (/^reference/.test(f)) return 'reference';
	if (/^law\./.test(f)) return f.replace(/^law\./, 'laws.');
	return f;
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
	if (/^(schedule|identification|estimation|gates)(\.|$)/.test(key)) return 'schedule';
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
		if (s.end_by !== 'duration') need(out, `${at}.sweeps`, s.sweeps);
		if (s.end_by === 'duration' || s.end_by === 'both') need(out, `${at}.duration_s`, s.duration_s);
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
		else if (x.abort_vrms !== null && Number.isFinite(x.ceiling_vrms) && x.abort_vrms <= x.ceiling_vrms) add(out, `drives.${i}.abort_vrms`, 'The rms abort is above the rms ceiling.');
		if (x.abort_vpk !== null && !Number.isFinite(x.abort_vpk)) add(out, `drives.${i}.abort_vpk`, NUMBER);
		else if (x.abort_vpk !== null && Number.isFinite(x.peak_v) && x.abort_vpk <= x.peak_v) add(out, `drives.${i}.abort_vpk`, 'The peak abort is above the peak ceiling.');
		if (!(x.pretest_max_vrms > 0)) add(out, `drives.${i}.pretest_max_vrms`, 'Give the most the identification may put on this drive, volts rms above 0.');
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
	need(out, 'estimation.memory_loops', d.estimation.memory_loops);
	if (Number.isInteger(d.estimation.n_pts) && (d.estimation.n_pts < 64 || (d.estimation.n_pts & (d.estimation.n_pts - 1)) !== 0)) add(out, 'estimation.n_pts', 'A power of two from 64.');
	for (const k of ['below_reference_db', 'start_vrms', 'tolerance_db'] as const) need(out, `identification.${k}`, d.identification[k]);
	for (const k of ['frames', 'max_steps', 'noise_frames'] as const) need(out, `identification.${k}`, d.identification[k], true);
	need(out, 'gates.coherence_threshold', d.gates.coherence_threshold);
	if (Number.isFinite(d.gates.coherence_threshold) && (d.gates.coherence_threshold < 0 || d.gates.coherence_threshold > 1)) add(out, 'gates.coherence_threshold', 'A coherence from 0 to 1.');
	need(out, 'gates.condition_threshold', d.gates.condition_threshold);
	if (!d.schedule.levels.length) add(out, 'schedule.levels', 'Give at least one level.');
	d.schedule.levels.forEach((l, i) => need(out, `schedule.levels.${i}.db`, l.db));
	need(out, 'schedule.full_level_s', d.schedule.full_level_s);
	for (const [k, v] of Object.entries(d.control)) need(out, `control.${k}`, v as number);
	for (const k of ['level_ramp_s', 'part_ramp_s'] as const) if (Number.isFinite(d.control[k]) && d.control[k] < MIMO_MIXED_DEFAULTS.min_ramp_s) add(out, `control.${k}`, `At least ${MIMO_MIXED_DEFAULTS.min_ramp_s} s: two control ticks.`);
	for (const [k, v] of Object.entries(d.protections)) need(out, `protections.${k}`, v as number, k === 'envelope_abort_lines' || k === 'divergence_loops' || k === 'settle_loops');
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
