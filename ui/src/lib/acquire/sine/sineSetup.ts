/**
 * The Sine setup as the operator edits it, and its conversion to and from the protocol's
 * `SineConfig` (schema/PROTOCOL.md, "Sine Configuration"). Pure functions only: the service
 * computes every number about the test (the profile at any frequency, its crossovers and
 * kinematics, the grid and the tick, the schedule's length, the identification's length, the stop
 * ramp, the tracking check), and this module only carries the operator's choices.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. A choice the wire leaves to the station (the rate) is null. The drive
 * ceiling and the drive abort have no default anywhere: the setup asks for both.
 *
 * A profile row keeps the profile's own units: g peak, in/s peak or in peak to peak. The editor
 * keeps every test kind's settings, so a change of kind loses nothing; the wire carries the chosen
 * one. A resonance search (`rstd`) is accepted on the wire but not offered here: a setup another
 * screen sent with one is kept as it came.
 */

import type { BandDb, ChannelRow, CompositeMode, LadderMode, ReportItem } from '$lib/protocol/types';
import type { LawEntry } from '$lib/protocol/mimo';
import type {
	SineCaptureRule,
	SineConfig,
	SineDirection,
	SineDwellFor,
	SineProfile,
	SineProfileRow,
	SineProfileValidateParams,
	SineQuantity,
	SineRstdTest,
	SineRungFor,
	SineTest
} from '$lib/protocol/sine';
import { lawWireParams, type LawDraft } from '../law/lawForm';
import { shakerRecord, type FieldErrors } from '../setup';
import { runChannels } from '../shock/shockSetup';

export { runChannels };

// ---------------------------------------------------------------------------------------------
// The draft

export interface SineRowDraft {
	freq_hz: number;
	quantity: SineQuantity;
	value: number;
	/** The row carries bands of its own, from it to the next row; off, the profile's hold. */
	own_bands: boolean;
	alarm_db: BandDb;
	abort_db: BandDb;
}

export interface SineProfileDraft {
	label: string;
	rows: SineRowDraft[];
	alarm_db: BandDb;
	abort_db: BandDb;
}

export interface SineLimitChannelDraft {
	/** The channel table row's `index`: an enabled input that is not a control row. */
	channel: number;
	/** Names a bound several channels share; empty, its own. */
	shared_name: string;
	/** The bound at each frequency, in the profile's form. */
	profile: SineProfileDraft;
}

export type SineTestKindDraft = 'sweep' | 'dwell' | 'step' | 'rstd';

/** How a sweep ends: after a count of one-way sweeps, a test time, or whichever comes first. */
export type SineEndBy = 'sweeps' | 'duration' | 'both';

export interface SineDraft {
	title: string;
	profile: SineProfileDraft;
	test: {
		kind: SineTestKindDraft;
		/** The sweep's and the step test's band. */
		band_hz: [number, number];
		sweep: {
			law: 'log' | 'linear';
			oct_per_min: number;
			hz_per_s: number;
			/** Null: the band's low edge. */
			start_hz: number | null;
			direction: SineDirection;
			end_by: SineEndBy;
			sweeps: number;
			duration_s: number;
		};
		dwell: { freq_hz: number; by: 'seconds' | 'cycles'; seconds: number; cycles: number };
		step: { steps_per_octave: number; spacing: 'log' | 'linear'; by: 'computed' | 'seconds' | 'cycles'; seconds: number; cycles: number };
		/** A resonance search a setup came with, kept as it came; the editor does not offer it. */
		rstd: SineRstdTest | null;
	};
	acquisition: { fs_hz: number | null; tick_ms: number };
	schedule: {
		mode: LadderMode;
		/** Ascending dB; the last is full level. */
		levels: number[];
		start_db: number;
		per_level: { by: 'sweeps' | 'seconds'; n: number };
		full_level: { by: 'sweeps' | 'seconds'; n: number };
	};
	identification: { level_db: number; gate_coherence: number; trust_coherence: number; trust_h_floor_frac: number; review: boolean };
	law: LawDraft;
	control: {
		composite: CompositeMode;
		/** Each control row's weight; a row with none weighs 1. */
		weights: { channel: number; weight: number }[];
		tracking: { kind: 'fixed' | 'proportional'; hz: number; pct: number; max_hz: number };
		law_budget: number;
		law_misses: number;
		max_step_db: number;
		max_rise_db_per_s: number;
		divergence_s: number;
		fault_debounce_ticks: number;
	};
	limits: {
		/** Volts peak; NaN until the operator gives it. */
		drive_vpk_ceiling: number;
		drive_vpk_abort: number;
		kinematic_monitor: boolean;
		channels: SineLimitChannelDraft[];
		notch: { drive_floor: number; smoothing_ticks: number; release_ramp_db_per_s: number };
	};
	cola: { enabled: boolean; volts_pk: number; waveform: 'sine' | 'square' | 'dc' };
	feasibility: {
		enabled: boolean;
		shaker_text: string;
		moving_mass_lb: { armature: number; slip_table: number; drive_adapter: number; fixturing: number; test_article: number };
	};
	capture: { rule: SineCaptureRule; first_s: number; level_settle_s: number; periodic_save_s: number | null; run_name: string };
	metadata: { key: string; label: string; value: string }[];
}

