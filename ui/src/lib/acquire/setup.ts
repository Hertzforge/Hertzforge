/**
 * The Random setup as the operator edits it, and its conversion to the protocol's `RandomConfig`.
 *
 * Numbers are held as numbers; a field that holds no number is NaN, which `localErrors` names
 * before anything is sent. The service is the authority on everything else: its dry run
 * (`random.configure` with `dry_run`) names the remaining errors, located by row, field or
 * channel, and `fieldPath` keys both kinds the same way so a field shows them together.
 */

import type {
	BandDb,
	Breakpoint,
	CompositeMode,
	ControlParams,
	LadderMode,
	LimitProfile,
	RandomConfig,
	ReportItem
} from '$lib/protocol/types';

export interface BreakpointDraft {
	freq_hz: number;
	/** A PSD anchor, or a slope from the row before. */
	kind: 'psd' | 'slope';
	psd_g2_hz: number;
	slope_db_oct: number;
	/** The row's own bands, or null to use the profile's. */
	alarm_db: BandDb | null;
	abort_db: BandDb | null;
}

export interface LevelDraft {
	db: number;
	/** Ignored on the last level, which holds for `full_level_s`. */
	dwell_s: number;
}

export interface ControlDraft {
	composite: CompositeMode;
	weights: { channel: number; weight: number }[];
	control_dof: number;
	damping: number;
	max_lines_out: number;
	max_loops_out: number;
	/** null: no clip. */
	max_sigma: number | null;
	drive_ceiling_vrms: number;
	/** null: no drive abort. */
	drive_abort_vrms: number | null;
	coherence_floor: number;
	review_after_sysid: boolean;
	/** level_db: the control response identification steps up to, dB relative to the reference. */
	pretest: { averages: number; level_db: number; max_vrms: number | null; start_vrms: number; shaped: boolean };
	soft_start_db: number;
	soft_start_relax_db: number;
	level_ramp_s: number;
	shutdown_ramp_s: number;
}

export interface FeasibilityDraft {
	enabled: boolean;
	/** The shaker record as text (JSON in the controller's shaker-database form). */
	shaker_text: string;
	/** Where the record came from, for the operator's eye. */
	shaker_source: string | null;
	moving_mass_lb: { armature: number; slip_table: number; drive_adapter: number; fixturing: number; test_article: number };
	crest_factor: number;
}

export interface SetupDraft {
	title: string;
	breakpoints: BreakpointDraft[];
	alarm_db: BandDb;
	abort_db: BandDb;
	acquisition: { fs_hz: number; n_pts: number; n_avg: number };
	schedule: { mode: LadderMode; levels: LevelDraft[]; full_level_s: number };
	control: ControlDraft;
	limits: LimitProfile[];
	feasibility: FeasibilityDraft;
}

/** The protocol's control defaults (PROTOCOL.md, "Random Test Configuration"). */
export function defaultControl(): ControlDraft {
	return {
		composite: 'weighted_avg',
		weights: [],
		control_dof: 120,
		damping: 4,
		max_lines_out: 10,
		max_loops_out: 5,
		max_sigma: 5,
		drive_ceiling_vrms: 10,
		drive_abort_vrms: null,
		coherence_floor: 0.5,
		review_after_sysid: true,
		pretest: { averages: 4, level_db: -20, max_vrms: 1, start_vrms: 0.01, shaped: false },
		soft_start_db: 9,
		soft_start_relax_db: 6,
		level_ramp_s: 3,
		shutdown_ramp_s: 3
	};
}

export function newBreakpoint(freq_hz: number, kind: 'psd' | 'slope', value: number): BreakpointDraft {
	return {
		freq_hz,
		kind,
		psd_g2_hz: kind === 'psd' ? value : NaN,
		slope_db_oct: kind === 'slope' ? value : NaN,
		alarm_db: null,
		abort_db: null
	};
}

/** A qualification profile, 20 to 2000 Hz, ready to arm on the simulator. */
export function defaultSetup(): SetupDraft {
	return {
		title: 'Qualification Random',
		breakpoints: [newBreakpoint(20, 'psd', 0.01), newBreakpoint(80, 'slope', 3), newBreakpoint(350, 'psd', 0.04), newBreakpoint(2000, 'slope', -3)],
		alarm_db: [3, 3],
		abort_db: [6, 6],
		acquisition: { fs_hz: 8192, n_pts: 2048, n_avg: 4 },
		schedule: {
			mode: 'auto',
			levels: [
				{ db: -12, dwell_s: 30 },
				{ db: -6, dwell_s: 30 },
				{ db: 0, dwell_s: 0 }
			],
			full_level_s: 300
		},
		control: defaultControl(),
		limits: [],
		feasibility: {
			enabled: false,
			shaker_text: '',
			shaker_source: null,
			moving_mass_lb: { armature: 0, slip_table: 0, drive_adapter: 0, fixturing: 0, test_article: 0 },
			crest_factor: 3
		}
	};
}

