/**
 * MIMO Mixed Mode in mock mode: a made-up run that walks every state the screens show, on the
 * control service's wire (`$lib/protocol/mimoMixed`, schema/PROTOCOL.md "MIMO Mixed Mode").
 * Configure reads the controller's own configuration and refuses as the service does (a missing
 * member is `config_unreadable`, the controller's first refusal is `field: "config"`, the table's
 * are `controls[k]`, `limits[k]` and `drives[k]`, the laws' are `law.<role>...`); the reference is
 * built from the breakpoint tables; Start identifies, gates on the coherence and holds for review
 * with the prediction and the budget forecast; Proceed (or an override of a hold) plays the floor
 * alone until it equalizes, then brings the tones and narrowbands in and climbs the level
 * schedule. Every number is made up so that every panel can be seen: it is not a measurement and
 * nothing here controls anything.
 *
 * The story: at full level the article softens under the first tone, its drive on the first
 * shaker climbs, and that shaker runs out of room; the ceiling rule scales the tones down (every
 * tone by one factor under `common_scale`, the last tone under `in_turn`), the tones and the
 * bands show as limiting until the run completes and ramps down. The station's clock runs ten
 * times the wall clock, so the configuration's times hold as given and a run takes minutes.
 *
 * To see a review hold: a coherence gate above the mock's worst coherence (0.81 with four
 * controls) fails the gate; a drive whose peak ceiling is under what the prediction needs holds
 * at the prediction.
 */

import type { ChannelRow, DataSource, LogEntry, RefusalReason, RunRecordRef, Series, SimulatorChoice, SpectrumView, StationRef, View } from '../types';
import { packedIndex, type LawEntry, type MimoBuildBreakpointsParams, type MimoReferenceResult } from '../mimo';
import {
	defaultAbortVpk,
	MIMO_MIXED_DEFAULTS,
	MIMO_MIXED_NOT_BUILT,
	MIMO_MIXED_QUANTITIES,
	type MimoMixedBandBody,
	type MimoMixedConfig,
	type MimoMixedDerived,
	type MimoMixedDriveRow,
	type MimoMixedEnd,
	type MimoMixedIdentificationView,
	type MimoMixedLawRole,
	type MimoMixedLiveHead,
	type MimoMixedOutcome,
	type MimoMixedOverride,
	type MimoMixedOverrideReason,
	type MimoMixedPart,
	type MimoMixedPartKind,
	type MimoMixedPartRow,
	type MimoMixedPhase,
	type MimoMixedPrediction,
	type MimoMixedQuantity,
	type MimoMixedReportItem,
	type MimoMixedRunState,
	type MimoMixedSchedule,
	type MimoMixedState,
	type MimoMixedToneBody
} from '../mimoMixed';
import { MockError, type MockConnection, type MockMethod } from './socket';

export interface MimoMixedMockHost {
	speed: number;
	timeoutS: number;
	own(conn: MockConnection): void;
	hasController(): boolean;
	otherRun(): string | null;
	broadcast(topic: string, data: Record<string, unknown>): void;
	rows(): ChannelRow[];
	station(): { source: DataSource; ref: StationRef };
	/** The station's simulator models: the sine structure a simulated run plays. */
	models(): SimulatorChoice;
	clientName(conn: MockConnection): string;
	/** The tones' laws, world `sine_mimo`; absent: only the built-in `matrix`. */
	toneLaws?(): LawEntry[];
	record?: 'unwritable' | 'problem';
	recordFolder?(runId: string): string;
}

export interface MimoMixedMock {
	methods: Record<string, MockMethod>;
	isLive(): boolean;
	liveId(): string | null;
	detach(conn: MockConnection): void;
	seatExpired(): void;
	/** The floor's laws, world `mimo`. */
	floorLaws(): LawEntry[];
}

const TICK_MS = 50;
const LIVE_MS = 200;
/** Station seconds per wall second, before the host's speed. */
const CLOCK_RATE = 10;
/** Station seconds: the identification, the gate, the floor's least time alone. */
const SYSID_S = 6;
const GATE_S = 1;
const FLOOR_S = 3;
const GRID_POINTS = 256;
const HISTORY = 120;
const FORECAST_STEP_S = 0.5;
const LIVE: MimoMixedRunState[] = ['armed', 'running', 'review', 'stopping'];
/** The service's subscription bounds. */
const RATE_DEFAULT = 2;
const RATE_MIN = 0.2;
const RATE_MAX = 10;
const MAX_PLOTS = 16;

function refuse(code: 'refused' | 'bad_request', reason: string, message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason as RefusalReason, detail);
}

const badParams = (param: string, message: string): never => refuse('bad_request', 'bad_params', message, { param });
const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;
/** `x` to `digits` significant digits, as the service rounds its volts and amplitudes. */
const sig = (x: number, digits: number) => (x === 0 || !Number.isFinite(x) ? x : Number(x.toPrecision(digits)));
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const amp = (db: number) => 10 ** (db / 20);
const ser = (name: string, role: Series['role'], x: ArrayLike<number>, y: ArrayLike<number>): Series => ({ name, role, x: Float64Array.from(x), y: Float64Array.from(y) });
const listOf = <T>(v: T | T[] | undefined, n: number, fallback: T): T[] => (Array.isArray(v) ? v : Array.from({ length: n }, () => (v === undefined ? fallback : v)));

// ---------------------------------------------------------------------------------------------
// The floor's law catalog, world `mimo`

function floorEntry(id: string, name: string, summary: string, params: LawEntry['params']): LawEntry {
	return {
		id,
		world: 'mimo',
		builtin: true,
		name,
		version: '1.0.0',
		author: 'Hertzforge',
		summary,
		sha256: null,
		check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: 'Built in: cleared for hardware.' },
		params,
		diagnostics: [{ key: 'step', label: 'Step', unit: '', help: 'The share of the correction taken on the last loop.', layout: 'scalar' }],
		shapes: ['square', 'more_controls_than_drives', 'fewer_controls_than_drives'],
		needs: null
	};
}

const FLOOR_LAWS: LawEntry[] = [
	floorEntry('pinv_closed', 'pseudo-inverse, closed loop', "Corrects the drive matrix each loop through the model's regularized inverse, by the measured error at the controls.", [
		{ key: 'step', label: 'Step', help: 'The share of the correction taken each loop, 0 to 1.', kind: 'number', unit: '', min: 0.05, max: 1, step: 0.05, default: 0.5 }
	]),
	floorEntry('pinv_open', 'pseudo-inverse, open loop', "Plays the model's inverse of the reference and makes no correction. For a check of the model.", [])
];

// ---------------------------------------------------------------------------------------------
// The run

interface PartRun {
	id: string;
	kind: MimoMixedPartKind;
	name: string;
	enabled: boolean;
	ramp: number;
	schedule: MimoMixedSchedule | null;
	window: { on_s: number; off_s: number | null };
	/** Per control: g peak (a tone), or the multiplier on the floor's PSD (a band). */
	target: number[];
	phase: number[];
	/** The alarm band, dB under and over. */
	alarm: [number, number];
	freq: number | null;
	direction: 'up' | 'down';
	sweeps: number;
	err: number[];
	scale: number;
	limiting: boolean;
	/** A tone's measured amplitude and phase per control, a band's level error per control. */
	ampHist: number[][];
	phHist: number[][];
}

interface Run {
	id: string;
	raw: Record<string, unknown>;
	config: MimoMixedConfig;
	derived: MimoMixedDerived;
	state: MimoMixedRunState;
	phase: MimoMixedPhase;
	mode: 'auto' | 'manual';
	source: DataSource;
	station: StationRef;
	/** The sine structure the station named at configure (null: sized to the table); the arm refuses another. */
	named: string | null;
	unit: string;
	n: number;
	m: number;
	/** The floor's PSD per control on the display lines at full level, and the lines. */
	freqs: number[];
	psd: number[][];
	levels: number[];
	rung: number;
	levelDb: number | null;
	/** Station seconds since start; null before. */
	t: number | null;
	phaseT: number;
	partsT: number | null;
	held: boolean;
	floorErr: number[];
	floorEqualized: boolean;
	loops: number;
	parts: PartRun[];
	softening: number;
	identification: MimoMixedIdentificationView | null;
	prediction: MimoMixedPrediction | null;
	overrides: MimoMixedOverride[];
	end: MimoMixedEnd | null;
	/** The station time the ramp down ends, and how the run ends then. */
	stopping: { at: number; outcome: MimoMixedOutcome; name: string; from: number } | null;
	timer: ReturnType<typeof setInterval> | null;
	record: RunRecordRef | null;
	histT: number[];
	logged: Set<string>;
}

/** The PSD of a breakpoint table at `f`: log-log between anchors, slopes from the anchor before. */
function tablePsd(rows: { freq_hz: number; psd_g2_hz?: number; slope_db_oct?: number }[], f: number): number {
	const pts: [number, number][] = [];
	for (const r of rows) {
		if (r.psd_g2_hz !== undefined) pts.push([r.freq_hz, r.psd_g2_hz]);
		else if (pts.length && r.slope_db_oct !== undefined) {
			const [f0, p0] = pts[pts.length - 1];
			pts.push([r.freq_hz, p0 * 10 ** ((r.slope_db_oct * Math.log2(r.freq_hz / f0)) / 10)]);
		}
	}
	return logLog(pts, f);
}

function logLog(pts: [number, number][], f: number): number {
	if (!pts.length || f < pts[0][0] - 1e-9 || f > pts[pts.length - 1][0] + 1e-9) return 0;
	for (let i = 1; i < pts.length; i++) {
		const [fa, pa] = pts[i - 1];
		const [fb, pb] = pts[i];
		if (f <= fb) return pa > 0 && pb > 0 && fb > fa ? pa * (pb / pa) ** (Math.log(f / fa) / Math.log(fb / fa)) : pa;
	}
	return pts[pts.length - 1][1];
}