export const DEFAULT_SINE_LAW = 'servo_blend';

export const QUANTITIES: { id: SineQuantity; label: string; unit: string }[] = [
	{ id: 'accel_g', label: 'Acceleration', unit: 'g pk' },
	{ id: 'velocity_ips', label: 'Velocity', unit: 'in/s pk' },
	{ id: 'disp_pp_in', label: 'Displacement', unit: 'in pk-pk' }
];

export const quantityUnit = (q: SineQuantity): string => QUANTITIES.find((x) => x.id === q)?.unit ?? '';

export const CAPTURE_RULES: { id: SineCaptureRule; label: string; help: string }[] = [
	{ id: 'off', label: 'Off', help: 'Nothing is recorded beyond the run report.' },
	{ id: 'entire_test', label: 'Entire test', help: 'Everything from engage to the end.' },
	{ id: 'manual', label: 'Manual', help: 'Only when you start and stop it.' },
	{ id: 'first_seconds', label: 'First seconds at each level', help: 'A set number of seconds at each level, once it has settled there.' }
];

export function newRow(freq_hz = 100, quantity: SineQuantity = 'accel_g', value = 1): SineRowDraft {
	return { freq_hz, quantity, value, own_bands: false, alarm_db: [1.5, 1.5], abort_db: [3, 3] };
}

export function newLimitChannel(row: ChannelRow | undefined, band: readonly [number, number] = [20, 2000]): SineLimitChannelDraft {
	return {
		channel: row?.index ?? 0,
		shared_name: '',
		profile: { label: '', rows: [newRow(band[0], 'accel_g', NaN), newRow(band[1], 'accel_g', NaN)], alarm_db: [1.5, 1.5], abort_db: [3, 3] }
	};
}

export function defaultSineSetup(): SineDraft {
	return {
		title: 'Sine 1 g, 20 to 2000 Hz',
		profile: {
			label: '1 g',
			rows: [newRow(20, 'disp_pp_in', 0.0097), newRow(45, 'accel_g', 1), newRow(2000, 'accel_g', 1)],
			alarm_db: [1.5, 1.5],
			abort_db: [3, 3]
		},
		test: {
			kind: 'sweep',
			band_hz: [20, 2000],
			sweep: { law: 'log', oct_per_min: 2, hz_per_s: 5, start_hz: null, direction: 'up', end_by: 'sweeps', sweeps: 4, duration_s: 900 },
			dwell: { freq_hz: 100, by: 'seconds', seconds: 60, cycles: 6000 },
			step: { steps_per_octave: 3, spacing: 'log', by: 'computed', seconds: 10, cycles: 1000 },
			rstd: null
		},
		acquisition: { fs_hz: null, tick_ms: 10 },
		schedule: { mode: 'auto', levels: [-12, -6, 0], start_db: -12, per_level: { by: 'sweeps', n: 1 }, full_level: { by: 'sweeps', n: 2 } },
		identification: { level_db: -20, gate_coherence: 0.5, trust_coherence: 0.5, trust_h_floor_frac: 0.05, review: true },
		law: { id: DEFAULT_SINE_LAW, params: {} },
		control: {
			composite: 'weighted_avg',
			weights: [],
			tracking: { kind: 'proportional', hz: 10, pct: 10, max_hz: 100 },
			law_budget: 0.25,
			law_misses: 3,
			max_step_db: 1,
			max_rise_db_per_s: 240,
			divergence_s: 1,
			fault_debounce_ticks: 50
		},
		limits: {
			drive_vpk_ceiling: NaN,
			drive_vpk_abort: NaN,
			kinematic_monitor: true,
			channels: [],
			notch: { drive_floor: 0.01, smoothing_ticks: 3, release_ramp_db_per_s: 20 }
		},
		cola: { enabled: false, volts_pk: 0.1, waveform: 'sine' },
		feasibility: { enabled: false, shaker_text: '', moving_mass_lb: { armature: 0, slip_table: 0, drive_adapter: 0, fixturing: 0, test_article: 0 } },
		capture: { rule: 'off', first_s: 10, level_settle_s: 1, periodic_save_s: null, run_name: 'sine' },
		metadata: [{ key: 'run_number', label: 'Run Number', value: '' }]
	};
}

const isRows = (v: unknown): v is SineRowDraft[] => Array.isArray(v) && v.every((r) => r && typeof r === 'object');