/** FFT lengths the setup offers. */
export const N_PTS_OPTIONS = [256, 512, 1024, 2048, 4096, 8192, 16384];

// ---------------------------------------------------------------------------------------------
// To and from the protocol

const finiteOrNull = (v: number | null): number | null => (v === null || !Number.isFinite(v) ? null : v);

export function shakerRecord(text: string): Record<string, unknown> | null {
	try {
		const parsed: unknown = JSON.parse(text);
		return typeof parsed === 'object' && parsed !== null && !Array.isArray(parsed) ? (parsed as Record<string, unknown>) : null;
	} catch {
		return null;
	}
}

/** The configuration the draft describes. Call `localErrors` first: NaN fields go out as null. */
export function toConfig(d: SetupDraft): RandomConfig {
	const breakpoints: Breakpoint[] = d.breakpoints.map((b) => {
		const row = (b.kind === 'psd' ? { freq_hz: b.freq_hz, psd_g2_hz: b.psd_g2_hz } : { freq_hz: b.freq_hz, slope_db_oct: b.slope_db_oct }) as Breakpoint;
		if (b.alarm_db) row.alarm_db = [b.alarm_db[0], b.alarm_db[1]];
		if (b.abort_db) row.abort_db = [b.abort_db[0], b.abort_db[1]];
		return row;
	});
	const last = d.schedule.levels.length - 1;
	const c = d.control;
	const control: ControlParams = {
		composite: c.composite,
		weights: c.weights.map((w) => ({ channel: w.channel, weight: w.weight })),
		control_dof: c.control_dof,
		damping: c.damping,
		max_lines_out: c.max_lines_out,
		max_loops_out: c.max_loops_out,
		max_sigma: finiteOrNull(c.max_sigma),
		drive_ceiling_vrms: c.drive_ceiling_vrms,
		drive_abort_vrms: finiteOrNull(c.drive_abort_vrms),
		coherence_floor: c.coherence_floor,
		review_after_sysid: c.review_after_sysid,
		pretest: { averages: c.pretest.averages, level_db: c.pretest.level_db, max_vrms: finiteOrNull(c.pretest.max_vrms), start_vrms: c.pretest.start_vrms, shaped: c.pretest.shaped },
		soft_start_db: c.soft_start_db,
		soft_start_relax_db: c.soft_start_relax_db,
		level_ramp_s: c.level_ramp_s,
		shutdown_ramp_s: c.shutdown_ramp_s
	};
	const f = d.feasibility;
	const shaker = f.enabled ? shakerRecord(f.shaker_text) : null;
	return {
		title: d.title.trim(),
		profile: { breakpoints, alarm_db: [d.alarm_db[0], d.alarm_db[1]], abort_db: [d.abort_db[0], d.abort_db[1]] },
		acquisition: { fs_hz: d.acquisition.fs_hz, n_pts: d.acquisition.n_pts, n_avg: d.acquisition.n_avg },
		schedule: {
			mode: d.schedule.mode,
			levels: d.schedule.levels.map((l, i) => (i === last ? { db: l.db } : { db: l.db, dwell_s: l.dwell_s })),
			full_level_s: d.schedule.full_level_s
		},
		control,
		limits: d.limits.map((l) => ({ channel: l.channel, kind: l.kind, rows: l.rows.map((r) => ({ freq_hz: r.freq_hz, ceiling: r.ceiling })) })),
		feasibility:
			f.enabled && shaker
				? { shaker, moving_mass_lb: { ...f.moving_mass_lb }, crest_factor: f.crest_factor }
				: null
	};
}

/** JSON with every object's keys in order, so the same value always gives the same text. */
function canonical(v: unknown): string {
	if (Array.isArray(v)) return `[${v.map(canonical).join(',')}]`;
	if (v !== null && typeof v === 'object') {
		const o = v as Record<string, unknown>;
		const keys = Object.keys(o)
			.filter((k) => o[k] !== undefined)
			.sort();
		return `{${keys.map((k) => `${JSON.stringify(k)}:${canonical(o[k])}`).join(',')}}`;
	}
	return JSON.stringify(v) ?? 'null';
}

/**
 * The identity of a configuration, for telling whether the editor holds what the service holds.
 * The service hands a configuration back with its keys in its own order, so keys are sorted. A
 * configuration read back goes through `fromConfig` and `toConfig` first, so anything it left to
 * the defaults is filled in the way the editor fills it.
 */
export function configKey(config: RandomConfig): string {
	return canonical(config);
}