/** The reference put on the grid: its band in lines, each control's diagonal on every line, the rms. */
function onGrid(c: MimoMixedConfig, fs: number, npts: number) {
	const r = c.reference;
	const n = c.controls.length;
	const df = fs / npts;
	if (r.kind !== 'inline' || r.freqs_hz.length < 2) return null;
	const per = (r.n * (r.n + 1)) / 2;
	const diag = (i: number): [number, number][] => r.freqs_hz.map((f, l) => [f, i < r.n ? r.re[l * per + packedIndex(r.n, i, i)] : 0]);
	const lo = Math.ceil(r.freqs_hz[0] / df - 1e-9);
	const hi = Math.floor(r.freqs_hz[r.freqs_hz.length - 1] / df + 1e-9);
	const lines = Math.max(0, hi - lo + 1);
	const tables = Array.from({ length: n }, (_, i) => diag(i));
	const rms = tables.map((t) => {
		let s = 0;
		for (let k = lo; k <= hi; k++) s += logLog(t, k * df);
		return Math.sqrt(s * df);
	});
	const stride = Math.max(1, Math.ceil(lines / GRID_POINTS));
	const freqs: number[] = [];
	for (let k = lo; k <= hi; k += stride) freqs.push(round(k * df, 4));
	const psd = tables.map((t) => freqs.map((f) => logLog(t, f)));
	return { band: [round(lo * df, 4), round(hi * df, 4)] as [number, number], lines, points: r.freqs_hz.length, rms, freqs, psd };
}

function scheduleSpan(s: MimoMixedSchedule | null): [number, number] | null {
	if (!s) return null;
	if (s.kind === 'fixed') return [s.hz, s.hz];
	if (s.kind === 'sweep') return [Math.min(s.lo_hz, s.hi_hz), Math.max(s.lo_hz, s.hi_hz)];
	return null;
}

/** The dwell at rung `i` of the schedule: one entry for every rung, else its own, the last for those past. */
function dwellFor(dwell: number[], i: number): number {
	if (!dwell.length) return 0;
	return dwell.length === 1 ? dwell[0] : dwell[Math.min(i, dwell.length - 1)];
}