function mergeProfile(base: SineProfileDraft, saved: Partial<SineProfileDraft> | undefined): SineProfileDraft {
	const out = { ...base, ...(saved ?? {}) } as SineProfileDraft;
	out.rows = isRows(out.rows) && out.rows.length ? out.rows.map((r) => ({ ...newRow(), ...r })) : base.rows;
	return out;
}

/** A draft read back from storage, every section filled from the defaults where it is missing. */
export function mergeSineSetup(saved: Partial<SineDraft> | null): SineDraft {
	const base = defaultSineSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as SineDraft;
	out.profile = mergeProfile(base.profile, saved.profile);
	const t = (saved.test ?? {}) as Partial<SineDraft['test']>;
	out.test = {
		...base.test,
		...t,
		sweep: { ...base.test.sweep, ...(t.sweep ?? {}) },
		dwell: { ...base.test.dwell, ...(t.dwell ?? {}) },
		step: { ...base.test.step, ...(t.step ?? {}) },
		rstd: t.rstd ?? null
	};
	if (!Array.isArray(out.test.band_hz) || out.test.band_hz.length !== 2) out.test.band_hz = base.test.band_hz;
	out.acquisition = { ...base.acquisition, ...(saved.acquisition ?? {}) };
	const s = (saved.schedule ?? {}) as Partial<SineDraft['schedule']>;
	out.schedule = { ...base.schedule, ...s, per_level: { ...base.schedule.per_level, ...(s.per_level ?? {}) }, full_level: { ...base.schedule.full_level, ...(s.full_level ?? {}) } };
	if (!Array.isArray(out.schedule.levels) || !out.schedule.levels.length) out.schedule.levels = base.schedule.levels;
	out.identification = { ...base.identification, ...(saved.identification ?? {}) };
	out.law = { ...base.law, ...(saved.law ?? {}) };
	const c = (saved.control ?? {}) as Partial<SineDraft['control']>;
	out.control = { ...base.control, ...c, tracking: { ...base.control.tracking, ...(c.tracking ?? {}) } };
	if (!Array.isArray(out.control.weights)) out.control.weights = [];
	const l = (saved.limits ?? {}) as Partial<SineDraft['limits']>;
	out.limits = { ...base.limits, ...l, notch: { ...base.limits.notch, ...(l.notch ?? {}) } };
	out.limits.channels = Array.isArray(out.limits.channels)
		? out.limits.channels.map((ch) => ({ ...newLimitChannel(undefined), ...ch, profile: mergeProfile(newLimitChannel(undefined).profile, ch.profile) }))
		: [];
	out.cola = { ...base.cola, ...(saved.cola ?? {}) };
	out.feasibility = { ...base.feasibility, ...(saved.feasibility ?? {}) };
	out.feasibility.moving_mass_lb = { ...base.feasibility.moving_mass_lb, ...(saved.feasibility?.moving_mass_lb ?? {}) };
	out.capture = { ...base.capture, ...(saved.capture ?? {}) };
	if (!Array.isArray(out.metadata)) out.metadata = base.metadata;
	return out;
}

// ---------------------------------------------------------------------------------------------
// To and from the protocol

const pair = (v: readonly number[]): BandDb => [v[0], v[1]];

function rowOf(r: SineRowDraft): SineProfileRow {
	const row: SineProfileRow = { freq_hz: r.freq_hz, quantity: r.quantity, value: r.value };
	if (r.own_bands) {
		row.alarm_db = pair(r.alarm_db);
		row.abort_db = pair(r.abort_db);
	}
	return row;
}

/** A profile as the wire takes it: a row's own bands only where it has them. */
export function profileOf(p: SineProfileDraft): SineProfile {
	const out: SineProfile = { rows: p.rows.map(rowOf), alarm_db: pair(p.alarm_db), abort_db: pair(p.abort_db) };
	if (p.label.trim()) out.label = p.label.trim();
	return out;
}

/** A profile from the wire as the editor holds it; bands the profile leaves out are `base`'s. */
export function profileDraftOf(p: SineProfile, base: SineProfileDraft): SineProfileDraft {
	const alarm = p.alarm_db ? pair(p.alarm_db) : base.alarm_db;
	const abort = p.abort_db ? pair(p.abort_db) : base.abort_db;
	return {
		label: p.label ?? '',
		rows: p.rows.map((r) => ({
			freq_hz: r.freq_hz,
			quantity: r.quantity,
			value: r.value,
			own_bands: !!(r.alarm_db || r.abort_db),
			alarm_db: r.alarm_db ? pair(r.alarm_db) : alarm,
			abort_db: r.abort_db ? pair(r.abort_db) : abort
		})),
		alarm_db: alarm,
		abort_db: abort
	};
}

const forOf = (by: 'seconds' | 'cycles', seconds: number, cycles: number): SineDwellFor => (by === 'seconds' ? { seconds } : { cycles });
const rungOf = (r: { by: 'sweeps' | 'seconds'; n: number }): SineRungFor => (r.by === 'sweeps' ? { sweeps: r.n } : { seconds: r.n });

