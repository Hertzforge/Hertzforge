/**
 * The classical shock setup as the operator edits it, and its conversion to and from the
 * protocol's `ShockConfig` (schema/PROTOCOL.md, "Classical Shock Configuration"). Pure functions
 * only: the service computes every number about the pulse (its kinematics, its quiet window, the
 * grid, the time between pulses), and this module only carries the operator's choices.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. A choice the wire leaves to the service's default (the overshoot
 * abort, the predicted peak ceiling) is null. The drive ceiling and the drive abort have no
 * default anywhere: the setup asks for both.
 *
 * The shell's own defaults differ from the wire's on purpose: rectangular shoulders four times
 * the duration, and a ladder that advances on a passing pulse with at most five pulses a level.
 */

import type { ChannelRow, LadderMode, ReportItem } from '$lib/protocol/types';
import type { LawEntry } from '$lib/protocol/mimo';
import type {
	CompensationShape,
	ShockAdvance,
	ShockConfig,
	ShockMonitor,
	ShockPolarity,
	ShockPreviewParams,
	ShockShape,
	ShockStandardName
} from '$lib/protocol/shock';
import { lawWireParams, type LawDraft } from '../law/lawForm';
import { shakerRecord, type FieldErrors } from '../setup';

// ---------------------------------------------------------------------------------------------
// The draft

export interface ShockLevelDraft {
	db: number;
	pulses: number;
}

export interface ShockDraft {
	title: string;
	pulse: {
		shape: ShockShape;
		amplitude_g: number;
		duration_ms: number;
		polarity: ShockPolarity;
		/** Trapezoid only, fractions of the duration; kept for the other shapes, sent as null. */
		rise_frac: number;
		fall_frac: number;
		compensation: { shape: CompensationShape; pre_frac: number; post_frac: number; symmetric: boolean };
	};
	standard: {
		name: ShockStandardName;
		tolerance_abort: 'full_level' | 'every_level';
		/** Fraction over the commanded peak; null: twice the standard's amplitude tolerance. */
		overshoot_abort: number | null;
	};
	acquisition: {
		fs_hz: number;
		srs: { f_lo_hz: number; f_hi_hz: number; bands_per_octave: number; band_tol_db: number };
		pvss: boolean;
	};
	schedule: {
		mode: LadderMode;
		levels: ShockLevelDraft[];
		start_db: number;
		advance: ShockAdvance;
		max_pulses_per_level: number;
	};
	spacing: { cooling_s: number; settling_s: number; compute_s: number };
	identification: {
		drive_vrms: number;
		duration_s: number;
		nfft: number;
		band_hz: [number, number];
		min_mean_coherence: number;
		review: boolean;
	};
	law: LawDraft;
	control: {
		band_hz: [number, number];
		coherence_floor: number;
		tikhonov_floor: number;
		ref_floor_db: number;
		min_band_energy: number;
		law_budget: number;
		law_misses: number;
		max_pulses: number;
		max_peak_rise_db: number;
		max_bin_rise_db: number;
		first_drive_ratio: number;
		divergence_pulses: number;
	};
	limits: {
		/** Volts peak; NaN until the operator gives it. */
		drive_vpk_ceiling: number;
		drive_vpk_abort: number;
		/** A predicted response peak over it scales the pulse down; null: none. */
		peak_accel_g: number | null;
		monitors: ShockMonitor[];
	};
	feasibility: {
		enabled: boolean;
		/** The shaker record as text (JSON in the controller's shaker-database form). */
		shaker_text: string;
		moving_mass_lb: { armature: number; slip_table: number; drive_adapter: number; fixturing: number; test_article: number };
		residual_velocity_ips: number;
		residual_disp_in: number;
	};
	train: { count: number; spacing_s: number };
	gunfire: { enabled: boolean; rounds_per_min: number; rounds: number };
	capture: { auto_start: boolean; run_name: string };
	metadata: { key: string; label: string; value: string }[];
}

export const DEFAULT_CLASSICAL_LAW = 'fd_correct';

export const SHAPES: { id: ShockShape; label: string }[] = [
	{ id: 'half_sine', label: 'Half-sine' },
	{ id: 'haversine', label: 'Haversine' },
	{ id: 'terminal_peak_sawtooth', label: 'Terminal-peak sawtooth' },
	{ id: 'initial_peak_sawtooth', label: 'Initial-peak sawtooth' },
	{ id: 'trapezoid', label: 'Trapezoid' },
	{ id: 'rectangle', label: 'Rectangle' }
];