/** A draft from a configuration, for loading one back into the editor. */
export function fromConfig(config: RandomConfig): SetupDraft {
	const base = defaultSetup();
	const ctl = { ...base.control, ...(config.control ?? {}) } as ControlDraft;
	ctl.pretest = { ...base.control.pretest, ...(config.control?.pretest ?? {}) } as ControlDraft['pretest'];
	ctl.weights = (config.control?.weights ?? []).map((w) => ({ ...w }));
	return {
		title: config.title,
		breakpoints: config.profile.breakpoints.map((b) => ({
			freq_hz: b.freq_hz,
			kind: b.psd_g2_hz !== undefined ? 'psd' : 'slope',
			psd_g2_hz: b.psd_g2_hz ?? NaN,
			slope_db_oct: b.slope_db_oct ?? NaN,
			alarm_db: b.alarm_db ? [b.alarm_db[0], b.alarm_db[1]] : null,
			abort_db: b.abort_db ? [b.abort_db[0], b.abort_db[1]] : null
		})),
		alarm_db: config.profile.alarm_db ?? [3, 3],
		abort_db: config.profile.abort_db ?? [6, 6],
		acquisition: { ...config.acquisition },
		schedule: {
			mode: config.schedule.mode,
			levels: config.schedule.levels.map((l) => ({ db: l.db, dwell_s: l.dwell_s ?? 0 })),
			full_level_s: config.schedule.full_level_s
		},
		control: ctl,
		limits: (config.limits ?? []).map((l) => ({ ...l, rows: l.rows.map((r) => ({ ...r })) })),
		feasibility: config.feasibility
			? {
					enabled: true,
					shaker_text: JSON.stringify(config.feasibility.shaker, null, 2),
					shaker_source: null,
					moving_mass_lb: { ...config.feasibility.moving_mass_lb },
					crest_factor: config.feasibility.crest_factor ?? 3
				}
			: base.feasibility
	};
}

// ---------------------------------------------------------------------------------------------
// Where a finding belongs

/**
 * The editor's key for a field: the protocol's field path (`acquisition.n_pts`,
 * `schedule.levels.1`, `control.pretest.averages`, `limits.0.rows.2`) and, for breakpoint rows,
 * `profile.breakpoints.<row>.<column>`.
 */
export type FieldKey = string;

/** Findings keyed by the field they belong to. A key's list holds every message for it. */
export type FieldErrors = Record<FieldKey, string[]>;

function add(map: FieldErrors, key: FieldKey, message: string) {
	(map[key] ??= []).push(message);
}

const BREAKPOINT_COLUMN: Partial<Record<ReportItem['reason'], string>> = {
	first_row_not_anchor: 'kind',
	row_level_ambiguous: 'kind',
	frequency_not_positive: 'freq_hz',
	frequency_not_ascending: 'freq_hz',
	psd_negative: 'value',
	alarm_wider_than_abort: 'bands'
};

/** The key a service finding belongs to, or null for one about a channel table row. */
export function itemKey(item: ReportItem): FieldKey | null {
	if (item.row !== undefined && item.row !== null) {
		return `profile.breakpoints.${item.row}.${BREAKPOINT_COLUMN[item.reason] ?? 'row'}`;
	}
	if (item.field) return item.field;
	if (item.channel !== undefined && item.channel !== null) {
		// Weights and limits name the channel they refer to.
		if (item.reason === 'weight_unknown_channel') return 'control.weights';
		if (item.reason === 'limit_unknown_channel') return 'limits';
		return null;
	}
	return null;
}

/** Group service findings by field; findings about channel table rows are left out. */
export function byField(items: ReportItem[]): FieldErrors {
	const out: FieldErrors = {};
	for (const it of items) {
		const key = itemKey(it);
		if (key) add(out, key, it.message);
	}
	return out;
}

/** Messages for a key and everything under it (`schedule.levels` also gathers `schedule.levels.2`). */
export function under(map: FieldErrors, key: FieldKey): string[] {
	const out: string[] = [];
	for (const [k, v] of Object.entries(map)) if (k === key || k.startsWith(`${key}.`)) out.push(...v);
	return out;
}

/** Whether a finding is about the channel table rather than the Random setup. */
export function isTableItem(item: ReportItem): boolean {
	return itemKey(item) === null && item.row === undefined;
}

// ---------------------------------------------------------------------------------------------
// What the editor can say before anything is sent

const NUMBER = 'Enter a number.';
const WHOLE = 'Enter a whole number.';

function needNumber(map: FieldErrors, key: FieldKey, v: number, whole = false) {
	if (!Number.isFinite(v)) add(map, key, NUMBER);
	else if (whole && !Number.isInteger(v)) add(map, key, WHOLE);
}

function needOptional(map: FieldErrors, key: FieldKey, v: number | null) {
	if (v !== null && !Number.isFinite(v)) add(map, key, NUMBER);
}