export function createMimoMixedMock(host: MimoMixedMockHost): MimoMixedMock {
	let run: Run | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastKey = '';
	const subs = new Map<MockConnection, { rate: number; plots: { id: string; quantity: MimoMixedQuantity; channels: number[] | null; parts: string[] | null }[] }>();
	let liveTimer: ReturnType<typeof setInterval> | null = null;
	const isLive = () => run !== null && LIVE.includes(run.state);

	function addLog(kind: string, text: string) {
		const e = { seq: log.length, at_unix: Date.now() / 1000, kind, text } as LogEntry;
		log.push(e);
		if (run) host.broadcast('mimo_mixed.log', { run_id: run.id, entries: [e] });
	}

	const toneLaws = (): { id: string; name: string; version: string; params: LawEntry['params'] }[] =>
		host.toneLaws?.() ?? [{ id: 'matrix', name: 'matrix', version: '0.1.0', params: [] }];

	// ---- checking a configuration ---------------------------------------------------------------

	const item = (reason: string, field: string, message: string): MimoMixedReportItem => ({ reason, field, message }) as MimoMixedReportItem;

	/** What the controller's serde form needs, by its member: the first one missing, or null. */
	function unreadable(c: Record<string, unknown>): string | null {
		const need = (o: unknown, keys: string[], where: string): string | null => {
			if (!isObject(o)) return `invalid type: ${where} is not an object`;
			for (const k of keys) if (!(k in o)) return `missing field \`${k}\``;
			return null;
		};
		const top = need(c, ['controls', 'drives', 'parts', 'estimation', 'schedule'], 'the configuration');
		if (top) return top;
		if (!Array.isArray(c.controls) || !Array.isArray(c.drives) || !Array.isArray(c.parts)) return 'invalid type: the controls, the drives and the parts are lists';
		for (const d of c.drives) {
			const e = need(d, ['channel_index', 'ceiling_vrms', 'peak_v', 'pretest_max_vrms'], 'a drive');
			if (e) return e;
		}
		for (const l of Array.isArray(c.limits) ? c.limits : []) {
			const e = need(l, ['channel_index', 'ceiling_psd', 'abort_margin_db'], 'a limit');
			if (e) return e;
		}
		for (const p of c.parts) {
			const e = need(p, ['id', 'body'], 'a part') ?? need((p as Record<string, unknown>).body, ['kind'], "a part's body");
			if (e) return e;
			const b = (p as { body: Record<string, unknown> }).body;
			if (!['floor', 'tone', 'narrowband'].includes(String(b.kind))) return `unknown variant \`${String(b.kind)}\`, expected one of \`floor\`, \`tone\`, \`narrowband\``;
			for (const k of ['alarm_db', 'abort_db']) if (k in b && b[k] === null) return `invalid type: null, expected a band for \`${k}\``;
			if (b.kind !== 'floor') {
				const s = need(b.schedule, ['kind'], 'a schedule');
				if (s) return s;
				const sk = (b.schedule as Record<string, unknown>).kind;
				if (sk === 'sweep') {
					const e2 = need(b.schedule, ['lo_hz', 'hi_hz', 'start_hz', 'law', 'rate', 'target'], 'a sweep');
					if (e2) return e2;
				}
			}
		}
		const est = need(c.estimation, ['frames_per_loop', 'memory_loops'], 'the estimation');
		if (est) return est;
		const sch = need(c.schedule, ['mode', 'start_db', 'table_db', 'dwell_s'], 'the schedule');
		if (sch) return sch;
		const opt: [string, string[]][] = [
			['identification', ['start_vrms', 'below_reference_db', 'tolerance_db', 'max_steps', 'frames', 'noise_frames']],
			['gates', ['coherence_threshold', 'condition_threshold']],
			['caps', ['up_db', 'down_db']],
			['protections', ['envelope_abort_lines', 'divergence_loops', 'divergence_floor_db', 'overload_fraction', 'settle_loops']]
		];
		for (const [k, keys] of opt)
			if (k in c) {
				const e = need(c[k], keys, k);
				if (e) return e;
			}
		return null;
	}

	/** The controller's own validation: its first refusal, by name, at `field: "config"`. */
	function validate(c: MimoMixedConfig, band: [number, number] | null): MimoMixedReportItem | null {
		const at = (reason: string, words: string) => item(reason, 'config', words);
		const n = c.controls.length;
		const parts = c.parts;
		const floors = parts.filter((p) => p.body.kind === 'floor');
		if (!floors.length) return at('no_floor', 'the parts have no floor.');
		if (floors.length > 1) return at('more_than_one_floor', 'the parts have more than one floor.');
		const tones = parts.filter((p): p is MimoMixedPart<MimoMixedToneBody> => p.body.kind === 'tone');
		const bands = parts.filter((p): p is MimoMixedPart<MimoMixedBandBody> => p.body.kind === 'narrowband');
		if (tones.length > 12) return at('too_many_tones', `${tones.length} tones: at most 12.`);
		if (bands.length > 12) return at('too_many_bands', `${bands.length} narrowbands: at most 12.`);
		const ids = new Set<string>();
		for (const p of parts) {
			if (ids.has(p.id)) return at('duplicate_part_id', `two parts have the id "${p.id}".`);
			ids.add(p.id);
		}
		if (c.frf_mode !== undefined && c.frf_mode !== 'Hold') return at('frf_update_not_built', 'an updated model during the run is not built: the model is held from the identification.');
		const law = c.law ?? {};
		if (law.bands && !['auto', 'scalar_servo', 'per_control'].includes(law.bands.id)) return at('unknown_band_law', `no band law "${law.bands.id}".`);
		if (law.ceiling_rule && !['common_scale', 'in_turn'].includes(law.ceiling_rule.id)) return at('unknown_ceiling_rule', `no ceiling rule "${law.ceiling_rule.id}".`);
		if (law.fill_rule && !['no_error', 'model_fill'].includes(law.fill_rule.id)) return at('unknown_fill_rule', `no fill rule "${law.fill_rule.id}".`);
		for (const d of c.drives) {
			if (d.abort_vpk !== undefined && !(d.abort_vpk > d.peak_v)) return at('abort_below_ceiling', `drive ${d.channel_index}'s peak abort ${d.abort_vpk} V is not above its peak ceiling ${d.peak_v} V.`);
			if (d.abort_vrms !== undefined && !(d.abort_vrms > d.ceiling_vrms)) return at('abort_below_ceiling', `drive ${d.channel_index}'s rms abort ${d.abort_vrms} V is not above its rms ceiling ${d.ceiling_vrms} V.`);
		}
		for (const [k, v] of [
			['level_ramp_s', c.level_ramp_s],
			['part_ramp_s', c.part_ramp_s]
		] as const)
			if (v !== undefined && v < MIMO_MIXED_DEFAULTS.min_ramp_s) return at('ramp_too_short', `${k.replace(/_/g, ' ')} ${v} s is under two control ticks (${MIMO_MIXED_DEFAULTS.min_ramp_s} s).`);
		const counted = (v: unknown, what: string, id: string) => (Array.isArray(v) && v.length !== n ? at('wrong_control_count', `part "${id}": ${what} has ${v.length} values for ${n} controls.`) : null);
		for (const p of [...tones, ...bands]) {
			const s = p.body.schedule;
			if (s.kind === 'follow') {
				if (s.of === p.id) return at('follow_self', `part "${p.id}" follows itself.`);
				if (!ids.has(s.of)) return at('follow_unknown_part', `part "${p.id}" follows "${s.of}", which is not a part.`);
			}
			if (s.kind === 'segments') return at('segments_not_built', `part "${p.id}": a segment schedule is not built.`);
			const span = scheduleSpan(s);
			if (band && span && (span[0] < band[0] || span[1] > band[1])) return at('part_out_of_band', `part "${p.id}" plays from ${round(span[0], 1)} to ${round(span[1], 1)} Hz, outside the floor's band, ${round(band[0], 1)} to ${round(band[1], 1)} Hz.`);
		}
		for (const t of tones) {
			const e = counted(t.body.amplitude, 'the amplitude', t.id) ?? (t.body.phases_stated ? counted(t.body.phase_deg, 'the phases', t.id) : null);
			if (e) return e;
		}
		for (const b of bands) {
			const s = b.body.statement;
			if (s.kind === 'matrix') return at('band_matrix_not_built', `narrowband "${b.id}": a band stated as a full matrix is not built; state it as a raise or an added level.`);
			const e = counted(s.kind === 'raise' ? s.db : s.level_g2_hz, s.kind === 'raise' ? 'the raise' : 'the added level', b.id);
			if (e) return e;
			if (s.kind === 'raise' && listOf(s.db, n, 0).some((db) => db < 0)) return at('band_not_additive', `narrowband "${b.id}" lowers the floor: a band adds to the floor, it does not take from it.`);
			if (s.kind === 'added' && ((s.coherence ?? 0) < 0 || (s.coherence ?? 0) > 1)) return at('band_not_additive', `narrowband "${b.id}": the coherence is from 0 to 1.`);
		}
		return null;
	}

	function lawRole(entry: { id: string; name: string; version: string }, world: 'mimo' | 'sine_mimo'): MimoMixedLawRole {
		return { id: entry.id, name: entry.name, version: entry.version, sha256: null, builtin: true, world, hardware: true, not_cleared: null };
	}

	function lawParams(errors: MimoMixedReportItem[], field: string, entry: { name: string; params: LawEntry['params'] }, given: Record<string, unknown> | undefined) {
		for (const [k, v] of Object.entries(given ?? {})) {
			const spec = entry.params.find((p) => p.key === k);
			if (!spec) errors.push(item('param_unknown', `${field}.params.${k}`, `The law ${entry.name} has no parameter "${k}".`));
			else if (spec.kind === 'number' && typeof v === 'number' && ((spec.min !== undefined && spec.min !== null && v < spec.min) || (spec.max !== undefined && spec.max !== null && v > spec.max)))
				errors.push(item('param_out_of_range', `${field}.params.${k}`, `${spec.label} ${v} is outside ${spec.min} to ${spec.max}.`));
		}
	}

	function check(raw: Record<string, unknown>): { errors: MimoMixedReportItem[]; warnings: MimoMixedReportItem[]; derived: MimoMixedDerived | null; config: MimoMixedConfig | null; grid: ReturnType<typeof onGrid> } {
		const errors: MimoMixedReportItem[] = [];
		const warnings: MimoMixedReportItem[] = [];
		const none = { errors, warnings, derived: null, config: null, grid: null };
		const est = isObject(raw.estimation) ? raw.estimation : {};
		const fs = isNum(est.fs_hz) ? est.fs_hz : NaN;
		const npts = isNum(est.n_pts) ? est.n_pts : 0;
		if (!(fs > 0)) errors.push(item('out_of_range', 'estimation.fs_hz', 'The sample rate is a number of Hz above 0.'));
		if (npts < 64 || !Number.isInteger(npts) || (npts & (npts - 1)) !== 0) errors.push(item('out_of_range', 'estimation.n_pts', 'The points per frame are a power of two from 64.'));
		if (errors.length) return none;
		const ref = raw.reference;
		if (!isObject(ref) || (ref.kind !== 'inline' && ref.kind !== 'file')) {
			errors.push(item('reference_invalid', 'reference', 'The reference: give it inline or as a reference file.'));
			return none;
		}
		if (ref.kind === 'file') {
			errors.push(item('reference_invalid', 'reference', 'The reference: this demonstration service reads no reference files; build the reference from breakpoints.'));
			return none;
		}
		const nControls = Array.isArray(raw.controls) ? raw.controls.length : 0;
		if (ref.n !== nControls) {
			errors.push(item('reference_invalid', 'reference', `The reference: it is for ${String(ref.n)} controls; the setup has ${nControls}. Build it again.`));
			return none;
		}
		const why = unreadable(raw);
		if (why) {
			errors.push(item('config_unreadable', 'config', `The configuration could not be read: ${why}.`));
			return none;
		}
		const c = raw as unknown as MimoMixedConfig;
		const grid = onGrid(c, fs, npts);
		if (!grid || grid.lines < 2) {
			errors.push(item('reference_invalid', 'reference', 'The reference: it has no lines on the grid.'));
			return none;
		}
		const v = validate(c, grid.band);
		if (v) errors.push(v);
		const rows = host.rows();
		const input = (i: number) => rows.some((r) => r.index === i && r.enabled && r.role !== 'drive');
		c.controls.forEach((ch, k) => {
			if (!input(ch)) errors.push(item('channel_unknown', `controls[${k}]`, `Channel ${ch} is not an enabled input of the table.`));
		});
		(c.limits ?? []).forEach((l, k) => {
			if (!input(l.channel_index)) errors.push(item('channel_unknown', `limits[${k}]`, `Channel ${l.channel_index} is not an enabled input of the table.`));
		});
		c.drives.forEach((d, k) => {
			const first = c.drives.slice(0, k).findIndex((e) => e.channel_index === d.channel_index);
			if (first >= 0) errors.push(item('duplicate_drive', `drives[${k}]`, `Channel ${d.channel_index} is already drive ${first}; each drive is its own table row.`));
			if (!rows.some((r) => r.index === d.channel_index && r.enabled && r.role === 'drive')) errors.push(item('channel_unknown', `drives[${k}]`, `Channel ${d.channel_index} is not an enabled drive of the table.`));
		});
		const law = c.law ?? {};
		const floorId = law.floor?.id ?? MIMO_MIXED_DEFAULTS.floor_law;
		const toneId = law.tones?.id ?? MIMO_MIXED_DEFAULTS.tone_law;
		const floor = FLOOR_LAWS.find((l) => l.id === floorId);
		const tone = toneLaws().find((l) => l.id === toneId);
		if (!floor) errors.push(item('law_unknown', 'law.floor.id', `The floor's law "${floorId}" is not in the catalog.`));
		if (!tone) errors.push(item('law_unknown', 'law.tones.id', `The tones' law "${toneId}" is not in the catalog.`));
		if (floor) lawParams(errors, 'law.floor', floor, law.floor?.params);
		if (tone) lawParams(errors, 'law.tones', tone, law.tones?.params);
		if (errors.length || !floor || !tone) return { ...none, grid };
		const derived: MimoMixedDerived = {
			grid: { fs_hz: fs, n_pts: npts, df_hz: fs / npts, lines: npts / 2 + 1 },
			band_hz: grid.band,
			reference_rms: grid.rms.map((x) => sig(x, 6)),
			reference_source: `inline, ${grid.points} points from ${grid.band[0]} to ${grid.band[1]} Hz, on ${grid.lines} grid lines`,
			law: { floor: lawRole(floor, 'mimo'), tones: lawRole(tone, 'sine_mimo') }
		};
		return { errors, warnings, derived, config: c, grid };
	}

	// ---- the reference --------------------------------------------------------------------------

	function buildReference(p: MimoBuildBreakpointsParams): MimoReferenceResult {
		const fs = p.grid.fs_hz;
		const npts = p.grid.n_pts;
		const df = fs / npts;
		const n = p.tables.length;
		const errors: MimoReferenceResult['errors'] = [];
		p.tables.forEach((t, i) => {
			if (t.rows.length < 2) errors.push({ reason: 'out_of_range', message: `${t.label}: give at least two rows.`, row: 0, field: `tables.${i}` } as never);
			if (t.rows.some((r) => r.psd_g2_hz !== undefined && !(r.psd_g2_hz > 0))) errors.push({ reason: 'out_of_range', message: `${t.label}: every PSD is above 0.`, field: `tables.${i}` } as never);
		});
		const lo = Math.min(...p.tables.map((t) => t.rows[0]?.freq_hz ?? Infinity));
		const hi = Math.max(...p.tables.map((t) => t.rows[t.rows.length - 1]?.freq_hz ?? 0));
		if (!(hi > lo) || hi > fs / 2.56) errors.push({ reason: 'out_of_range', message: `The band reaches ${hi} Hz; at ${fs} Hz the highest line is ${round(fs / 2.56, 1)} Hz.` } as never);
		const base = { labels: p.tables.map((t) => t.label), unit: p.unit ?? 'g', df_hz: df, warnings: [], transforms: { response: null, drive: null }, provenance: null };
		const psd0 = { violations: 0, min_eigenvalue_ratio: 0, worst_hz: null, lines_hz: [], repair: null, repair_accepted: false };
		if (errors.length) return { ok: false, reference: null, band_hz: null, lines: 0, errors, psd: psd0, views: [], ...base };
		const freqs: number[] = [];
		for (let f = Math.ceil(lo / df) * df; f <= hi + 1e-9; f += df) freqs.push(round(f, 4));
		const per = (n * (n + 1)) / 2;
		const re = new Array<number>(freqs.length * per).fill(0);
		const im = new Array<number>(freqs.length * per).fill(0);
		const coh = new Map((p.pairs ?? []).map((q) => [`${q.row}-${q.col}`, q]));
		freqs.forEach((f, l) => {
			const g = p.tables.map((t) => tablePsd(t.rows, f));
			for (let r = 0; r < n; r++)
				for (let c = r; c < n; c++) {
					const k = l * per + packedIndex(n, r, c);
					if (r === c) re[k] = g[r];
					else {
						const q = coh.get(`${r}-${c}`);
						const gamma = q ? Math.sqrt(typeof q.coherence === 'number' ? q.coherence : 0) : 0;
						const ph = ((q && typeof q.phase_deg === 'number' ? q.phase_deg : 0) * Math.PI) / 180;
						const mag = gamma * Math.sqrt(g[r] * g[c]);
						re[k] = mag * Math.cos(ph);
						im[k] = mag * Math.sin(ph);
					}
				}
		});
		const stride = Math.max(1, Math.ceil(freqs.length / GRID_POINTS));
		const xs = freqs.filter((_, i) => i % stride === 0);
		const views: View[] = [
			{
				view: 'spectrum',
				title: 'Reference PSDs',
				x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
				y: { label: 'PSD', unit: `${p.unit ?? 'g'}²/Hz`, scale: 'log' },
				series: p.tables.map((t, i) => ser(t.label, i === 0 ? 'reference' : 'other', xs, xs.map((f) => tablePsd(t.rows, f))))
			} as SpectrumView
		];
		return { ok: true, reference: { kind: 'inline', freqs_hz: freqs, n, re, im }, band_hz: [freqs[0], freqs[freqs.length - 1]], lines: freqs.length, errors: [], psd: { ...psd0, lines_hz: [] }, views, ...base } as MimoReferenceResult;
	}

	// ---- the state ------------------------------------------------------------------------------

	const levelAmp = (r: Run) => (r.levelDb === null ? 0 : amp(r.levelDb));
	const driving = (r: Run) => r.state === 'running' && ['floor_only', 'engaged', 'full_level'].includes(r.phase);
	const partsIn = (r: Run) => (r.state === 'running' || r.state === 'stopping') && (r.phase === 'engaged' || r.phase === 'full_level' || r.phase === 'ramping_down') && r.partsT !== null;
	/** The controller's status exists once the run has started. */
	const started = (r: Run) => r.t !== null;
	const crest = (r: Run) => r.config.crest_factor ?? MIMO_MIXED_DEFAULTS.crest_factor;
	const floorBand = (r: Run): [number, number] => {
		const f = r.config.parts.find((p) => p.body.kind === 'floor')?.body as { alarm_db?: unknown } | undefined;
		const b = Array.isArray(f?.alarm_db) ? (Array.isArray(f.alarm_db[0]) ? f.alarm_db[0] : f.alarm_db) : MIMO_MIXED_DEFAULTS.floor_alarm_db;
		return [Number(b[0]), Number(b[1])];
	};

	function toneScale(r: Run): number {
		return r.parts.filter((p) => p.kind === 'tone' && p.ramp > 0).reduce((s, p) => Math.min(s, p.scale), 1);
	}

	function partRow(r: Run, p: PartRun): MimoMixedPartRow {
		const st = started(r);
		const la = levelAmp(r);
		const on = p.kind === 'floor' ? driving(r) || r.state === 'stopping' : partsIn(r) && p.ramp > 0;
		const controls = r.config.controls.map((ch, i) => {
			if (p.kind === 'floor') {
				const target = r.derived.reference_rms[i] * (r.levelDb === null ? 1 : la);
				const e = on ? r.floorErr[i] : null;
				const [lo, hi] = floorBand(r);
				return { channel: ch, target: sig(target, 4), measured: e === null ? null : sig(target * amp(e), 4), error_db: e === null ? null : round(e, 2), alarm: e !== null && (e < -lo || e > hi) };
			}
			if (!st) return { channel: ch, target: null, measured: null, error_db: null, alarm: false };
			const e = on ? p.err[i] + (p.kind === 'tone' && p.limiting ? 20 * Math.log10(Math.max(p.scale, 1e-3)) : 0) : null;
			const alarm = e !== null && (e < -p.alarm[0] || e > p.alarm[1]);
			if (p.kind === 'narrowband') return { channel: ch, target: null, measured: null, error_db: e === null ? null : round(e, 2), alarm };
			const t = p.target[i] * la * p.ramp;
			return { channel: ch, target: sig(t, 4), measured: e === null ? sig(0, 4) : sig(t * amp(e), 4), error_db: e === null ? null : round(e, 2), alarm };
		});
		return {
			id: p.id,
			kind: p.kind,
			name: p.name,
			enabled: p.enabled,
			on,
			freq_hz: !st || p.freq === null ? null : round(p.freq, 3),
			held: r.held,
			limiting: on && p.limiting,
			scale: !st || p.kind === 'floor' ? null : round(p.kind === 'tone' ? p.scale : p.ramp, 4),
			folded: !st || p.kind !== 'narrowband' ? null : false,
			amplitude: p.kind === 'floor' ? 'rms' : p.kind === 'tone' ? 'peak' : 'db',
			unit: p.kind === 'narrowband' ? 'dB' : r.unit,
			controls
		};
	}

	/** The tones' drive per drive at full level, volts peak per g, and the softening on the first. */
	function toneGain(r: Run, j: number): number {
		return 1.2 * (1 + 0.15 * j) * (j === 0 ? 1 + r.softening : 1);
	}

	function driveNumbers(r: Run, withScale = true) {
		const la = levelAmp(r);
		return r.config.drives.map((d, j) => {
			const rms = driving(r) || r.state === 'stopping' ? 0.25 * (1 + 0.1 * j) * la * (1 + (0.5 * Math.max(0, ...r.floorErr)) / 6) : 0;
			const toneSum = partsIn(r) ? r.parts.filter((p) => p.kind === 'tone').reduce((s, p) => s + toneGain(r, j) * Math.max(...p.target, 0) * p.ramp * (withScale ? p.scale : 1) * la, 0) : 0;
			return { d, rms, held: rms * crest(r), toneSum };
		});
	}

	function drivesOf(r: Run): MimoMixedDriveRow[] {
		const st = started(r);
		const scale = toneScale(r);
		return driveNumbers(r).map(({ d, rms, held, toneSum }) => ({
			channel: d.channel_index,
			random_rms_v: st ? sig(rms, 4) : null,
			peak_v: st ? sig(held * 0.9 + toneSum, 4) : null,
			random_peak_v: sig(held, 4),
			tone_sum_v: st ? sig(toneSum, 4) : null,
			ceiling_v: d.peak_v,
			abort_vpk: sig(d.abort_vpk ?? defaultAbortVpk(d.peak_v), 4),
			headroom_v: sig(d.peak_v - held, 4),
			headroom_db: held > 0 ? round(20 * Math.log10(d.peak_v / held), 2) : null,
			scale: round(scale, 4)
		}));
	}

	function limitsOf(r: Run): MimoMixedState['limits'] {
		if (!started(r)) return [];
		const la = levelAmp(r);
		const f = r.parts.find((p) => p.kind === 'tone' && p.ramp > 0)?.freq ?? r.freqs[Math.floor(r.freqs.length / 2)] ?? null;
		return (r.config.limits ?? []).map((l, k) => ({
			channel: l.channel_index,
			over_db: driving(r) ? round(-l.abort_margin_db - 1.5 + 2.5 * la + 1.2 * r.softening - 0.5 * k, 2) : null,
			at_hz: driving(r) && f !== null ? round(f, 3) : null
		}));
	}

	function stateOf(r: Run | null): MimoMixedState {
		if (!r) {
			return {
				run_id: null, state: 'unconfigured', phase: 'idle', title: null, source: null, station: null, grid: null, clock: null, mode: null, levels_db: null,
				level_db: null, t_s: null, at_level_s: null, remaining_at_level_s: null, held: false, alarm: false, limiting: false, parts: [], drives: [], scale: null, floor_error_db: [], floor_equalized: false, floor: null,
				limits: [], identification: null, prediction: null, override: null, overrides: [], law: null, end: null
			};
		}
		const rows = r.parts.map((p) => partRow(r, p));
		const st = started(r);
		const actual = r.state !== 'configured';
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.config.title ?? 'MIMO Mixed Mode',
			source: r.source,
			station: r.station,
			grid: { ...r.derived.grid, band_hz: r.derived.band_hz, actual },
			clock: actual ? 'shared' : null,
			mode: r.mode,
			levels_db: r.levels,
			level_db: r.levelDb === null ? null : round(r.levelDb, 2),
			t_s: r.t === null ? null : round(r.t, 3),
			...levelProgress(r),
			held: r.held,
			alarm: rows.some((p) => p.controls.some((c) => c.alarm)),
			limiting: rows.some((p) => p.limiting),
			parts: rows,
			drives: drivesOf(r),
			scale: st ? round(toneScale(r), 4) : null,
			floor_error_db: st && (driving(r) || r.state === 'stopping') ? r.floorErr.map((e) => round(e, 2)) : [],
			floor_equalized: r.floorEqualized,
			floor: st ? { equalized: r.floorEqualized, loops: r.loops, misses: 0, underruns: 0, dropped: 0, clamped: 0 } : null,
			limits: limitsOf(r),
			identification: r.identification,
			prediction: r.prediction,
			override: r.overrides[r.overrides.length - 1] ?? null,
			overrides: [...r.overrides],
			law: { floor: r.derived.law.floor, tones: r.derived.law.tones },
			end: r.end,
			...(r.record ? { record: { ...r.record } } : {})
		};
	}

	function stateChanged(force = false) {
		const s = stateOf(run);
		const key = JSON.stringify([s.state, s.phase, s.mode, s.level_db, s.held, s.limiting, s.alarm, s.floor_equalized, s.parts.map((p) => [p.enabled, p.on, p.limiting]), s.end, s.overrides.length, s.identification?.passed ?? null]);
		if (!force && key === lastKey) return;
		lastKey = key;
		host.broadcast('mimo_mixed.state', s as unknown as Record<string, unknown>);
	}

	function setPhase(r: Run, state: MimoMixedRunState, phase: MimoMixedPhase) {
		r.state = state;
		r.phase = phase;
		r.phaseT = 0;
	}

	// ---- the story ------------------------------------------------------------------------------

	function identify(r: Run) {
		const gate = r.config.gates?.coherence_threshold ?? 0.5;
		const coherence = Array.from({ length: r.n }, (_, i) => round(0.93 - 0.04 * i, 4));
		const id = r.config.identification;
		const start = id?.start_vrms ?? 0.01;
		r.identification = {
			level_db: -(id?.below_reference_db ?? 20),
			steps: 2,
			frames: id?.frames ?? Math.max(10 * r.m, 64),
			lag_frames: 265,
			band_coherence: coherence,
			gate_coherence: gate,
			passed: coherence.every((c) => c >= gate),
			drive_vrms: r.config.drives.map((d) => sig(Math.min(start * 1.278, d.pretest_max_vrms), 4))
		};
		addLog('identification', `identified at ${r.identification.level_db} dB: band coherence ${Math.min(...coherence).toFixed(2)} at worst against the gate ${gate}`);
		if (!r.identification.passed) {
			setPhase(r, 'review', 'gate_failed');
			addLog('gate', 'the coherence gate failed; the run holds for the operator');
			return;
		}
		review(r);
	}

	/** The prediction once identified, and the hold it puts the run in. */
	function review(r: Run) {
		r.prediction = predict(r);
		addLog('identification', `prediction ${r.prediction.within ? 'within' : 'over'} every drive ceiling${r.prediction.forecast.refused ? `; the forecast refuses (${r.prediction.forecast.refused})` : ''}`);
		setPhase(r, 'review', r.prediction.within || r.prediction.forecast.refused ? 'awaiting_review' : 'prediction_exceeded');
	}

	function predict(r: Run): MimoMixedPrediction {
		const k = crest(r);
		const tones = r.parts.filter((p) => p.kind === 'tone' && p.enabled);
		const drives = r.config.drives.map((d, j) => {
			const rms = 0.25 * (1 + 0.1 * j);
			const sum = tones.reduce((s, p) => s + 1.2 * (1 + 0.15 * j) * Math.max(...p.target, 0), 0);
			const ms = tones.reduce((s, p) => s + (1.2 * (1 + 0.15 * j) * Math.max(...p.target, 0)) ** 2 / 2, 0);
			const peak = rms * k + sum;
			return { drive: d.channel_index, random_rms_v: sig(rms, 4), random_peak_v: sig(rms * k, 4), random_peak_factor: round(k, 3), tone_peak_sum_v: sig(sum, 4), tone_rms_v: sig(Math.sqrt(ms), 4), total_rms_v: sig(Math.sqrt(rms * rms + ms), 4), peak_v: sig(peak, 4), ceiling_vrms: d.ceiling_vrms, ceiling_peak_v: d.peak_v, within: rms <= d.ceiling_vrms && peak <= d.peak_v };
		});
		const controls = r.config.controls.map((ch, i) => {
			const rr = r.derived.reference_rms[i];
			const tr = Math.sqrt(tones.reduce((s, p) => s + p.target[i] ** 2 / 2, 0));
			return { control: ch, random_rms: sig(rr, 4), tone_rms: sig(tr, 4), total_rms: sig(Math.sqrt(rr * rr + tr * tr), 4) };
		});
		const points = drives.map((d) => ({ drive: d.drive, time_s: round(r.config.full_level_duration_s ?? 60, 3), need_v: d.peak_v, ceiling_v: d.ceiling_peak_v }));
		const worst = points.reduce<(typeof points)[number] | null>((w, x) => (!w || x.need_v / x.ceiling_v > w.need_v / w.ceiling_v ? x : w), null);
		// the floor alone over a drive's peak: no schedule fits it
		const misfit = drives.find((d) => (d.random_peak_v ?? 0) > (d.ceiling_peak_v ?? Infinity));
		const cannot = misfit ? { drive: misfit.drive, time_s: 0, need_v: misfit.random_peak_v, ceiling_v: misfit.ceiling_peak_v } : null;
		return { drives, controls, forecast: { step_s: FORECAST_STEP_S, worst: cannot ?? worst, cannot_fit: cannot, refused: cannot ? 'budget_cannot_fit' : null }, within: drives.every((d) => d.within) };
	}

	/** Engage from the review: refused when the forecast cannot fit, as the engine refuses it. */
	function engage(r: Run) {
		const f = r.prediction?.forecast;
		if (f?.refused === 'budget_cannot_fit' && f.cannot_fit) {
			const c = f.cannot_fit;
			refuse('refused', 'budget_cannot_fit', `The run cannot engage: drive ${c.drive} needs ${c.need_v} V against its ${c.ceiling_v} V ceiling at ${c.time_s} s. No override passes the drive budget.`, { drive: c.drive, time_s: c.time_s, need_v: c.need_v, ceiling_v: c.ceiling_v });
		}
		setPhase(r, 'running', 'floor_only');
		r.rung = 0;
		r.levelDb = r.config.schedule.start_db - (r.config.soft_start_db ?? MIMO_MIXED_DEFAULTS.soft_start_db);
		r.floorErr = Array.from({ length: r.n }, (_, i) => 5 - 0.6 * i);
		addLog('engage', 'engaged: the floor plays alone at the soft-start level');
	}

	function freqOf(r: Run, p: PartRun, dt: number): number | null {
		const s = p.schedule;
		if (!s) return null;
		if (s.kind === 'fixed') return s.hz;
		if (s.kind === 'follow') {
			const lead = r.parts.find((x) => x.id === s.of);
			return lead?.freq === null || lead?.freq === undefined ? null : lead.freq * s.ratio;
		}
		if (s.kind !== 'sweep') return p.freq;
		const [lo, hi] = [Math.min(s.lo_hz, s.hi_hz), Math.max(s.lo_hz, s.hi_hz)];
		const f = p.freq ?? s.start_hz ?? lo;
		if (r.held || dt <= 0) return f;
		const up = p.direction === 'up';
		let next = s.law === 'log_oct_per_min' ? f * 2 ** (((up ? 1 : -1) * s.rate * dt) / 60) : f + (up ? 1 : -1) * s.rate * dt;
		if (next >= hi) {
			next = hi;
			p.direction = 'down';
			p.sweeps += 0.5;
		} else if (next <= lo) {
			next = lo;
			p.direction = 'up';
			p.sweeps += 0.5;
		}
		return next;
	}

	/** Whether a sweep has met its target, in sweeps or seconds of its window. */
	function sweepDone(p: PartRun, partsT: number): boolean {
		const s = p.schedule;
		if (!s || s.kind !== 'sweep') return false;
		const t = s.target;
		const since = partsT - p.window.on_s;
		if (t.kind === 'sweeps') return p.sweeps >= t.n;
		if (t.kind === 'duration') return since >= t.s;
		return p.sweeps >= t.sweeps || since >= t.duration_s;
	}

	const noise = (k: number) => (Math.sin(k * 12.9898 + Date.now() / 700) * 43758.5453) % 1;

	function beginEnd(r: Run, outcome: MimoMixedOutcome, name: string) {
		const ramp = r.config.shutdown_ramp_s ?? 0.5;
		setPhase(r, 'stopping', 'ramping_down');
		r.stopping = { at: (r.t ?? 0) + ramp, outcome, name, from: r.t ?? 0 };
	}

	function step(r: Run, dtWall: number) {
		const dt = dtWall * CLOCK_RATE;
		r.t = (r.t ?? 0) + dt;
		r.phaseT += dt;
		if (r.stopping && r.t >= r.stopping.at) {
			const s = r.stopping;
			const message = s.name === 'completed' ? 'Test complete: the schedule finished and every part ramped down with the controlled stop.' : s.outcome === 'safe_stop' ? 'Nobody held control, so the service stopped the run with its controlled stop.' : `Stopped at ${round(r.levelDb ?? 0, 2)} dB after ${round(r.t, 2)} s.`;
			return end(r, s.outcome, 'stopped', { name: s.name, message, ramp_s: round(r.t - s.from, 3) });
		}
		if (r.phase === 'sysid' && r.phaseT >= SYSID_S) {
			setPhase(r, 'running', 'coherence_gate');
			return stateChanged(true);
		}
		if (r.phase === 'coherence_gate' && r.phaseT >= GATE_S) {
			identify(r);
			return stateChanged(true);
		}
		if (!driving(r) && r.state !== 'stopping') return;
		r.loops += 1;
		r.floorErr = r.floorErr.map((e, i) => e * Math.exp(-dt / 1.0) + 0.25 * noise(i + 1));
		if (r.phase === 'floor_only') {
			if (r.floorErr.every((e) => Math.abs(e) < 1) && r.phaseT >= FLOOR_S) {
				r.floorEqualized = true;
				r.phase = 'engaged';
				r.phaseT = 0;
				r.partsT = 0;
				r.levelDb = r.config.schedule.start_db;
				addLog('state', `the floor equalized in ${round(r.t, 1)} s: the parts come in`);
			} else if (r.phaseT > (r.config.floor_settle_s ?? MIMO_MIXED_DEFAULTS.floor_settle_s)) {
				return end(r, 'aborted', 'aborted', { name: 'floor_not_equalized', part: 'floor', message: 'Aborted (floor_not_equalized): the floor did not equalize in time.' });
			}
		} else {
			if (!r.held && r.partsT !== null) r.partsT += dt;
			const ramp = dt / Math.max(MIMO_MIXED_DEFAULTS.min_ramp_s, r.config.part_ramp_s ?? MIMO_MIXED_DEFAULTS.part_ramp_s);
			for (const p of r.parts) {
				if (p.kind === 'floor') continue;
				const pt = r.partsT ?? 0;
				const inWindow = pt >= p.window.on_s && (p.window.off_s === null || pt < p.window.off_s) && !sweepDone(p, pt);
				const want = p.enabled && inWindow && r.state !== 'stopping' ? 1 : 0;
				const was = p.ramp;
				p.ramp = want > p.ramp ? Math.min(1, p.ramp + ramp) : Math.max(0, p.ramp - ramp);
				if (was === 0 && p.ramp > 0 && !r.logged.has(`in-${p.id}`)) {
					r.logged.add(`in-${p.id}`);
					addLog('part', `part ${p.id} in${p.freq !== null ? ` at ${round(p.freq, 1)} Hz` : ''}`);
				}
				p.freq = freqOf(r, p, r.held ? 0 : dt);
				p.err = p.err.map((e, i) => e * Math.exp(-dt / 0.5) + 0.15 * noise(i + 7 + p.id.length));
			}
			applyCeiling(r);
			if (r.state !== 'stopping') schedule(r);
			if (r.phase === 'full_level') r.softening = Math.min(softCap(r), r.softening + (dt * softCap(r)) / 20);
		}
		keep(r);
		stateChanged();
	}

	/** The time at the rung or at full level, and what is left of it, as the schedule below counts them. */
	function levelProgress(r: Run): { at_level_s: number | null; remaining_at_level_s: number | null } {
		const lr = r.config.level_ramp_s ?? MIMO_MIXED_DEFAULTS.level_ramp_s;
		const at = round(Math.max(0, r.phaseT), 3);
		const left = (total: number) => round(Math.max(0, total - r.phaseT), 3);
		if (r.phase === 'engaged') return { at_level_s: at, remaining_at_level_s: r.mode === 'auto' ? left(dwellFor(r.config.schedule.dwell_s, r.rung) + lr) : null };
		if (r.phase === 'full_level') {
			const d = r.config.full_level_duration_s;
			return { at_level_s: at, remaining_at_level_s: d === undefined || d === null ? null : left(d + lr) };
		}
		return { at_level_s: null, remaining_at_level_s: null };
	}

	/** The level schedule: the rungs between the start and full level, each its dwell, then full level for its time. */
	function schedule(r: Run) {
		const s = r.config.schedule;
		const lr = r.config.level_ramp_s ?? MIMO_MIXED_DEFAULTS.level_ramp_s;
		if (r.held) {
			r.phaseT = Math.max(0, r.phaseT - 1e-9);
			return;
		}
		if (r.phase === 'engaged') {
			if ((r.levelDb ?? -Infinity) >= -1e-6) {
				r.phase = 'full_level';
				r.phaseT = 0;
				addLog('level', 'full level');
				return;
			}
			if (r.mode !== 'auto') return;
			const rungs = s.table_db.filter((d) => d > s.start_db && d < 0);
			if (r.phaseT >= dwellFor(s.dwell_s, r.rung) + lr) {
				if (r.rung < rungs.length) {
					r.levelDb = rungs[r.rung];
					r.rung += 1;
					addLog('level', `level ${r.levelDb} dB`);
				} else {
					r.levelDb = 0;
					r.phase = 'full_level';
					addLog('level', 'full level');
				}
				r.phaseT = 0;
			}
		} else if (r.phase === 'full_level') {
			if ((r.levelDb ?? 0) < -1e-6) {
				r.phase = 'engaged';
				r.phaseT = 0;
				return;
			}
			const d = r.config.full_level_duration_s;
			if (d !== undefined && r.phaseT >= d + lr) {
				addLog('state', 'the full-level time is done: ramping down');
				beginEnd(r, 'completed', 'completed');
			}
		}
	}

	/** How far the first drive's tone gain grows at full level: until the tones ask about 1.16 times its room. */
	function softCap(r: Run): number {
		const d = r.config.drives[0];
		if (!d) return 0;
		const room = d.peak_v - 0.25 * crest(r);
		const ask = r.parts.filter((p) => p.kind === 'tone').reduce((s, p) => s + 1.2 * Math.max(...p.target, 0), 0);
		return ask > 0 && room > 0 ? Math.max(0.2, room / (0.86 * ask) - 1) : 0;
	}

	/** The ceiling rule: each drive's room left to the tones, and the scale it puts on them; a band held while the drive binds. */
	function applyCeiling(r: Run) {
		const la = levelAmp(r);
		const tones = r.parts.filter((p) => p.kind === 'tone');
		const rule = r.config.law?.ceiling_rule?.id ?? 'common_scale';
		let s = 1;
		let worst = -1;
		driveNumbers(r, false).forEach(({ d, held }, j) => {
			const room = la > 0 ? (d.peak_v - held) / la : Infinity;
			const ask = tones.reduce((sum, p) => sum + toneGain(r, j) * Math.max(...p.target, 0) * p.ramp, 0);
			if (ask > 0 && room / ask < s) {
				s = Math.max(0, room / ask);
				worst = j;
			}
		});
		const binds = s < 0.9995;
		if (rule === 'common_scale') {
			for (const p of tones) {
				p.scale = binds ? s : 1;
				p.limiting = binds && p.ramp > 0;
			}
		} else {
			tones.forEach((p, k) => {
				const last = k === tones.length - 1;
				p.scale = binds && last ? Math.max(0, 1 - (1 - s) * tones.length) : 1;
				p.limiting = binds && last && p.ramp > 0;
			});
		}
		for (const b of r.parts.filter((p) => p.kind === 'narrowband')) b.limiting = binds && b.ramp > 0;
		if (binds && !r.logged.has('limit')) {
			r.logged.add('limit');
			const ch = r.config.drives[worst]?.channel_index;
			addLog('alarm', rule === 'common_scale' ? `drive ${ch} is out of room: every tone at ${Math.round(s * 100)} % of its target` : `drive ${ch} is out of room: the last tone is scaled down`);
		}
	}

	function keep(r: Run) {
		r.histT.push(round(r.t ?? 0, 3));
		if (r.histT.length > HISTORY) r.histT.shift();
		const rows = r.parts.map((p) => partRow(r, p));
		r.parts.forEach((p, k) => {
			if (p.kind === 'floor') return;
			for (let i = 0; i < r.n; i++) {
				const c = rows[k].controls[i];
				const push = (h: number[][], v: number) => {
					(h[i] ??= []).push(v);
					if (h[i].length > HISTORY) h[i].shift();
				};
				push(p.ampHist, p.kind === 'tone' ? (c.measured ?? 0) : (c.error_db ?? 0));
				push(p.phHist, round(p.phase[i] + 2 * noise(i + k), 2));
			}
		});
	}

	function end(r: Run, outcome: MimoMixedOutcome, phase: 'stopped' | 'aborted', over: Partial<MimoMixedEnd> = {}) {
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		r.stopping = null;
		r.end = { outcome, name: null, message: 'The run ended.', at_s: r.t === null ? null : round(r.t, 3), level_db: r.levelDb === null ? null : round(r.levelDb, 2), part: null, control: null, drive: null, ramp_s: null, ...over };
		r.state = 'finished';
		r.phase = phase;
		r.held = false;
		r.parts.forEach((p) => {
			p.ramp = 0;
			p.limiting = false;
		});
		addLog('state', `${outcome === 'completed' ? 'completed' : 'ended'}: ${r.end.message}`);
		stateChanged(true);
		stopLive();
	}

	// ---- display --------------------------------------------------------------------------------

	const fAxis = { label: 'Frequency', unit: 'Hz', scale: 'log' as const };
	const tAxis = { label: 'Time', unit: 's', scale: 'linear' as const };
	const psdUnit = (r: Run) => `${r.unit}^2/Hz`;

	function spectrum(title: string, y: SpectrumView['y'], series: Series[], link: 'frequency' | 'time' = 'frequency', x: SpectrumView['x'] = fAxis): SpectrumView {
		return { view: 'spectrum', title, x, y, link, series } as SpectrumView;
	}

	function measuredPsd(r: Run, i: number, tones: boolean): number[] {
		const la2 = levelAmp(r) ** 2;
		const parts = r.parts.filter((p) => p.kind !== 'floor' && p.ramp > 0 && p.freq !== null);
		const df = r.freqs.length > 1 ? r.freqs[1] - r.freqs[0] : 1;
		return r.freqs.map((f, k) => {
			let y = (r.psd[i][k] ?? 0) * la2 * amp(r.floorErr[i] ?? 0) ** 2 * (1 + 0.15 * noise(k + i));
			for (const p of parts) {
				const near = Math.abs(f - (p.freq as number)) <= df / 2;
				if (p.kind === 'tone' && tones && near) y += (p.target[i] * Math.sqrt(la2) * p.ramp * p.scale) ** 2 / 2 / df;
				if (p.kind === 'narrowband' && Math.abs(f - (p.freq as number)) <= widthOf(r, p) / 2) y += (r.psd[i][k] ?? 0) * la2 * (p.target[i] - 1) * p.ramp;
			}
			return y;
		});
	}

	function widthOf(r: Run, p: PartRun): number {
		const b = r.config.parts.find((x) => x.id === p.id)?.body as MimoMixedBandBody | undefined;
		if (!b || b.kind !== 'narrowband') return 0;
		return b.width.kind === 'hz' ? b.width.hz : b.width.share * (p.freq ?? 0);
	}

	const label = (r: Run, i: number) => `control ${r.config.controls[i]}`;
	const hasSpectra = (r: Run) => r.floorEqualized || driving(r) || r.state === 'stopping' || (r.state === 'finished' && r.loops > 0);

	function controlView(r: Run, pick: number[]): SpectrumView {
		const la2 = levelAmp(r) ** 2;
		const [lo, hi] = floorBand(r);
		const series: Series[] = [];
		for (const i of pick) {
			const ref = r.psd[i].map((y) => y * la2);
			series.push(ser(label(r, i), 'measured', r.freqs, measuredPsd(r, i, true)), ser(`${label(r, i)} reference`, 'reference', r.freqs, ref));
			series.push(ser(`${label(r, i)} alarm high`, 'tolerance', r.freqs, ref.map((y) => y * 10 ** (hi / 10))), ser(`${label(r, i)} alarm low`, 'tolerance', r.freqs, ref.map((y) => y * 10 ** (-lo / 10))));
			series.push(ser(`${label(r, i)} abort high`, 'abort', r.freqs, ref.map((y) => y * 10 ** (2 * hi / 10))), ser(`${label(r, i)} abort low`, 'abort', r.freqs, ref.map((y) => y * 10 ** ((-2 * lo) / 10))));
		}
		return spectrum('Control spectra', { label: 'PSD', unit: psdUnit(r), scale: 'log' }, series);
	}

	function crossSeries(r: Run, pick: number[], role: Series['role']): Series[] {
		const out: Series[] = [];
		for (let a = 0; a + 1 < r.n; a++)
			if (pick.includes(a) || pick.includes(a + 1)) out.push(ser(`|cross| ${r.config.controls[a]} with ${r.config.controls[a + 1]}`, role, r.freqs, r.freqs.map((_, k) => (r.psd[a][k] ?? 0) * levelAmp(r) ** 2 * Math.max(0.01, 0.05 + 0.04 * noise(k + a)))));
		return out;
	}

	function driveView(r: Run): SpectrumView {
		return spectrum(
			'Drive spectra',
			{ label: 'PSD', unit: 'V²/Hz', scale: 'log' },
			r.config.drives.map((d, j) => ser(`drive ${d.channel_index}`, 'drive', r.freqs, measuredPsd(r, Math.min(j, r.n - 1), false).map((y) => y * 0.06 * (1 + 0.1 * j))))
		);
	}

	function plotView(r: Run, p: { quantity: MimoMixedQuantity; parts: string[] | null }, pick: number[]): View | null {
		const la2 = levelAmp(r) ** 2;
		switch (p.quantity) {
			case 'control':
				return hasSpectra(r) ? controlView(r, pick) : null;
			case 'residual':
				return hasSpectra(r)
					? spectrum('Residual spectra', { label: 'PSD', unit: psdUnit(r), scale: 'log' }, [...pick.flatMap((i) => [ser(label(r, i), 'measured', r.freqs, measuredPsd(r, i, false)), ser(`${label(r, i)} reference`, 'reference', r.freqs, r.psd[i].map((y) => y * la2))]), ...crossSeries(r, pick, 'other')])
					: null;
			case 'cross':
				return hasSpectra(r) ? spectrum('Cross spectra', { label: '|CSD|', unit: psdUnit(r), scale: 'log' }, crossSeries(r, pick, 'measured')) : null;
			case 'drive':
				return hasSpectra(r) ? driveView(r) : null;
			case 'tone_amplitude':
			case 'tone_phase':
			case 'band_level': {
				const kind = p.quantity === 'band_level' ? 'narrowband' : 'tone';
				const series: Series[] = [];
				for (const x of r.parts.filter((y) => y.kind === kind && (!p.parts || p.parts.includes(y.id))))
					for (const i of pick) {
						const hist = (p.quantity === 'tone_phase' ? x.phHist[i] : x.ampHist[i]) ?? [];
						series.push(ser(`${x.id} ${label(r, i)}`, 'measured', r.histT.slice(-hist.length), hist));
					}
				const y =
					p.quantity === 'tone_amplitude'
						? { label: 'Amplitude', unit: `${r.unit} pk`, scale: 'linear' as const }
						: p.quantity === 'tone_phase'
							? { label: 'Phase', unit: 'deg', scale: 'linear' as const }
							: { label: 'Level against the statement', unit: 'dB', scale: 'linear' as const };
				return spectrum(p.quantity === 'tone_amplitude' ? 'Tone amplitude' : p.quantity === 'tone_phase' ? 'Tone phase' : 'Band level', y, series, 'time', tAxis);
			}
		}
		return null;
	}

	function liveHead(r: Run): MimoMixedLiveHead {
		const s = stateOf(r);
		return {
			run_id: r.id,
			source: r.source,
			state: s.state,
			phase: s.phase,
			mode: s.mode,
			t_s: s.t_s,
			at_level_s: s.at_level_s,
			remaining_at_level_s: s.remaining_at_level_s,
			level_db: s.level_db,
			held: s.held,
			alarm: s.alarm,
			limiting: s.limiting,
			floor_error_db: s.floor_error_db,
			floor_equalized: s.floor_equalized,
			scale: s.scale,
			parts: s.parts,
			drives: s.drives,
			class: r.station.class,
			loop: r.loops
		};
	}

	function dataFor(conn: MockConnection) {
		const sub = subs.get(conn);
		if (!sub || !run || run.state === 'configured') return;
		const r = run;
		const plots = sub.plots.map((p) => {
			const pick = r.config.controls.map((c, i) => (!p.channels || p.channels.includes(c) ? i : -1)).filter((i) => i >= 0);
			const v = plotView(r, p, pick);
			return { id: p.id, quantity: p.quantity, channels: pick.map((i) => r.config.controls[i]), parts: p.parts, views: v ? [v] : [] };
		});
		conn.emit('mimo_mixed.data', { ...liveHead(r), plots } as unknown as Record<string, unknown>);
	}

	function sendData() {
		for (const c of subs.keys()) dataFor(c);
	}

	function startLive() {
		if (liveTimer) return;
		liveTimer = setInterval(() => {
			if (run && isLive()) {
				const r = run;
				const all = r.config.controls.map((_, i) => i);
				host.broadcast('mimo_mixed.live', { ...liveHead(r), views: hasSpectra(r) ? [controlView(r, all), driveView(r)] : [] } as unknown as Record<string, unknown>);
				sendData();
			}
		}, LIVE_MS);
	}

	function stopLive() {
		if (liveTimer) clearInterval(liveTimer);
		liveTimer = null;
		sendData();
	}

	// ---- methods --------------------------------------------------------------------------------

	const needRun = (): Run => run ?? refuse('refused', 'not_configured', 'MIMO Mixed Mode is not configured; configure a test first.');
	const wrong = (r: Run, verb: string): never => refuse('refused', 'wrong_state', `${verb} does not apply while the run is ${r.state} (${r.phase.replace(/_/g, ' ')}).`, { state: r.state, phase: r.phase });
	const finished = (r: Run) => {
		if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.');
	};

	function newRun(raw: Record<string, unknown>, config: MimoMixedConfig, derived: MimoMixedDerived, grid: NonNullable<ReturnType<typeof onGrid>>): Run {
		runCount += 1;
		const st = host.station();
		const n = config.controls.length;
		const rows = host.rows();
		const unitRow = rows.find((r) => r.index === config.controls[0]);
		const parts: PartRun[] = config.parts.map((p) => {
			const b = p.body;
			const base = { id: p.id, kind: b.kind, name: p.name ? p.name : p.id, enabled: p.enabled !== false, ramp: 0, window: { on_s: p.window?.on_s ?? 0, off_s: p.window?.off_s ?? null }, direction: 'up' as const, sweeps: 0, err: Array(n).fill(0), scale: 1, limiting: false, ampHist: [], phHist: [] };
			const bandOf = (v: unknown, d: readonly number[]): [number, number] => {
				const x = Array.isArray(v) ? (Array.isArray(v[0]) ? v[0] : v) : d;
				return [Number(x[0]), Number(x[1])];
			};
			if (b.kind === 'tone') {
				const s = b.schedule;
				return { ...base, schedule: s, target: listOf(b.amplitude, n, { kind: 'constant', g_pk: 1 }).map((a) => (a.kind === 'constant' ? a.g_pk : (a.rows[0]?.value ?? 1))), phase: b.phases_stated ? (b.phase_deg ?? Array(n).fill(0)) : Array(n).fill(0), alarm: bandOf(b.alarm_db, MIMO_MIXED_DEFAULTS.tone_alarm_db), freq: s.kind === 'fixed' ? s.hz : s.kind === 'sweep' ? s.start_hz : null };
			}
			if (b.kind === 'narrowband') {
				const s = b.statement;
				const at = scheduleSpan(b.schedule)?.[0] ?? 0;
				const target = s.kind === 'raise' ? listOf(s.db, n, 0).map((db) => 10 ** (db / 10)) : listOf(s.kind === 'added' ? s.level_g2_hz : 0, n, 0).map((g, i) => 1 + g / Math.max(1e-9, logLog(grid.freqs.map((f, k) => [f, grid.psd[i][k]]), at)));
				return { ...base, schedule: b.schedule, target, phase: Array(n).fill(0), alarm: bandOf(b.alarm_db, MIMO_MIXED_DEFAULTS.band_alarm_db), freq: b.schedule.kind === 'fixed' ? b.schedule.hz : b.schedule.kind === 'sweep' ? b.schedule.start_hz : null };
			}
			return { ...base, schedule: null, target: Array(n).fill(1), phase: Array(n).fill(0), alarm: bandOf((b as { alarm_db?: unknown }).alarm_db, MIMO_MIXED_DEFAULTS.floor_alarm_db), freq: null };
		});
		return {
			id: `mimo-mixed-${String(runCount).padStart(4, '0')}`,
			raw,
			config,
			derived,
			state: 'configured',
			phase: 'idle',
			mode: config.schedule.mode,
			source: st.source,
			station: st.ref,
			named: host.models().sine_structure,
			unit: unitRow && typeof unitRow.units === 'string' ? unitRow.units : 'g',
			n,
			m: config.drives.length,
			freqs: grid.freqs,
			psd: grid.psd,
			levels: [...config.schedule.table_db],
			rung: 0,
			levelDb: null,
			t: null,
			phaseT: 0,
			partsT: null,
			held: false,
			floorErr: Array(n).fill(0),
			floorEqualized: false,
			loops: 0,
			parts,
			softening: 0,
			identification: null,
			prediction: null,
			overrides: [],
			end: null,
			stopping: null,
			timer: null,
			record: null,
			histT: [],
			logged: new Set()
		};
	}

	function armRun(r: Run) {
		const other = host.otherRun();
		if (other) refuse('refused', 'run_active', `Run ${other} holds the station.`, { run_id: other });
		const now = host.models().sine_structure;
		if (host.station().ref.kind === 'sim' && r.named !== now) refuse('refused', 'arm_refused', "The station's simulator models changed after this test was configured; apply the setup again.", { configured: r.named, station: now });
		if (r.record) return;
		const folder = host.recordFolder?.(r.id) ?? `~/Hertzforge_Data/${r.id}`;
		if (host.record === 'unwritable') refuse('refused', 'record_unwritable', `The run's record could not be written, so the run did not arm: ${folder}.`, { path: folder });
		r.record = { folder, streams: r.config.record?.streams === false ? 'off' : 'on', problem: null };
	}

	function configure(params: Record<string, unknown>, conn: MockConnection) {
		if (!isObject(params.config)) badParams('config', 'Configuring needs a test configuration.');
		const raw = params.config as Record<string, unknown>;
		const dry = params.dry_run === true;
		const res = check(raw);
		if (dry) {
			const g = res.grid;
			const views: View[] = g && res.derived ? [spectrum('Reference', { label: 'PSD', unit: `${res.config ? 'g' : 'g'}^2/Hz`, scale: 'log' }, g.psd.map((ys, i) => ser(`control ${(raw.controls as number[])[i]}`, 'reference', g.freqs, ys)))] : [];
			return { errors: res.errors, warnings: res.warnings, derived: res.derived, views };
		}
		host.own(conn);
		if (isLive() || host.otherRun()) refuse('refused', 'run_active', 'A run is live; stop it first.');
		if (!res.derived || !res.config || !res.grid) refuse('bad_request', 'config_invalid', `The configuration has ${res.errors.length} ${res.errors.length === 1 ? 'error' : 'errors'}.`, { errors: res.errors, warnings: res.warnings });
		if (run?.timer) clearInterval(run.timer);
		run = newRun(raw, res.config, res.derived, res.grid);
		log = [];
		addLog('state', `configured "${stateOf(run).title}" on ${run.station.class}`);
		stateChanged(true);
		return { state: stateOf(run), derived: res.derived, warnings: res.warnings };
	}

	const methods: Record<string, MockMethod> = {
		'mimo_mixed.reference.build_breakpoints': (params) => {
			const p = params as unknown as MimoBuildBreakpointsParams;
			if (!isObject(p.grid) || !Array.isArray(p.tables)) badParams('tables', 'Give the grid and the tables.');
			return buildReference(p);
		},
		'mimo_mixed.reference.import': () => refuse('refused', 'bad_params', 'This demonstration service reads no reference files: build the reference from breakpoints, or connect the control service to import one.'),
		'mimo_mixed.configure': (params, conn) => configure(params, conn),
		'mimo_mixed.config': () => {
			if (!run) return { run_id: null, config: null, derived: null, views: [] };
			const all = run.config.controls.map((_, i) => i);
			return { run_id: run.id, config: run.raw, derived: run.derived, views: [spectrum('Reference', { label: 'PSD', unit: psdUnit(run), scale: 'log' }, all.map((i) => ser(label(run as Run, i), 'reference', (run as Run).freqs, (run as Run).psd[i])))] };
		},
		'mimo_mixed.state': () => stateOf(run),
		'mimo_mixed.arm': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			finished(r);
			if (r.state !== 'configured') wrong(r, 'Arming');
			armRun(r);
			setPhase(r, 'armed', 'armed');
			addLog('station', 'armed: the station is open; nothing is driven');
			startLive();
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.start': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			finished(r);
			if (r.state !== 'configured' && r.state !== 'armed') wrong(r, 'Starting');
			if (r.state === 'configured') armRun(r);
			setPhase(r, 'running', 'sysid');
			r.t = 0;
			r.timer = setInterval(() => run === r && step(r, (TICK_MS / 1000) * host.speed), TICK_MS);
			startLive();
			addLog('state', 'identification: independent random on every drive at once');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.proceed': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'review') wrong(r, 'Proceeding');
			if (r.phase === 'gate_failed') refuse('refused', 'gate_failed', 'The identification did not pass its coherence gate, so the run holds; override with your reason, or stop.');
			if (r.phase === 'prediction_exceeded') refuse('refused', 'prediction_exceeded', 'The prediction puts a drive over its ceiling, so the run holds; change the setup, or override with your reason (the ceilings and aborts still hold the drive).');
			engage(r);
			addLog('note', 'the operator reviewed the identification and proceeded');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.override': (params, conn) => {
			if (typeof params.ack !== 'string' || !params.ack.trim()) badParams('ack', "An override is the operator's own words on why the run should go on; it cannot be empty.");
			if (params.reason !== undefined && params.reason !== null && params.reason !== 'gate_failed' && params.reason !== 'prediction_exceeded') badParams('reason', '"reason" is "gate_failed" or "prediction_exceeded": what the override acknowledges');
			host.own(conn);
			const r = needRun();
			if (r.state !== 'review') wrong(r, 'Overriding');
			if (r.phase !== 'gate_failed' && r.phase !== 'prediction_exceeded') refuse('refused', 'override_not_needed', 'The identification passed its gate and the prediction is within every ceiling; proceed instead.');
			const holds = r.phase as MimoMixedOverrideReason;
			if (params.reason && params.reason !== holds)
				refuse('refused', 'override_not_needed', `The run holds for ${holds === 'gate_failed' ? 'a failed coherence gate' : 'a prediction over a drive ceiling'}, not for what this override names; acknowledge what holds the run (reason "${holds}").`, { holds });
			const ack = (params.ack as string).trim();
			const by = host.clientName(conn);
			r.overrides.push({ by, text: ack, reason: holds });
			addLog('override', `override (${holds}) by ${by}: ${ack}`);
			if (holds === 'gate_failed') {
				review(r);
				if ((r.phase as MimoMixedPhase) !== 'awaiting_review' || r.prediction?.forecast.refused) {
					stateChanged(true);
					return stateOf(r);
				}
			}
			engage(r);
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.level': (params, conn) => {
			const delta = isNum(params.delta_db) ? params.delta_db : null;
			const to = isNum(params.to_db) ? params.to_db : null;
			if ((delta === null) === (to === null)) badParams('delta_db', 'A level change is a step ("delta_db") or a level ("to_db"), one of the two.');
			host.own(conn);
			const r = needRun();
			if (!driving(r)) wrong(r, 'A level change');
			if (r.mode !== 'manual') refuse('refused', 'manual_mode_required', 'The level moves by hand only in manual mode; switch with mimo_mixed.mode first.', { mode: 'auto' });
			if (to !== null && !r.levels.some((l) => Math.abs(l - to) < 1e-9)) refuse('refused', 'not_a_rung', `${to} dB is not a level of the schedule.`, { levels_db: r.levels });
			const low = Math.min(r.config.schedule.start_db, ...r.levels);
			const want = to ?? (r.levelDb ?? r.config.schedule.start_db) + (delta as number);
			const level = Math.max(low, Math.min(0, want));
			r.levelDb = level;
			r.rung = r.levels.filter((d) => d > r.config.schedule.start_db && d < 0 && d <= level + 1e-9).length;
			if (r.phase !== 'floor_only') {
				r.phase = level >= -1e-6 ? 'full_level' : 'engaged';
				r.phaseT = 0;
			}
			addLog('level', `level to ${round(level, 2)} dB asked`);
			stateChanged(true);
			return { level_db: round(level, 2) };
		},
		'mimo_mixed.mode': (params, conn) => {
			if (params.mode !== 'auto' && params.mode !== 'manual') badParams('mode', '"mode" is "auto" or "manual"');
			host.own(conn);
			const r = needRun();
			finished(r);
			const mode = params.mode as 'auto' | 'manual';
			if (r.mode !== mode) addLog('level', `mode ${mode}`);
			r.mode = mode;
			stateChanged(true);
			return { mode };
		},
		'mimo_mixed.hold': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (!driving(r)) wrong(r, 'Holding');
			r.held = true;
			addLog('hold', 'progress held');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.resume': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (!driving(r)) wrong(r, 'Resuming');
			r.held = false;
			addLog('hold', 'progress resumed');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_mixed.part': (params, conn) => {
			if (typeof params.id !== 'string' || !params.id) badParams('id', 'Switching a part names it by "id".');
			if (typeof params.on !== 'boolean') badParams('on', '"on" is true (switch the part on) or false (switch it off).');
			host.own(conn);
			const r = needRun();
			const id = params.id as string;
			const on = params.on as boolean;
			const p = r.parts.find((x) => x.id === id);
			if (!p) refuse('refused', 'part_refused', `The test has no part "${id}".`, { id, why: 'unknown_part' });
			if (p.kind === 'floor' && !on) refuse('refused', 'part_refused', 'The floor plays for the whole run; it cannot be switched off.', { id, why: 'floor' });
			if (!driving(r)) wrong(r, 'Switching a part');
			if (p.enabled !== on) {
				p.enabled = on;
				addLog('part', `part ${id} switched ${on ? 'on' : 'off'} by ${host.clientName(conn)}`);
				stateChanged(true);
			}
			return { id, on };
		},
		'mimo_mixed.stop': (_p, conn) => {
			const r = run;
			if (!r) return stateOf(null);
			if (!isLive()) return stateOf(r);
			if (r.state === 'armed' || r.state === 'review' || r.phase === 'sysid' || r.phase === 'coherence_gate') end(r, 'operator_stop', 'stopped', { message: `Stopped by ${host.clientName(conn)} before driving.` });
			else if (!r.stopping) {
				beginEnd(r, 'operator_stop', 'stopped');
				stateChanged(true);
			}
			return stateOf(r);
		},
		'mimo_mixed.abort': (_p, conn) => {
			const r = run;
			if (!r) return stateOf(null);
			if (isLive()) end(r, 'operator_abort', 'aborted', { message: `Aborted by ${host.clientName(conn)} at ${r.levelDb === null ? 'before driving' : `${round(r.levelDb, 2)} dB after ${round(r.t ?? 0, 2)} s`}.` });
			return stateOf(r);
		},
		'mimo_mixed.reset': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (isLive()) refuse('refused', 'run_active', 'The run is live; stop it first.');
			const res = check(r.raw);
			if (!res.derived || !res.config || !res.grid) refuse('bad_request', 'config_invalid', `The configuration has ${res.errors.length} ${res.errors.length === 1 ? 'error' : 'errors'}.`, { errors: res.errors, warnings: res.warnings });
			run = newRun(r.raw, res.config, res.derived, res.grid);
			log = [];
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_mixed.clear': (_p, conn) => {
			host.own(conn);
			if (isLive()) refuse('refused', 'run_active', 'The run is live; stop it first.');
			run = null;
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_mixed.log': (params) => {
			const since = params.since === undefined || params.since === null ? 0 : params.since;
			if (!Number.isInteger(since) || (since as number) < 0) badParams('since', '"since" is a whole number from 0');
			return { entries: log.filter((e) => e.seq >= (since as number)), next: log.length };
		},
		'mimo_mixed.subscribe': (params, conn) => {
			if (params.default === true) {
				subs.delete(conn);
				return { default: true, rate_hz: RATE_DEFAULT, plots: null, quantities: [...MIMO_MIXED_QUANTITIES] };
			}
			let rate = RATE_DEFAULT;
			if (params.rate_hz !== undefined && params.rate_hz !== null) {
				if (!isNum(params.rate_hz) || params.rate_hz < RATE_MIN || params.rate_hz > RATE_MAX) badParams('rate_hz', `"rate_hz" is a number of frames per second from ${RATE_MIN} to ${RATE_MAX}`);
				rate = params.rate_hz as number;
			}
			if (!Array.isArray(params.plots)) badParams('plots', 'a subscription is {"default": true} or a list of "plots"');
			const list = params.plots as unknown[];
			if (!list.length || list.length > MAX_PLOTS) badParams('plots', `from 1 to ${MAX_PLOTS} plots`);
			const plots = list.map((p, k) => {
				if (!isObject(p)) return badParams('plots', `plot ${k} is an object`);
				if (typeof p.id !== 'string' || !p.id) return badParams('plots', `plot ${k} has no "id"`);
				if (!MIMO_MIXED_QUANTITIES.includes(p.quantity as MimoMixedQuantity)) return badParams('plots', `plot ${k}'s quantity is one of ${MIMO_MIXED_QUANTITIES.join(', ')}`);
				let channels: number[] | null = null;
				if (p.channels !== undefined && p.channels !== null) {
					if (!Array.isArray(p.channels)) return badParams('plots', `plot ${k}'s channels are a list`);
					if (!p.channels.every((c) => Number.isInteger(c) && c >= 0)) return badParams('plots', `plot ${k}'s channels are indexes`);
					channels = p.channels as number[];
				}
				let parts: string[] | null = null;
				if (p.parts !== undefined && p.parts !== null) {
					if (!Array.isArray(p.parts)) return badParams('plots', `plot ${k}'s parts are a list`);
					if (!p.parts.every((x) => typeof x === 'string')) return badParams('plots', `plot ${k}'s parts are ids`);
					parts = p.parts as string[];
				}
				return { id: p.id, quantity: p.quantity as MimoMixedQuantity, channels, parts };
			});
			subs.set(conn, { rate, plots });
			queueMicrotask(() => dataFor(conn));
			return { default: false, rate_hz: rate, plots, quantities: [...MIMO_MIXED_QUANTITIES] };
		}
	};
	// The verbs the service does not build, refused by their names before anything else.
	const NOT_BUILT_WORDS: Record<string, string> = {
		adjust_not_built: "MIMO Mixed Mode's laws take no live parameters (neither a MIMO Random floor law nor a MIMO Sine tone law declares one); configure the parameters instead.",
		exclude_not_built: 'MIMO Mixed Mode does not take a control out of control at the gate: the floor, every tone and every band are stated per control; configure the controls instead, or override the gate.',
		identification_save_not_built: 'MIMO Mixed Mode does not save its identification as an identification set; the run\'s record keeps it.',
		identification_load_not_built: 'MIMO Mixed Mode identifies on every run; it cannot load an identification set.'
	};
	for (const [method, reason] of Object.entries(MIMO_MIXED_NOT_BUILT)) methods[method] = () => refuse('refused', reason, NOT_BUILT_WORDS[reason]);

	return {
		methods,
		isLive,
		liveId: () => (isLive() && run ? run.id : null),
		detach: (conn) => {
			subs.delete(conn);
		},
		seatExpired: () => {
			if (!run || !isLive()) return;
			const message = `Nobody held control for ${host.timeoutS} s, so the service stopped the run with its controlled stop.`;
			if (run.state === 'armed' || run.state === 'review' || run.phase === 'sysid' || run.phase === 'coherence_gate') end(run, 'safe_stop', 'stopped', { message });
			else if (!run.stopping) {
				beginEnd(run, 'safe_stop', 'stopped');
				stateChanged(true);
			}
		},
		floorLaws: () => FLOOR_LAWS
	};
}