export const COMPENSATIONS: { id: CompensationShape; label: string }[] = [
	{ id: 'rectangular', label: 'Rectangular' },
	{ id: 'rounded_rectangular', label: 'Rounded rectangular' },
	{ id: 'half_sine', label: 'Half-sine' }
];

export const STANDARDS: { id: ShockStandardName; label: string; amplitude: number; velocity: number }[] = [
	{ id: 'mil_std_810_516', label: 'MIL-STD-810, Method 516', amplitude: 0.15, velocity: 0.1 },
	{ id: 'iec_60068_2_27', label: 'IEC 60068-2-27', amplitude: 0.2, velocity: 0.15 }
];

export function defaultShockSetup(): ShockDraft {
	return {
		title: 'Half-Sine 20 g 11 ms',
		pulse: {
			shape: 'half_sine',
			amplitude_g: 20,
			duration_ms: 11,
			polarity: 'positive',
			rise_frac: 0.1,
			fall_frac: 0.1,
			compensation: { shape: 'rectangular', pre_frac: 4, post_frac: 4, symmetric: true }
		},
		standard: { name: 'mil_std_810_516', tolerance_abort: 'full_level', overshoot_abort: null },
		acquisition: { fs_hz: 51200, srs: { f_lo_hz: 20, f_hi_hz: 2000, bands_per_octave: 3, band_tol_db: 3 }, pvss: false },
		schedule: {
			mode: 'auto',
			levels: [
				{ db: -12, pulses: 1 },
				{ db: -9, pulses: 1 },
				{ db: -6, pulses: 1 },
				{ db: -3, pulses: 1 },
				{ db: 0, pulses: 3 }
			],
			start_db: -12,
			advance: 'on_pass',
			max_pulses_per_level: 5
		},
		spacing: { cooling_s: 1, settling_s: 0.5, compute_s: 0.25 },
		identification: { drive_vrms: 0.05, duration_s: 4, nfft: 2048, band_hz: [5, 5000], min_mean_coherence: 0.8, review: true },
		law: { id: DEFAULT_CLASSICAL_LAW, params: {} },
		control: {
			band_hz: [5, 5000],
			coherence_floor: 0.9,
			tikhonov_floor: 0.05,
			ref_floor_db: 50,
			min_band_energy: 0.9,
			law_budget: 0.25,
			law_misses: 3,
			max_pulses: 100,
			max_peak_rise_db: 10,
			max_bin_rise_db: 10,
			first_drive_ratio: 2,
			divergence_pulses: 3
		},
		limits: { drive_vpk_ceiling: NaN, drive_vpk_abort: NaN, peak_accel_g: null, monitors: [] },
		feasibility: {
			enabled: false,
			shaker_text: '',
			moving_mass_lb: { armature: 0, slip_table: 0, drive_adapter: 0, fixturing: 0, test_article: 0 },
			residual_velocity_ips: 0.1,
			residual_disp_in: 0.001
		},
		train: { count: 5, spacing_s: 6 },
		gunfire: { enabled: false, rounds_per_min: 30, rounds: 3 },
		capture: { auto_start: false, run_name: 'classical_shock' },
		metadata: [{ key: 'run_number', label: 'Run Number', value: '' }]
	};
}

/** A draft read back from storage, every section filled from the defaults where it is missing. */
export function mergeShockSetup(saved: Partial<ShockDraft> | null): ShockDraft {
	const base = defaultShockSetup();
	if (!saved || typeof saved !== 'object') return base;
	const out = { ...base, ...saved } as ShockDraft;
	out.pulse = { ...base.pulse, ...(saved.pulse ?? {}) };
	out.pulse.compensation = { ...base.pulse.compensation, ...(saved.pulse?.compensation ?? {}) };
	out.standard = { ...base.standard, ...(saved.standard ?? {}) };
	out.acquisition = { ...base.acquisition, ...(saved.acquisition ?? {}) };
	out.acquisition.srs = { ...base.acquisition.srs, ...(saved.acquisition?.srs ?? {}) };
	out.schedule = { ...base.schedule, ...(saved.schedule ?? {}) };
	if (!Array.isArray(out.schedule.levels) || !out.schedule.levels.length) out.schedule.levels = base.schedule.levels;
	out.spacing = { ...base.spacing, ...(saved.spacing ?? {}) };
	out.identification = { ...base.identification, ...(saved.identification ?? {}) };
	out.law = { ...base.law, ...(saved.law ?? {}) };
	out.control = { ...base.control, ...(saved.control ?? {}) };
	out.limits = { ...base.limits, ...(saved.limits ?? {}) };
	if (!Array.isArray(out.limits.monitors)) out.limits.monitors = [];
	out.feasibility = { ...base.feasibility, ...(saved.feasibility ?? {}) };
	out.feasibility.moving_mass_lb = { ...base.feasibility.moving_mass_lb, ...(saved.feasibility?.moving_mass_lb ?? {}) };
	out.train = { ...base.train, ...(saved.train ?? {}) };
	out.gunfire = { ...base.gunfire, ...(saved.gunfire ?? {}) };
	out.capture = { ...base.capture, ...(saved.capture ?? {}) };
	if (!Array.isArray(out.metadata)) out.metadata = base.metadata;
	return out;
}