/**
 * Fields that hold no number, a title that is empty, a shaker record that does not read.
 * Everything else is the service's to judge.
 */
export function localErrors(d: SetupDraft): FieldErrors {
	const out: FieldErrors = {};
	if (!d.title.trim()) add(out, 'title', 'Give the test a title.');
	d.breakpoints.forEach((b, i) => {
		const k = `profile.breakpoints.${i}`;
		needNumber(out, `${k}.freq_hz`, b.freq_hz);
		needNumber(out, `${k}.value`, b.kind === 'psd' ? b.psd_g2_hz : b.slope_db_oct);
		for (const band of [b.alarm_db, b.abort_db]) {
			if (band && !(Number.isFinite(band[0]) && Number.isFinite(band[1]))) add(out, `${k}.bands`, 'Give both sides of the band, or use the profile bands.');
		}
	});
	for (const [key, band] of [
		['profile.alarm_db', d.alarm_db],
		['profile.abort_db', d.abort_db]
	] as const) {
		if (!(Number.isFinite(band[0]) && Number.isFinite(band[1]))) add(out, key, 'Give both sides of the band.');
	}
	needNumber(out, 'acquisition.fs_hz', d.acquisition.fs_hz);
	needNumber(out, 'acquisition.n_pts', d.acquisition.n_pts, true);
	needNumber(out, 'acquisition.n_avg', d.acquisition.n_avg, true);
	const last = d.schedule.levels.length - 1;
	d.schedule.levels.forEach((l, i) => {
		needNumber(out, `schedule.levels.${i}.db`, l.db);
		if (i < last) needNumber(out, `schedule.levels.${i}.dwell_s`, l.dwell_s);
	});
	needNumber(out, 'schedule.full_level_s', d.schedule.full_level_s);
	const c = d.control;
	for (const key of ['control_dof', 'damping', 'drive_ceiling_vrms', 'coherence_floor', 'soft_start_db', 'soft_start_relax_db', 'level_ramp_s', 'shutdown_ramp_s'] as const) {
		needNumber(out, `control.${key}`, c[key]);
	}
	needNumber(out, 'control.max_lines_out', c.max_lines_out, true);
	needNumber(out, 'control.max_loops_out', c.max_loops_out, true);
	needOptional(out, 'control.max_sigma', c.max_sigma);
	needOptional(out, 'control.drive_abort_vrms', c.drive_abort_vrms);
	needNumber(out, 'control.pretest.averages', c.pretest.averages, true);
	needNumber(out, 'control.pretest.level_db', c.pretest.level_db);
	needOptional(out, 'control.pretest.max_vrms', c.pretest.max_vrms);
	needNumber(out, 'control.pretest.start_vrms', c.pretest.start_vrms);
	c.weights.forEach((w, i) => needNumber(out, `control.weights.${i}`, w.weight));
	d.limits.forEach((lim, i) =>
		lim.rows.forEach((r, k) => {
			needNumber(out, `limits.${i}.rows.${k}`, r.freq_hz);
			needNumber(out, `limits.${i}.rows.${k}`, r.ceiling);
		})
	);
	const f = d.feasibility;
	if (f.enabled) {
		if (!f.shaker_text.trim()) add(out, 'feasibility.shaker', 'Load or paste the shaker record, or turn the check off.');
		else if (!shakerRecord(f.shaker_text)) add(out, 'feasibility.shaker', 'The shaker record does not read as a record (JSON object).');
		for (const [k, v] of Object.entries(f.moving_mass_lb)) needNumber(out, `feasibility.moving_mass_lb.${k}`, v);
		needNumber(out, 'feasibility.crest_factor', f.crest_factor);
	}
	return out;
}

export function errorCount(map: FieldErrors): number {
	return Object.values(map).reduce((n, v) => n + v.length, 0);
}

/** Merge two field maps without repeating a message. */
export function mergeErrors(a: FieldErrors, b: FieldErrors): FieldErrors {
	const out: FieldErrors = {};
	for (const map of [a, b]) for (const [k, v] of Object.entries(map)) for (const m of v) if (!(out[k] ??= []).includes(m)) out[k].push(m);
	return out;
}

/** What the profile editor reads and edits: a breakpoint table, its bands, and the findings on them. */
export interface ProfileHost {
	setup: { title: string; breakpoints: BreakpointDraft[]; alarm_db: BandDb; abort_db: BandDb };
	locked: boolean;
	fieldErrors: FieldErrors;
	fieldWarnings: FieldErrors;
}

/** What the schedule editor reads and edits: the level schedule and the findings on it. */
export interface ScheduleHost {
	setup: { schedule: { mode: LadderMode; levels: LevelDraft[]; full_level_s: number } };
	locked: boolean;
	fieldErrors: FieldErrors;
}