/** The test the draft describes, the chosen kind's settings only. */
export function testOf(d: SineDraft): SineTest {
	const t = d.test;
	switch (t.kind) {
		case 'rstd':
			if (t.rstd) return structuredClone(t.rstd);
			break;
		case 'dwell':
			return { kind: 'dwell', freq_hz: t.dwell.freq_hz, end: forOf(t.dwell.by, t.dwell.seconds, t.dwell.cycles) };
		case 'step':
			return { kind: 'step', band_hz: [t.band_hz[0], t.band_hz[1]], steps_per_octave: t.step.steps_per_octave, spacing: t.step.spacing, dwell: t.step.by === 'computed' ? null : forOf(t.step.by, t.step.seconds, t.step.cycles) };
		default:
			break;
	}
	const s = t.sweep;
	return {
		kind: 'sweep',
		band_hz: [t.band_hz[0], t.band_hz[1]],
		rate: s.law === 'log' ? { law: 'log', oct_per_min: s.oct_per_min } : { law: 'linear', hz_per_s: s.hz_per_s },
		start_hz: s.start_hz ?? t.band_hz[0],
		direction: s.direction,
		end: { sweeps: s.end_by === 'duration' ? null : s.sweeps, duration_s: s.end_by === 'sweeps' ? null : s.duration_s }
	};
}

/** The band the test covers: a dwell's is its one frequency. */
export function bandOf(d: SineDraft): [number, number] {
	if (d.test.kind === 'dwell') return [d.test.dwell.freq_hz, d.test.dwell.freq_hz];
	if (d.test.kind === 'rstd' && d.test.rstd) return [d.test.rstd.band_hz[0], d.test.rstd.band_hz[1]];
	return [d.test.band_hz[0], d.test.band_hz[1]];
}

/**
 * The configuration the draft describes, every field explicit. Call `localErrors` first. With the
 * channel table's rows, a weight is sent only for a row that is an enabled control row.
 */
export function toSineConfig(d: SineDraft, law: LawEntry | null, rows?: ChannelRow[]): SineConfig {
	const f = d.feasibility;
	const shaker = f.enabled ? shakerRecord(f.shaker_text) : null;
	const controlRows = rows ? new Set(rows.filter((r) => r.enabled && r.role === 'control').map((r) => r.index)) : null;
	const weights = d.control.weights.filter((w) => !controlRows || controlRows.has(w.channel)).map((w) => ({ channel: w.channel, weight: w.weight }));
	const c = d.control;
	const tracking = c.tracking.kind === 'fixed' ? { kind: 'fixed' as const, hz: c.tracking.hz } : { kind: 'proportional' as const, pct: c.tracking.pct, max_hz: c.tracking.max_hz };
	const capture = d.capture;
	return {
		title: d.title.trim(),
		profile: profileOf(d.profile),
		test: testOf(d),
		acquisition: { fs_hz: d.acquisition.fs_hz, tick_ms: d.acquisition.tick_ms },
		schedule: {
			mode: d.schedule.mode,
			levels_db: [...d.schedule.levels],
			start_db: d.schedule.start_db,
			per_level: rungOf(d.schedule.per_level),
			full_level: rungOf(d.schedule.full_level)
		},
		identification: { ...d.identification },
		control: {
			composite: c.composite,
			weights,
			tracking,
			law: { id: d.law.id, params: lawWireParams(law, d.law.params) },
			law_budget: c.law_budget,
			law_misses: c.law_misses,
			max_step_db: c.max_step_db,
			max_rise_db_per_s: c.max_rise_db_per_s,
			divergence_s: c.divergence_s,
			fault_debounce_ticks: c.fault_debounce_ticks
		},
		limits: {
			drive_vpk_ceiling: d.limits.drive_vpk_ceiling,
			drive_vpk_abort: d.limits.drive_vpk_abort,
			kinematic_monitor: d.limits.kinematic_monitor,
			channels: d.limits.channels.map((ch) => ({ channel: ch.channel, profile: profileOf(ch.profile), shared_name: ch.shared_name.trim() || null })),
			notch: { ...d.limits.notch }
		},
		cola: { ...d.cola },
		feasibility: f.enabled && shaker ? { shaker, moving_mass_lb: { ...f.moving_mass_lb } } : null,
		capture: {
			rule: capture.rule,
			first_s: capture.rule === 'first_seconds' ? capture.first_s : null,
			level_settle_s: capture.level_settle_s,
			periodic_save_s: capture.periodic_save_s,
			run_name: capture.run_name.trim()
		},
		metadata: d.metadata.map((m) => (m.label ? { key: m.key, label: m.label, value: m.value } : { key: m.key, value: m.value }))
	};
}