// ---------------------------------------------------------------------------------------------
// To and from the protocol

/** The configuration the draft describes, every field explicit. Call `localErrors` first. */
export function toShockConfig(d: ShockDraft, law: LawEntry | null): ShockConfig {
	const trapezoid = d.pulse.shape === 'trapezoid';
	const f = d.feasibility;
	const shaker = f.enabled ? shakerRecord(f.shaker_text) : null;
	return {
		title: d.title.trim(),
		pulse: {
			shape: d.pulse.shape,
			amplitude_g: d.pulse.amplitude_g,
			duration_ms: d.pulse.duration_ms,
			polarity: d.pulse.polarity,
			rise_frac: trapezoid ? d.pulse.rise_frac : null,
			fall_frac: trapezoid ? d.pulse.fall_frac : null,
			compensation: { ...d.pulse.compensation }
		},
		standard: { name: d.standard.name, tolerance_abort: d.standard.tolerance_abort, overshoot_abort: d.standard.overshoot_abort },
		acquisition: { fs_hz: d.acquisition.fs_hz, srs: { ...d.acquisition.srs }, pvss: d.acquisition.pvss },
		schedule: {
			mode: d.schedule.mode,
			levels_db: d.schedule.levels.map((l) => l.db),
			pulses: d.schedule.levels.map((l) => l.pulses),
			start_db: d.schedule.start_db,
			advance: d.schedule.advance,
			max_pulses_per_level: d.schedule.max_pulses_per_level
		},
		spacing: { ...d.spacing },
		identification: { ...d.identification, band_hz: [d.identification.band_hz[0], d.identification.band_hz[1]] },
		control: {
			law: { id: d.law.id, params: lawWireParams(law, d.law.params) },
			...d.control,
			band_hz: [d.control.band_hz[0], d.control.band_hz[1]]
		},
		limits: {
			drive_vpk_ceiling: d.limits.drive_vpk_ceiling,
			drive_vpk_abort: d.limits.drive_vpk_abort,
			peak_accel_g: d.limits.peak_accel_g,
			monitors: d.limits.monitors.map((m) => ({ ...m }))
		},
		feasibility:
			f.enabled && shaker
				? { shaker, moving_mass_lb: { ...f.moving_mass_lb }, residual_velocity_ips: f.residual_velocity_ips, residual_disp_in: f.residual_disp_in }
				: null,
		train: { ...d.train },
		gunfire: d.gunfire.enabled ? { rounds_per_min: d.gunfire.rounds_per_min, rounds: d.gunfire.rounds } : null,
		capture: { ...d.capture },
		metadata: d.metadata.map((m) => ({ ...m }))
	};
}

/**
 * The draft for a configuration the service holds (another screen applied it, or this one before
 * a reload). Fields the configuration leaves out take the wire's defaults where the wire has one,
 * and the shell's where it does not; one pulse count for every level is spread over the levels.
 */