/**
 * The draft for a configuration the service holds (another screen applied it, or this one before a
 * reload). Fields the configuration leaves out take the wire's defaults; a test kind it does not
 * carry keeps the shell's.
 */
export function fromSineConfig(c: SineConfig): SineDraft {
	const base = defaultSineSetup();
	const t = c.test;
	const test: SineDraft['test'] = structuredClone(base.test);
	const dwellBy = (f: SineDwellFor | null | undefined) =>
		f && 'cycles' in f ? { by: 'cycles' as const, cycles: f.cycles } : f && 'seconds' in f ? { by: 'seconds' as const, seconds: f.seconds } : {};
	test.kind = t.kind;
	if (t.kind === 'sweep') {
		test.band_hz = [t.band_hz[0], t.band_hz[1]];
		const sweeps = t.end.sweeps ?? null;
		const duration = t.end.duration_s ?? null;
		test.sweep = {
			law: t.rate.law,
			oct_per_min: t.rate.law === 'log' ? t.rate.oct_per_min : base.test.sweep.oct_per_min,
			hz_per_s: t.rate.law === 'linear' ? t.rate.hz_per_s : base.test.sweep.hz_per_s,
			start_hz: t.start_hz ?? null,
			direction: t.direction ?? 'up',
			end_by: sweeps !== null && duration !== null ? 'both' : duration !== null ? 'duration' : 'sweeps',
			sweeps: sweeps ?? base.test.sweep.sweeps,
			duration_s: duration ?? base.test.sweep.duration_s
		};
	} else if (t.kind === 'dwell') {
		test.dwell = { ...test.dwell, freq_hz: t.freq_hz, ...dwellBy(t.end) };
	} else if (t.kind === 'step') {
		test.band_hz = [t.band_hz[0], t.band_hz[1]];
		const by = t.dwell ? dwellBy(t.dwell) : { by: 'computed' as const };
		test.step = { ...test.step, steps_per_octave: t.steps_per_octave, spacing: t.spacing ?? 'log', ...by };
	} else {
		test.rstd = structuredClone(t);
		test.band_hz = [t.band_hz[0], t.band_hz[1]];
	}
	const sch = c.schedule ?? {};
	const levels = sch.levels_db ?? [-12, -9, -6, -3, 0];
	const rung = (r: SineRungFor | undefined, fallback: SineDraft['schedule']['per_level']) => (!r ? { by: 'sweeps' as const, n: 1 } : 'sweeps' in r ? { by: 'sweeps' as const, n: r.sweeps } : 'seconds' in r ? { by: 'seconds' as const, n: r.seconds } : fallback);
	const ctl = c.control ?? {};
	const tr = ctl.tracking;
	const cap = c.capture ?? {};
	const lim = c.limits;
	const wireId = { level_db: -20, gate_coherence: 0.5, trust_coherence: 0.5, trust_h_floor_frac: 0.05, review: true };
	const wireControl = { law_budget: 0.25, law_misses: 3, max_step_db: 1, max_rise_db_per_s: 240, divergence_s: 1, fault_debounce_ticks: 50 };
	return {
		title: c.title ?? '',
		profile: profileDraftOf(c.profile, base.profile),
		test,
		acquisition: { fs_hz: c.acquisition?.fs_hz ?? null, tick_ms: c.acquisition?.tick_ms ?? 10 },
		schedule: {
			mode: sch.mode ?? 'auto',
			levels: [...levels],
			start_db: sch.start_db ?? levels[0],
			per_level: rung(sch.per_level, base.schedule.per_level),
			full_level: rung(sch.full_level, base.schedule.full_level)
		},
		identification: { ...wireId, ...(c.identification ?? {}) },
		law: { id: ctl.law?.id ?? DEFAULT_SINE_LAW, params: { ...((ctl.law?.params ?? {}) as LawDraft['params']) } },
		control: {
			composite: ctl.composite ?? 'weighted_avg',
			weights: (ctl.weights ?? []).map((w) => ({ ...w })),
			tracking: tr
				? tr.kind === 'fixed'
					? { ...base.control.tracking, kind: 'fixed', hz: tr.hz }
					: { ...base.control.tracking, kind: 'proportional', pct: tr.pct, max_hz: tr.max_hz }
				: { ...base.control.tracking },
			law_budget: ctl.law_budget ?? wireControl.law_budget,
			law_misses: ctl.law_misses ?? wireControl.law_misses,
			max_step_db: ctl.max_step_db ?? wireControl.max_step_db,
			max_rise_db_per_s: ctl.max_rise_db_per_s ?? wireControl.max_rise_db_per_s,
			divergence_s: ctl.divergence_s ?? wireControl.divergence_s,
			fault_debounce_ticks: ctl.fault_debounce_ticks ?? wireControl.fault_debounce_ticks
		},
		limits: {
			drive_vpk_ceiling: lim.drive_vpk_ceiling,
			drive_vpk_abort: lim.drive_vpk_abort,
			kinematic_monitor: lim.kinematic_monitor ?? true,
			channels: (lim.channels ?? []).map((ch) => ({ channel: ch.channel, shared_name: ch.shared_name ?? '', profile: profileDraftOf(ch.profile, newLimitChannel(undefined).profile) })),
			notch: { ...base.limits.notch, ...(lim.notch ?? {}) }
		},
		cola: { ...base.cola, enabled: false, ...(c.cola ?? {}) },
		feasibility: c.feasibility
			? { enabled: true, shaker_text: JSON.stringify(c.feasibility.shaker, null, 2), moving_mass_lb: { ...c.feasibility.moving_mass_lb } }
			: base.feasibility,
		capture: {
			rule: cap.rule ?? 'off',
			first_s: cap.first_s ?? base.capture.first_s,
			level_settle_s: cap.level_settle_s ?? base.capture.level_settle_s,
			periodic_save_s: cap.periodic_save_s ?? null,
			run_name: cap.run_name ?? base.capture.run_name
		},
		metadata: (c.metadata ?? []).map((m) => ({ key: m.key, label: m.label ?? '', value: m.value }))
	};
}