export function fromShockConfig(c: ShockConfig): ShockDraft {
	const base = defaultShockSetup();
	const levels = c.schedule.levels_db.map((db, i) => ({ db, pulses: c.schedule.pulses.length === 1 ? c.schedule.pulses[0] : (c.schedule.pulses[i] ?? NaN) }));
	const comp = c.pulse.compensation;
	return {
		title: c.title,
		pulse: {
			shape: c.pulse.shape,
			amplitude_g: c.pulse.amplitude_g,
			duration_ms: c.pulse.duration_ms,
			polarity: c.pulse.polarity ?? 'positive',
			rise_frac: c.pulse.rise_frac ?? base.pulse.rise_frac,
			fall_frac: c.pulse.fall_frac ?? base.pulse.fall_frac,
			compensation: comp ? { ...comp } : { shape: 'rectangular', pre_frac: 4, post_frac: 4, symmetric: true }
		},
		standard: {
			name: c.standard?.name ?? 'mil_std_810_516',
			tolerance_abort: c.standard?.tolerance_abort ?? 'full_level',
			overshoot_abort: c.standard?.overshoot_abort ?? null
		},
		acquisition: {
			fs_hz: c.acquisition?.fs_hz ?? base.acquisition.fs_hz,
			srs: { ...base.acquisition.srs, ...(c.acquisition?.srs ?? {}) },
			pvss: c.acquisition?.pvss ?? false
		},
		schedule: {
			mode: c.schedule.mode,
			levels,
			start_db: c.schedule.start_db ?? levels[0]?.db ?? base.schedule.start_db,
			advance: c.schedule.advance ?? 'count',
			max_pulses_per_level: c.schedule.max_pulses_per_level ?? base.schedule.max_pulses_per_level
		},
		spacing: { ...base.spacing, ...(c.spacing ?? {}) },
		identification: {
			...base.identification,
			...(c.identification ?? {}),
			band_hz: c.identification?.band_hz ? [c.identification.band_hz[0], c.identification.band_hz[1]] : base.identification.band_hz
		},
		law: { id: c.control?.law?.id ?? DEFAULT_CLASSICAL_LAW, params: { ...((c.control?.law?.params ?? {}) as LawDraft['params']) } },
		control: (() => {
			const { law: _law, ...rest } = c.control ?? {};
			void _law;
			const out = { ...base.control, ...rest } as ShockDraft['control'];
			out.band_hz = rest.band_hz ? [rest.band_hz[0], rest.band_hz[1]] : base.control.band_hz;
			return out;
		})(),
		limits: {
			drive_vpk_ceiling: c.limits.drive_vpk_ceiling,
			drive_vpk_abort: c.limits.drive_vpk_abort,
			peak_accel_g: c.limits.peak_accel_g ?? null,
			monitors: (c.limits.monitors ?? []).map((m) => ({ ...m }))
		},
		feasibility: c.feasibility
			? {
					enabled: true,
					shaker_text: JSON.stringify(c.feasibility.shaker, null, 2),
					moving_mass_lb: { ...c.feasibility.moving_mass_lb },
					residual_velocity_ips: c.feasibility.residual_velocity_ips ?? base.feasibility.residual_velocity_ips,
					residual_disp_in: c.feasibility.residual_disp_in ?? base.feasibility.residual_disp_in
				}
			: base.feasibility,
		train: c.train ? { ...c.train } : { count: 1, spacing_s: base.train.spacing_s },
		gunfire: c.gunfire ? { enabled: true, ...c.gunfire } : { ...base.gunfire, enabled: false },
		capture: { ...base.capture, ...(c.capture ?? {}) },
		metadata: (c.metadata ?? []).map((m) => ({ ...m }))
	};
}

/** What `shock.preview` is asked for the draft's pulse, or null while it holds no numbers. */
export function previewParams(d: ShockDraft): ShockPreviewParams | null {
	const p = d.pulse;
	const nums = [p.amplitude_g, p.duration_ms, p.compensation.pre_frac, p.compensation.post_frac, d.acquisition.fs_hz];
	if (p.shape === 'trapezoid') nums.push(p.rise_frac, p.fall_frac);
	if (!nums.every(Number.isFinite)) return null;
	const trapezoid = p.shape === 'trapezoid';
	return {
		pulse: {
			shape: p.shape,
			amplitude_g: p.amplitude_g,
			duration_ms: p.duration_ms,
			polarity: p.polarity,
			rise_frac: trapezoid ? p.rise_frac : null,
			fall_frac: trapezoid ? p.fall_frac : null,
			compensation: { ...p.compensation }
		},
		standard: { name: d.standard.name, tolerance_abort: d.standard.tolerance_abort, overshoot_abort: d.standard.overshoot_abort },
		fs_hz: d.acquisition.fs_hz
	};
}

/** A new drive monitor on a table row (its unit is the row's). */
export function newMonitor(row: ChannelRow | undefined): ShockMonitor {
	return { channel: row?.index ?? 0, unit: row?.units ?? 'A', peak_ceiling: NaN, kind: 'between_pulse' };
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

/** The setup's sections, in the screen's order. */
export type ShockSection = 'station' | 'pulse' | 'standard' | 'schedule' | 'identification' | 'control' | 'limits' | 'trains';

export const SHOCK_SECTIONS: { id: ShockSection; label: string }[] = [
	{ id: 'station', label: 'Station & Channels' },
	{ id: 'pulse', label: 'Pulse' },
	{ id: 'standard', label: 'Standard' },
	{ id: 'schedule', label: 'Schedule' },
	{ id: 'identification', label: 'Identification' },
	{ id: 'control', label: 'Control' },
	{ id: 'limits', label: 'Limits' },
	{ id: 'trains', label: 'Trains & Capture' }
];

/**
 * The editor's key for a service finding. A field path loses its `config.` head and its brackets;
 * the level table's lists name the level (`schedule.levels_db[2]` is `schedule.levels.2.db`); the
 * law's path is the law panel's (`control.law.params.beta` is `law.params.beta`). A finding about
 * the channel table's control or drive row is the station's.
 */
export function shockItemKey(item: Pick<ReportItem, 'field' | 'channel'> & { reason: string }): string {
	if (item.reason === 'no_control_channel' || item.reason === 'no_drive_channel') return 'station.channels';
	if (item.reason === 'limit_unknown_channel' && !item.field) return 'limits.monitors';
	if (item.field) {
		const path = item.field.replace(/^config\./, '').replace(/\[(\d+)\]/g, '.$1');
		let m = /^schedule\.levels_db\.(\d+)$/.exec(path);
		if (m) return `schedule.levels.${m[1]}.db`;
		m = /^schedule\.pulses\.(\d+)$/.exec(path);
		if (m) return `schedule.levels.${m[1]}.pulses`;
		if (path === 'schedule.levels_db') return 'schedule.levels';
		if (path.startsWith('control.law')) return path.replace(/^control\.law(\.id)?/, (_, id) => (id ? 'law.id' : 'law')).replace(/^law$/, 'law.id');
		return path;
	}
	if (item.channel !== undefined && item.channel !== null) return 'station.channels';
	return 'setup';
}

export function shockByKey(items: { field?: string | null; channel?: number | null; reason: string; message: string }[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const k = shockItemKey(it as Pick<ReportItem, 'field' | 'channel'> & { reason: string });
		if (!(out[k] ??= []).includes(it.message)) out[k].push(it.message);
	}
	return out;
}

/** The section a key belongs to. */
export function shockSectionOf(key: string): ShockSection {
	if (/^station|^table|^channel/.test(key)) return 'station';
	if (/^(title|pulse)(\.|$)/.test(key)) return 'pulse';
	if (/^(standard|acquisition)(\.|$)/.test(key)) return 'standard';
	if (/^(schedule|spacing)(\.|$)/.test(key)) return 'schedule';
	if (/^identification(\.|$)/.test(key)) return 'identification';
	if (/^(law|control)(\.|$)/.test(key)) return 'control';
	if (/^(limits|feasibility)(\.|$)/.test(key)) return 'limits';
	if (/^(train|gunfire|capture|metadata)(\.|$)/.test(key)) return 'trains';
	return 'pulse';
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

function needOptional(map: FieldErrors, key: string, v: number | null) {
	if (v !== null && !Number.isFinite(v)) add(map, key, NUMBER);
}

/**
 * Fields that hold no number, a title that is empty, a drive ceiling or abort not given (or the
 * abort not above the ceiling), a shaker record that does not read. Everything else, the pulse's
 * physics included, is the service's to judge in the dry run.
 */
export function localErrors(d: ShockDraft, rows: ChannelRow[] = []): FieldErrors {
	const out: FieldErrors = {};
	if (!d.title.trim()) add(out, 'title', 'Give the test a title.');
	const p = d.pulse;
	need(out, 'pulse.amplitude_g', p.amplitude_g);
	need(out, 'pulse.duration_ms', p.duration_ms);
	if (p.shape === 'trapezoid') {
		need(out, 'pulse.rise_frac', p.rise_frac);
		need(out, 'pulse.fall_frac', p.fall_frac);
	}
	need(out, 'pulse.compensation.pre_frac', p.compensation.pre_frac);
	if (!p.compensation.symmetric) need(out, 'pulse.compensation.post_frac', p.compensation.post_frac);
	needOptional(out, 'standard.overshoot_abort', d.standard.overshoot_abort);
	need(out, 'acquisition.fs_hz', d.acquisition.fs_hz);
	for (const k of ['f_lo_hz', 'f_hi_hz', 'band_tol_db'] as const) need(out, `acquisition.srs.${k}`, d.acquisition.srs[k]);
	need(out, 'acquisition.srs.bands_per_octave', d.acquisition.srs.bands_per_octave, true);
	const s = d.schedule;
	if (!s.levels.length) add(out, 'schedule.levels', 'Give at least one level.');
	s.levels.forEach((l, i) => {
		need(out, `schedule.levels.${i}.db`, l.db);
		need(out, `schedule.levels.${i}.pulses`, l.pulses, true);
	});
	if (s.mode === 'manual') need(out, 'schedule.start_db', s.start_db);
	if (s.advance === 'on_pass') need(out, 'schedule.max_pulses_per_level', s.max_pulses_per_level, true);
	for (const k of ['cooling_s', 'settling_s', 'compute_s'] as const) need(out, `spacing.${k}`, d.spacing[k]);
	const id = d.identification;
	for (const k of ['drive_vrms', 'duration_s', 'min_mean_coherence'] as const) need(out, `identification.${k}`, id[k]);
	need(out, 'identification.nfft', id.nfft, true);
	if (!(Number.isFinite(id.band_hz[0]) && Number.isFinite(id.band_hz[1]))) add(out, 'identification.band_hz', 'Give both edges of the band.');
	const c = d.control;
	if (!(Number.isFinite(c.band_hz[0]) && Number.isFinite(c.band_hz[1]))) add(out, 'control.band_hz', 'Give both edges of the band.');
	for (const k of ['coherence_floor', 'tikhonov_floor', 'ref_floor_db', 'min_band_energy', 'law_budget', 'max_peak_rise_db', 'max_bin_rise_db', 'first_drive_ratio'] as const) need(out, `control.${k}`, c[k]);
	for (const k of ['law_misses', 'max_pulses', 'divergence_pulses'] as const) need(out, `control.${k}`, c[k], true);
	const lim = d.limits;
	if (!Number.isFinite(lim.drive_vpk_ceiling)) add(out, 'limits.drive_vpk_ceiling', 'Give the drive ceiling in volts peak. There is no default: it is your amplifier input.');
	if (!Number.isFinite(lim.drive_vpk_abort)) add(out, 'limits.drive_vpk_abort', 'Give the drive abort in volts peak, above the ceiling.');
	else if (Number.isFinite(lim.drive_vpk_ceiling) && lim.drive_vpk_abort <= lim.drive_vpk_ceiling) add(out, 'limits.drive_vpk_abort', 'The drive abort is above the drive ceiling.');
	needOptional(out, 'limits.peak_accel_g', lim.peak_accel_g);
	lim.monitors.forEach((m, i) => {
		need(out, `limits.monitors.${i}.peak_ceiling`, m.peak_ceiling);
		if (rows.length && !rows.some((r) => r.index === m.channel && r.enabled)) add(out, `limits.monitors.${i}.channel`, `Channel ${m.channel} is not an enabled row of the channel table.`);
	});
	const f = d.feasibility;
	if (f.enabled) {
		if (!f.shaker_text.trim()) add(out, 'feasibility.shaker', 'Load or paste the shaker record, or turn the check off.');
		else if (!shakerRecord(f.shaker_text)) add(out, 'feasibility.shaker', 'The shaker record does not read as a record (JSON object).');
		for (const [k, v] of Object.entries(f.moving_mass_lb)) need(out, `feasibility.moving_mass_lb.${k}`, v);
		need(out, 'feasibility.residual_velocity_ips', f.residual_velocity_ips);
		need(out, 'feasibility.residual_disp_in', f.residual_disp_in);
	}
	need(out, 'train.count', d.train.count, true);
	need(out, 'train.spacing_s', d.train.spacing_s);
	if (d.gunfire.enabled) {
		need(out, 'gunfire.rounds_per_min', d.gunfire.rounds_per_min);
		need(out, 'gunfire.rounds', d.gunfire.rounds, true);
	}
	if (d.capture.auto_start && !d.capture.run_name.trim()) add(out, 'capture.run_name', 'Name the capture, or turn auto start off.');
	return out;
}

/** The control and drive rows the run uses: the table's first enabled control and drive rows. */
export function runChannels(rows: ChannelRow[]): { control: ChannelRow | null; drive: ChannelRow | null } {
	return {
		control: rows.find((r) => r.enabled && r.role === 'control') ?? null,
		drive: rows.find((r) => r.enabled && r.role === 'drive') ?? null
	};
}