/**
 * What `sine.profile.validate` is asked for the draft's profile, or null while it holds no
 * numbers. The band is the test's, and the shaker's ratings go with it when the check is on.
 */
export function validateParams(d: SineDraft): SineProfileValidateParams | null {
	const p = d.profile;
	if (p.rows.length < 2) return null;
	const nums: number[] = [...p.alarm_db, ...p.abort_db];
	for (const r of p.rows) {
		nums.push(r.freq_hz, r.value);
		if (r.own_bands) nums.push(...r.alarm_db, ...r.abort_db);
	}
	if (!nums.every(Number.isFinite)) return null;
	const band = bandOf(d);
	const f = d.feasibility;
	const shaker = f.enabled ? shakerRecord(f.shaker_text) : null;
	return {
		profile: profileOf(p),
		band_hz: band.every(Number.isFinite) && band[1] > band[0] ? band : null,
		feasibility: shaker && Object.values(f.moving_mass_lb).every(Number.isFinite) ? { shaker, moving_mass_lb: { ...f.moving_mass_lb } } : null
	};
}

/**
 * Rows read from pasted text, before the service has read them: one row a line, tab, comma,
 * semicolon or space separated. The service reads the text (`sine.profile.import`); this only
 * says whether there is text to send.
 */
export const hasProfileText = (text: string): boolean => text.split(/\r?\n/).some((l) => l.trim() && !l.trim().startsWith('#'));

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

/** The setup's sections, in the screen's order. */
export type SineSection = 'station' | 'profile' | 'test' | 'schedule' | 'control' | 'limits' | 'capture';

export const SINE_SECTIONS: { id: SineSection; label: string }[] = [
	{ id: 'station', label: 'Station & Channels' },
	{ id: 'profile', label: 'Profile' },
	{ id: 'test', label: 'Test' },
	{ id: 'schedule', label: 'Schedule' },
	{ id: 'control', label: 'Control' },
	{ id: 'limits', label: 'Limits' },
	{ id: 'capture', label: 'Capture' }
];

/**
 * The editor's key for a service finding. A field path loses its `config.` head and its brackets.
 * A profile row's findings go under the profile table (`profile.rows`), their message naming the
 * row; an item located by `row` alone is a profile row's. A limit channel's go on its channel or
 * its bound; the law's path is the law panel's (`control.law.params.x` is `law.params.x`); a weight's
 * go under the weights. A finding about the channel table's control or drive row is the station's.
 */
export function sineItemKey(item: Pick<ReportItem, 'field' | 'channel' | 'row'> & { reason: string }): string {
	if (item.reason === 'no_control_channel' || item.reason === 'no_drive_channel') return 'station.channels';
	if (item.field) {
		const path = item.field.replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
		if (path === 'profile' || /^profile\.rows(\.|$)/.test(path)) return 'profile.rows';
		let m = /^schedule\.levels_db\.(\d+)$/.exec(path);
		if (m) return `schedule.levels.${m[1]}`;
		if (path === 'schedule.levels_db') return 'schedule.levels';
		if (/^schedule\.per_level(\.|$)/.test(path)) return 'schedule.per_level';
		if (/^schedule\.full_level(\.|$)/.test(path)) return 'schedule.full_level';
		if (/^test\.rate(\.|$)/.test(path)) return 'test.rate';
		m = /^test\.band_hz\.(\d)$/.exec(path);
		if (m) return `test.band_hz.${m[1]}`;
		m = /^limits\.channels\.(\d+)(?:\.(channel|profile|shared_name))?/.exec(path);
		if (m) return `limits.channels.${m[1]}.${m[2] === 'profile' ? 'profile' : m[2] === 'shared_name' ? 'shared_name' : 'channel'}`;
		if (/^control\.weights(\.|$)/.test(path)) return 'control.weights';
		if (/^control\.tracking(\.|$)/.test(path)) return 'control.tracking';
		if (path.startsWith('control.law.') || path === 'control.law') return path.replace(/^control\.law(\.id)?/, (_, id) => (id ? 'law.id' : 'law')).replace(/^law$/, 'law.id');
		if (/^feasibility(\.|$)/.test(path) && !/moving_mass/.test(path)) return 'feasibility.shaker';
		return path;
	}
	if (item.row !== undefined && item.row !== null) return 'profile.rows';
	if (item.reason === 'limit_unknown_channel' || item.reason === 'limit_on_control_channel') return 'limits.channels';
	if (item.reason === 'cola_needs_two_outputs') return 'cola';
	if (item.channel !== undefined && item.channel !== null) return 'station.channels';
	return 'setup';
}

export function sineByKey(items: { field?: string | null; channel?: number | null; row?: number; reason: string; message: string }[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = sineItemKey(it as Pick<ReportItem, 'field' | 'channel' | 'row'> & { reason: string });
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

/** The section a key belongs to. */
export function sineSectionOf(key: string): SineSection {
	if (/^station|^table|^channel/.test(key)) return 'station';
	if (/^(title|profile)(\.|$)/.test(key)) return 'profile';
	if (/^(test|acquisition\.fs_hz)(\.|$)/.test(key)) return 'test';
	if (/^identification\.(trust_coherence|trust_h_floor_frac)$/.test(key)) return 'control';
	if (/^(schedule|identification)(\.|$)/.test(key)) return 'schedule';
	if (/^(law|control|acquisition)(\.|$)/.test(key)) return 'control';
	if (/^(limits|cola|feasibility)(\.|$)/.test(key)) return 'limits';
	if (/^(capture|metadata)(\.|$)/.test(key)) return 'capture';
	return 'profile';
}

// ---------------------------------------------------------------------------------------------
// What the editor can say before anything is sent

const NUMBER = 'Enter a number.';
const WHOLE = 'Enter a whole number.';

function add(map: FieldErrors, key: string, message: string) {
	(map[key] ??= []).push(message);
}

function need(map: FieldErrors, key: string, v: number, whole = false) {
	if (!Number.isFinite(v)) add(map, key, whole ? WHOLE : NUMBER);
	else if (whole && !Number.isInteger(v)) add(map, key, WHOLE);
}

function needPair(map: FieldErrors, key: string, v: readonly number[]) {
	if (!(Number.isFinite(v[0]) && Number.isFinite(v[1]))) add(map, key, 'Give both: below and above.');
}

function profileErrors(out: FieldErrors, p: SineProfileDraft, at: string) {
	if (p.rows.length < 2) add(out, `${at}.rows`, 'Give at least two rows.');
	p.rows.forEach((r, i) => {
		need(out, `${at}.rows.${i}.freq_hz`, r.freq_hz);
		need(out, `${at}.rows.${i}.value`, r.value);
		if (r.own_bands) {
			needPair(out, `${at}.rows.${i}.alarm_db`, r.alarm_db);
			needPair(out, `${at}.rows.${i}.abort_db`, r.abort_db);
		}
	});
	needPair(out, `${at}.alarm_db`, p.alarm_db);
	needPair(out, `${at}.abort_db`, p.abort_db);
}

/**
 * Fields that hold no number, a title that is empty, a profile with too few rows, a drive ceiling
 * or abort not given, a limit channel on a row the table does not enable as a non-control input, a
 * shaker record that does not read. Everything else, the profile's shape and its coverage of the
 * band included, is the service's to judge in the dry run.
 */
export function localErrors(d: SineDraft, rows: ChannelRow[] = []): FieldErrors {
	const out: FieldErrors = {};
	if (!d.title.trim()) add(out, 'title', 'Give the test a title.');
	profileErrors(out, d.profile, 'profile');
	const t = d.test;
	if (t.kind === 'sweep' || t.kind === 'step') {
		need(out, 'test.band_hz.0', t.band_hz[0]);
		need(out, 'test.band_hz.1', t.band_hz[1]);
	}
	if (t.kind === 'sweep') {
		const s = t.sweep;
		need(out, 'test.rate', s.law === 'log' ? s.oct_per_min : s.hz_per_s);
		if (s.start_hz !== null) need(out, 'test.start_hz', s.start_hz);
		if (s.end_by !== 'duration') need(out, 'test.end.sweeps', s.sweeps, true);
		if (s.end_by !== 'sweeps') need(out, 'test.end.duration_s', s.duration_s);
	} else if (t.kind === 'dwell') {
		need(out, 'test.freq_hz', t.dwell.freq_hz);
		if (t.dwell.by === 'seconds') need(out, 'test.end.seconds', t.dwell.seconds);
		else need(out, 'test.end.cycles', t.dwell.cycles);
	} else if (t.kind === 'step') {
		need(out, 'test.steps_per_octave', t.step.steps_per_octave);
		if (t.step.by === 'seconds') need(out, 'test.dwell.seconds', t.step.seconds);
		else if (t.step.by === 'cycles') need(out, 'test.dwell.cycles', t.step.cycles);
	} else if (!t.rstd) add(out, 'test.kind', 'Choose a sweep, a dwell or a step test.');
	if (d.acquisition.fs_hz !== null) need(out, 'acquisition.fs_hz', d.acquisition.fs_hz);
	need(out, 'acquisition.tick_ms', d.acquisition.tick_ms);
	const sch = d.schedule;
	if (!sch.levels.length) add(out, 'schedule.levels', 'Give at least one level.');
	sch.levels.forEach((l, i) => need(out, `schedule.levels.${i}`, l));
	need(out, 'schedule.start_db', sch.start_db);
	for (const k of ['per_level', 'full_level'] as const) {
		need(out, `schedule.${k}`, sch[k].n, sch[k].by === 'sweeps');
		if (sch.mode === 'auto' && sch[k].by === 'sweeps' && t.kind !== 'sweep') add(out, `schedule.${k}`, 'Sweeps count in a sweep test only; give seconds.');
	}
	const id = d.identification;
	for (const k of ['level_db', 'gate_coherence', 'trust_coherence', 'trust_h_floor_frac'] as const) need(out, `identification.${k}`, id[k]);
	const c = d.control;
	if (c.weights.some((w) => !Number.isFinite(w.weight))) add(out, 'control.weights', 'Give every control channel a weight.');
	if (c.tracking.kind === 'fixed') need(out, 'control.tracking', c.tracking.hz);
	else {
		need(out, 'control.tracking', c.tracking.pct);
		need(out, 'control.tracking', c.tracking.max_hz);
	}
	for (const k of ['law_budget', 'max_step_db', 'max_rise_db_per_s', 'divergence_s'] as const) need(out, `control.${k}`, c[k]);
	need(out, 'control.law_misses', c.law_misses, true);
	need(out, 'control.fault_debounce_ticks', c.fault_debounce_ticks, true);
	const lim = d.limits;
	if (!Number.isFinite(lim.drive_vpk_ceiling)) add(out, 'limits.drive_vpk_ceiling', 'Give the drive ceiling in volts peak. There is no default: it is your amplifier input.');
	if (!Number.isFinite(lim.drive_vpk_abort)) add(out, 'limits.drive_vpk_abort', 'Give the drive abort in volts peak, at least the ceiling.');
	else if (Number.isFinite(lim.drive_vpk_ceiling) && lim.drive_vpk_abort < lim.drive_vpk_ceiling) add(out, 'limits.drive_vpk_abort', 'The drive abort is at least the drive ceiling.');
	for (const k of ['drive_floor', 'smoothing_ticks', 'release_ramp_db_per_s'] as const) need(out, `limits.notch.${k}`, lim.notch[k], k === 'smoothing_ticks');
	const control = new Set(rows.filter((r) => r.enabled && r.role === 'control').map((r) => r.index));
	const input = (index: number) => !rows.length || rows.some((r) => r.index === index && r.enabled && r.role !== 'drive');
	lim.channels.forEach((ch, i) => {
		if (control.has(ch.channel)) add(out, `limits.channels.${i}.channel`, `Channel ${ch.channel} is a control channel; a limit channel is another input.`);
		else if (!input(ch.channel)) add(out, `limits.channels.${i}.channel`, `Channel ${ch.channel} is not an enabled input of the channel table.`);
		profileErrors(out, ch.profile, `limits.channels.${i}.profile`);
	});
	if (d.cola.enabled) need(out, 'cola.volts_pk', d.cola.volts_pk);
	const f = d.feasibility;
	if (f.enabled) {
		if (!f.shaker_text.trim()) add(out, 'feasibility.shaker', 'Load or paste the shaker record, or turn the check off.');
		else if (!shakerRecord(f.shaker_text)) add(out, 'feasibility.shaker', 'The shaker record does not read as a record (JSON object).');
		for (const [k, v] of Object.entries(f.moving_mass_lb)) need(out, `feasibility.moving_mass_lb.${k}`, v);
	}
	const cap = d.capture;
	if (cap.rule === 'first_seconds') need(out, 'capture.first_s', cap.first_s);
	need(out, 'capture.level_settle_s', cap.level_settle_s);
	if (cap.periodic_save_s !== null) need(out, 'capture.periodic_save_s', cap.periodic_save_s);
	if (cap.rule !== 'off' && !cap.run_name.trim()) add(out, 'capture.run_name', 'Name the run folder, or turn capture off.');
	return out;
}
