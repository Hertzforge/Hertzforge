/**
 * Mock Sine (schema/PROTOCOL.md, "Sine" and the Sine world of "Control Laws"), on the mock control
 * service's run slot and controller seat.
 *
 * A believable closed-loop run: identification with a noise burst, the coherence check, the review
 * hold, then the tone engaged at the first level of the schedule. A sweep moves the frequency at
 * its rate, turns at the band's edges and counts its one-way sweeps; a dwell holds one frequency; a
 * step test moves from step to step. The automatic ladder moves on after each level's sweeps or
 * seconds and the run ends at the test's end with the controlled stop, the drive ramping down at
 * 120 dB a second. Hold freezes the progress, Reverse turns the sweep, Jog and Go to move the
 * frequency, a live law parameter changes from the next tick. Stop ramps; abort ends at once; with
 * nobody in control the run goes on and safe-stops after the timeout.
 *
 * It refuses what the service refuses, by the same reasons and on the same parameters: channels or
 * a point count on a plot that does not take them, a sweep verb on a test that is not a sweep, a
 * frequency outside the band, an adjust of a parameter that is not live or out of its range, a
 * level change on the automatic ladder, a relative path. Every value is synthetic, made only so
 * the shell has something to draw. Nothing here is the service's profile math or control.
 */

import type { ChannelRow, DataSource, LogEntry, LogKind, ReadingRow, RefusalReason, Series, SpectrumView, StationRef, View } from '../types';
import type { LawEntry, LawParam } from '../mimo';
import type {
	SineAlarm,
	SineConfig,
	SineCrossover,
	SineDerived,
	SineDirection,
	SineEnd,
	SineIdentificationFacts,
	SineItemReason,
	SineLive,
	SineLiveChannel,
	SineLiveQuantity,
	SineOutcome,
	SinePhase,
	SineProfile,
	SineProfileRow,
	SineQuantity,
	SineReportItem,
	SineRunState,
	SineState,
	SineSubscribePlot,
	SineTest,
	SineTrackingVerdict,
	SineTrip
} from '../sine';
import type { ShockMockHost } from './shock';
import { MockError, type MockConnection, type MockMethod } from './socket';

const G_IPS2 = 386.089;
const TICK_MS = 50;
const LIVE_MS = 200;
const SYSID_S = 1.5;
const GATE_S = 0.3;
const GRID = 256;
const STOP_DB_PER_S = 120;
/** A dwell's plots hold the last minute of test time. */
const DWELL_WINDOW_S = 60;
const FS_DEFAULT = 16384;

const QUANTITIES: SineLiveQuantity[] = ['control', 'drive', 'response', 'phase', 'thd', 'time', 'identification', 'law', 'readings'];
const WITH_CHANNELS: SineLiveQuantity[] = ['response', 'phase', 'time'];

function refuse(code: 'refused' | 'bad_request', reason: RefusalReason | 'not_a_sweep' | 'out_of_band' | 'adjust_refused', message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason as RefusalReason, detail);
}

function bad(param: string, message: string): never {
	refuse('bad_request', 'bad_params', message, { param });
}

const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const absolute = (p: unknown): p is string => typeof p === 'string' && /^(\/|[A-Za-z]:\\)/.test(p);
const f64 = (a: number[]) => Float64Array.from(a);
const db20 = (x: number) => 20 * Math.log10(x);
/** The last finite value of a list, or null. */
function lastFinite(list: readonly number[] | undefined): number | null {
	if (!list) return null;
	for (let i = list.length - 1; i >= 0; i--) if (Number.isFinite(list[i])) return list[i];
	return null;
}

// ---------------------------------------------------------------------------------------------
// The law catalog

const builtIn = { status: 'built_in' as const, date: null, tool_version: null, cases: [], hardware: true, message: 'Built in: cleared for hardware.' };
const unchecked = { status: 'unchecked' as const, date: null, tool_version: null, cases: [], hardware: false, message: 'Not checked: runs on the simulator; check it before it drives hardware.' };

/** The built-in law, as the service lists it. */
const SERVO_BLEND: LawEntry = {
	id: 'servo_blend',
	world: 'sine',
	builtin: true,
	name: 'Servo blend',
	version: '0.1.0',
	author: 'Hertzforge contributors',
	summary: "Moves the tone's amplitude each tick by a proportional-integral servo on the error in dB, blended with the identification's prediction of the drive at the present frequency. The controller caps every change and holds the drive under the limit channels and the drive ceiling.",
	sha256: null,
	check: builtIn,
	params: [
		{ key: 'blend_w', label: 'Prediction weight', help: "How much of each tick's change follows the identification's prediction of the drive (1) rather than the servo on the measured error (0). It can be changed while the test runs.", kind: 'number', unit: '', min: 0, max: 1, step: 0.05, default: 0.5 },
		{ key: 'compression', label: 'Compression', help: 'How fast the servo corrects: slow (integral gain 2, at most 20 dB/s), fast (6, at most 60 dB/s), or custom with the two values below.', kind: 'choice', options: ['slow', 'fast', 'custom'], default: 'slow' },
		{ key: 'ki', label: 'Integral gain', help: "The servo's integral gain, with custom compression.", kind: 'number', unit: '', min: 0.1, max: 20, step: 0.1, default: 2 },
		{ key: 'max_rate_db_per_s', label: 'Largest servo rate', help: 'The fastest the servo moves the drive, with custom compression.', kind: 'number', unit: 'dB/s', min: 1, max: 240, step: 1, default: 20 },
		{ key: 'kp', label: 'Proportional gain', help: "The servo's proportional gain on the error in dB.", kind: 'number', unit: '', min: 0, max: 2, step: 0.05, default: 0.3 }
	],
	diagnostics: [
		{ key: 'servo_step_db', label: 'Servo step', unit: 'dB', help: "The servo's part of the last tick's change.", layout: null },
		{ key: 'prediction_step_db', label: 'Prediction step', unit: 'dB', help: "The prediction's part of the last tick's change.", layout: null },
		{ key: 'total_step_db', label: 'Total step', unit: 'dB', help: "The change the law asked for on the last tick, before the controller's caps.", layout: null },
		{ key: 'servo_level_db', label: 'Servo level', unit: 'dB', help: "The servo's own drive level after the last tick, in dB relative to one drive unit.", layout: null }
	],
	shapes: null,
	needs: null,
	live: ['blend_w']
};

function importedLaw(path: string): LawEntry {
	let h = 0;
	for (const ch of path) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
	const sha = (h.toString(16).padStart(8, '0') + '51e0a1').repeat(5).slice(0, 64);
	const misbehave = /misbehave/i.test(path);
	if (misbehave)
		return {
			id: sha,
			world: 'sine',
			builtin: false,
			name: 'Sine misbehave',
			version: '0.1.0',
			author: 'law author',
			summary: 'A law that answers late on purpose, for seeing how the core stops a law that misses.',
			sha256: sha,
			check: unchecked,
			params: [{ key: 'after_s', label: 'Late after', help: 'Seconds of the run before every answer comes late.', kind: 'number', unit: 's', min: 0, max: 60, step: 1, default: 2 }],
			diagnostics: [],
			shapes: null,
			needs: null,
			live: []
		};
	return {
		id: sha,
		world: 'sine',
		builtin: false,
		name: 'Sine PI',
		version: '0.1.0',
		author: 'law author',
		summary: "A PI loop on the error in dB, seeded from the identification's prediction.",
		sha256: sha,
		check: unchecked,
		params: [
			{ key: 'kp', label: 'Proportional gain', help: 'The share of the error closed each tick.', kind: 'number', unit: '', min: 0, max: 2, step: 0.05, default: 0.5 },
			{ key: 'ki', label: 'Integral gain', help: 'The share of the summed error added each tick.', kind: 'number', unit: '', min: 0, max: 1, step: 0.01, default: 0.1 },
			{ key: 'start_offset_db', label: 'Start offset', help: "An offset on the law's own first drive, for seeing how the core's guards respond.", kind: 'number', unit: 'dB', min: -12, max: 12, step: 1, default: 0 }
		],
		diagnostics: [{ key: 'error_db', label: 'Error', unit: 'dB', help: 'The error the law closed on.', layout: null }],
		shapes: null,
		needs: null,
		live: ['kp']
	};
}

const numberParam = (p: LawParam): p is Extract<LawParam, { kind: 'number' | 'integer' }> => p.kind === 'number' || p.kind === 'integer';

// ---------------------------------------------------------------------------------------------
// The profile, made up for the screen

/** The exponent of frequency in a quantity's acceleration: g, in/s and in pk-pk. */
const POWER: Record<SineQuantity, number> = { accel_g: 0, velocity_ips: 1, disp_pp_in: 2 };

/** A row's value as acceleration at `f`, g peak. */
function accelOf(q: SineQuantity, v: number, f: number): number {
	const w = 2 * Math.PI * f;
	if (q === 'velocity_ips') return (v * w) / G_IPS2;
	if (q === 'disp_pp_in') return ((v / 2) * w * w) / G_IPS2;
	return v;
}

function valueIn(q: SineQuantity, a: number, f: number): number {
	const w = 2 * Math.PI * f;
	if (q === 'velocity_ips') return (a * G_IPS2) / w;
	if (q === 'disp_pp_in') return (2 * a * G_IPS2) / (w * w);
	return a;
}

interface ProfilePoint {
	accel_g: number;
	alarm: [number, number];
	abort: [number, number];
}

function profileAt(p: SineProfile, f: number): ProfilePoint {
	const rows = p.rows;
	const alarm0 = (p.alarm_db ?? [1.5, 1.5]) as [number, number];
	const abort0 = (p.abort_db ?? [3, 3]) as [number, number];
	let i = 0;
	while (i < rows.length - 2 && f >= rows[i + 1].freq_hz) i++;
	const a = rows[i];
	const b = rows[i + 1] ?? a;
	let value: number;
	if (f >= b.freq_hz && b === rows[rows.length - 1]) value = accelOf(b.quantity, b.value, f);
	else if (a.quantity === b.quantity && b.freq_hz > a.freq_hz) {
		const x = Math.log(Math.max(f, a.freq_hz) / a.freq_hz) / Math.log(b.freq_hz / a.freq_hz);
		value = accelOf(a.quantity, Math.exp(Math.log(a.value) + x * (Math.log(b.value) - Math.log(a.value))), f);
	} else value = accelOf(a.quantity, a.value, f);
	return { accel_g: value, alarm: (a.alarm_db ?? alarm0) as [number, number], abort: (a.abort_db ?? abort0) as [number, number] };
}

function logGrid(lo: number, hi: number, n = GRID): number[] {
	if (!(hi > lo)) return [lo];
	return Array.from({ length: n }, (_, i) => lo * (hi / lo) ** (i / (n - 1)));
}

function crossovers(p: SineProfile): SineCrossover[] {
	const out: SineCrossover[] = [];
	for (let i = 1; i < p.rows.length; i++) {
		const a = p.rows[i - 1];
		const b = p.rows[i];
		if (a.quantity === b.quantity) continue;
		// where the held left row and the right row's value read the same acceleration
		const n = POWER[a.quantity] - POWER[b.quantity];
		const ka = accelOf(a.quantity, a.value, 1);
		const kb = accelOf(b.quantity, b.value, 1);
		const f = n !== 0 ? (kb / ka) ** (1 / n) : b.freq_hz;
		out.push({ freq_hz: round(Number.isFinite(f) && f > 0 ? f : b.freq_hz, 1), from: a.quantity, to: b.quantity });
	}
	return out;
}

function peaksOver(p: SineProfile, band: [number, number]) {
	const lo = band[0];
	const hi = band[1] > band[0] ? band[1] : band[0];
	let ga = 0;
	let gv = 0;
	let gd = 0;
	for (const f of logGrid(lo, hi, 400)) {
		const a = profileAt(p, f).accel_g;
		ga = Math.max(ga, a);
		gv = Math.max(gv, valueIn('velocity_ips', a, f));
		gd = Math.max(gd, valueIn('disp_pp_in', a, f));
	}
	return { peak_accel_g: round(ga, 4), peak_velocity_ips: round(gv, 4), peak_disp_pp_in: round(gd, 5) };
}

const freqAxis = { label: 'Frequency', unit: 'Hz', scale: 'log' as const };

/** The profile at full level as acceleration, velocity and displacement, as the service draws it. */
function profileViews(p: SineProfile, band: [number, number]): SpectrumView[] {
	const lo = Math.min(band[0], p.rows[0]?.freq_hz ?? band[0]);
	const hi = Math.max(band[1], p.rows.at(-1)?.freq_hz ?? band[1]);
	const f = logGrid(Math.max(lo, 0.1), hi, 200);
	const pts = f.map((x) => profileAt(p, x));
	const at = (sign: number, side: 'alarm' | 'abort') => f64(pts.map((pt) => pt.accel_g * 10 ** ((sign * (side === 'alarm' ? pt.alarm : pt.abort)[sign > 0 ? 1 : 0]) / 20)));
	const x = f64(f);
	const shaped = (q: SineQuantity) => f64(pts.map((pt, i) => valueIn(q, pt.accel_g, f[i])));
	return [
		{
			view: 'spectrum',
			title: 'Profile',
			x: freqAxis,
			y: { label: 'Acceleration', unit: 'g', scale: 'log' },
			series: [
				{ name: 'Profile', role: 'reference', x, y: f64(pts.map((pt) => pt.accel_g)), shape: 'line' },
				{ name: 'Alarm high', role: 'tolerance', x, y: at(1, 'alarm'), shape: 'line' },
				{ name: 'Alarm low', role: 'tolerance', x, y: at(-1, 'alarm'), shape: 'line' },
				{ name: 'Abort high', role: 'abort', x, y: at(1, 'abort'), shape: 'line' },
				{ name: 'Abort low', role: 'abort', x, y: at(-1, 'abort'), shape: 'line' }
			],
			link: 'frequency'
		},
		{ view: 'spectrum', title: 'Velocity', x: freqAxis, y: { label: 'Velocity', unit: 'in/s', scale: 'log' }, series: [{ name: 'Profile', role: 'reference', x, y: shaped('velocity_ips'), shape: 'line' }], link: 'frequency' },
		{ view: 'spectrum', title: 'Displacement', x: freqAxis, y: { label: 'Displacement', unit: 'in pk-pk', scale: 'log' }, series: [{ name: 'Profile', role: 'reference', x, y: shaped('disp_pp_in'), shape: 'line' }], link: 'frequency' }
	];
}

type Item = SineReportItem;
const item = (reason: SineItemReason, message: string, field?: string, row?: number): Item => {
	const it: Item = { reason, message } as Item;
	if (field) it.field = field;
	if (row !== undefined) it.row = row;
	return it;
};

/** A profile's own findings, at `at` (`config.profile`, a limit channel's profile). */
function checkProfile(p: SineProfile, at: string, errors: Item[], warnings: Item[]) {
	if (!Array.isArray(p.rows) || p.rows.length < 2) {
		errors.push(item('profile_too_short', 'A profile has at least two rows.', `${at}.rows`));
		return;
	}
	p.rows.forEach((r, i) => {
		if (!(r.freq_hz > 0)) errors.push(item('frequency_not_positive', `Row ${i + 1}: the frequency is above 0 Hz.`, `${at}.rows[${i}].freq_hz`, i));
		else if (i > 0 && !(r.freq_hz > p.rows[i - 1].freq_hz)) errors.push(item('frequency_not_ascending', `Row ${i + 1}: frequencies rise from row to row.`, `${at}.rows[${i}].freq_hz`, i));
		if (!(r.value > 0)) errors.push(item('value_not_positive', `Row ${i + 1}: the value is above 0.`, `${at}.rows[${i}].value`, i));
		for (const k of ['alarm_db', 'abort_db'] as const) {
			const v = r[k];
			if (v && !(v[0] > 0 && v[1] > 0)) errors.push(item('out_of_range', `Row ${i + 1}: each side of a band is above 0 dB.`, `${at}.rows[${i}].${k}`, i));
		}
	});
	for (const k of ['alarm_db', 'abort_db'] as const) {
		const v = p[k];
		if (v && !(v[0] > 0 && v[1] > 0)) errors.push(item('out_of_range', 'Each side of a band is above 0 dB.', `${at}.${k}`));
	}
	const al = p.alarm_db ?? [1.5, 1.5];
	const ab = p.abort_db ?? [3, 3];
	if (al[0] > ab[0] || al[1] > ab[1]) warnings.push(item('alarm_wider_than_abort', 'The alarm band is wider than the abort band on one side: that side would abort before it alarms.', `${at}.abort_db`));
	if (errors.length) return;
	for (let i = 1; i < p.rows.length; i++) {
		const a = p.rows[i - 1];
		const b = p.rows[i];
		if (a.quantity === b.quantity) continue;
		const jump = Math.abs(db20(accelOf(a.quantity, a.value, b.freq_hz) / accelOf(b.quantity, b.value, b.freq_hz)));
		if (jump > 0.5) warnings.push(item('join_discontinuity', `Row ${i + 1}: the profile jumps ${jump.toFixed(1)} dB where it changes from ${a.quantity.replace('_', ' ')} to ${b.quantity.replace('_', ' ')}.`, `${at}.rows[${i}]`, i));
	}
}

// ---------------------------------------------------------------------------------------------
// The test, made up for the screen

function bandOf(t: SineTest): [number, number] {
	return t.kind === 'dwell' ? [t.freq_hz, t.freq_hz] : [t.band_hz[0], t.band_hz[1]];
}

/** One way across the band, seconds. */
function sweepSeconds(t: Extract<SineTest, { kind: 'sweep' }>): number {
	const [lo, hi] = t.band_hz;
	return t.rate.law === 'log' ? (Math.log2(hi / lo) / t.rate.oct_per_min) * 60 : (hi - lo) / t.rate.hz_per_s;
}

function stepFreqs(t: Extract<SineTest, { kind: 'step' }>): number[] {
	const [lo, hi] = t.band_hz;
	const out: number[] = [];
	if (t.spacing === 'linear') {
		const n = Math.max(1, Math.round(Math.log2(hi / lo) * t.steps_per_octave));
		for (let i = 0; i <= n; i++) out.push(lo + ((hi - lo) * i) / n);
	} else for (let f = lo; f <= hi * (1 + 1e-9) && out.length < 400; f *= 2 ** (1 / t.steps_per_octave)) out.push(f);
	return out;
}

const dwellSecondsAt = (d: { seconds: number } | { cycles: number } | null | undefined, f: number) => (!d ? Math.max(2, 30 / f) : 'seconds' in d ? d.seconds : d.cycles / f);

/** Test time the schedule takes, seconds; null in manual. */
function testSeconds(c: SineConfig): number | null {
	if ((c.schedule?.mode ?? 'auto') === 'manual') return null;
	const t = c.test;
	if (t.kind === 'sweep') {
		const one = sweepSeconds(t);
		const bySweeps = t.end.sweeps ? t.end.sweeps * one : null;
		const byTime = t.end.duration_s ?? null;
		return round(bySweeps !== null && byTime !== null ? Math.min(bySweeps, byTime) : (bySweeps ?? byTime ?? one), 1);
	}
	if (t.kind === 'dwell') return round('seconds' in t.end ? t.end.seconds : t.end.cycles / t.freq_hz, 1);
	if (t.kind === 'step') return round(stepFreqs(t).reduce((s, f) => s + dwellSecondsAt(t.dwell, f), 0), 1);
	return null;
}

/** The sweep against the tracking filter: the frequency's rate of change over the filter's bandwidth squared. */
function trackingOf(c: SineConfig): SineTrackingVerdict {
	const t = c.test;
	if (t.kind !== 'sweep') return { verdict: 'ok', ratio: 0, at_hz: null, message: null };
	const tr = c.control?.tracking ?? { kind: 'proportional', pct: 10, max_hz: 100 };
	let worst = 0;
	let at = t.band_hz[0];
	for (const f of logGrid(t.band_hz[0], t.band_hz[1], 64)) {
		const rate = t.rate.law === 'log' ? (t.rate.oct_per_min / 60) * Math.LN2 * f : t.rate.hz_per_s;
		const bw = tr.kind === 'fixed' ? tr.hz : Math.min((tr.pct / 100) * f, tr.max_hz);
		const r = rate / (bw * bw);
		if (r > worst) {
			worst = r;
			at = f;
		}
	}
	const ratio = round(worst, 3);
	if (ratio >= 1) return { verdict: 'refused', ratio, at_hz: round(at, 1), message: `The sweep is too fast for the tracking filter near ${round(at, 1)} Hz: the readout would fall far behind the tone. Slow the sweep or widen the filter.` };
	if (ratio >= 0.25) return { verdict: 'advisory', ratio, at_hz: round(at, 1), message: `Near ${round(at, 1)} Hz the readout runs measurably low for this sweep rate; a wider filter or a slower sweep keeps it on.` };
	return { verdict: 'ok', ratio, at_hz: null, message: null };
}

/** The plant of the simulated station: two modes, 200 and 550 Hz, Q 2.5; g per volt. */
const mode = (f: number, fn: number, q: number) => 1 / Math.sqrt((1 - (f / fn) ** 2) ** 2 + (f / (fn * q)) ** 2);
const plantGain = (f: number) => 40 * (0.6 * mode(f, 200, 2.5) + 0.4 * mode(f, 550, 2.5));
const plantPhase = (f: number) => (-180 / Math.PI) * Math.atan2(f / (200 * 2.5), 1 - (f / 200) ** 2);

// ---------------------------------------------------------------------------------------------
// The run

interface Trace {
	/** On the sweep grid in a sweep or a step test; against test time in a dwell. */
	x: number[];
	measured: number[];
	drive: number[];
	thd: number[];
	amp: Map<number, number[]>;
	phase: Map<number, number[]>;
}

interface Run {
	id: string;
	config: SineConfig;
	derived: SineDerived;
	views: SpectrumView[];
	law: LawEntry;
	lawLive: Record<string, number>;
	state: SineRunState;
	phase: SinePhase;
	station: StationRef;
	source: DataSource;
	mode: 'auto' | 'manual';
	levels: number[];
	rung: number;
	/** The scheduled level, and the level the drive is at now. */
	commanded: number;
	level: number;
	freq: number;
	direction: SineDirection;
	held: boolean;
	sweepsAtLevel: number;
	sweepsTotal: number;
	/** Station time since engage (holds included), test time (holds excluded), time at this level. */
	t: number;
	testT: number;
	atLevelT: number;
	phaseT: number;
	step: number;
	stepT: number;
	stopOutcome: SineOutcome;
	stopBy: string | undefined;
	/** What ends the run once its ramp is done, when a protection or the law ended it. */
	pendingTrip: SineTrip | null;
	rampT: number;
	drive: number;
	measured: number;
	target: number;
	notch: boolean;
	alarm: boolean;
	alarms: SineAlarm[];
	/** Control rows whose overload has latched: 95 % of the abort band's top, held to the end. */
	overloaded: Set<number>;
	misses: number;
	capped: number;
	values: Record<string, number>;
	identification: SineIdentificationFacts | null;
	feasibility: SineState['feasibility'];
	end: SineEnd | null;
	timer: ReturnType<typeof setInterval> | null;
	trace: Trace;
	grid: number[];
}

export interface SineMock {
	methods: Record<string, MockMethod>;
	/** The Sine world's laws, built in first. */
	laws(): LawEntry[];
	isLive(): boolean;
	/** The run's id while it is live, for another run's `run_active` refusal. */
	liveId(): string | null;
	detach(conn: MockConnection): void;
	/** Nobody held control for the timeout: the run safe-stops. */
	seatExpired(): void;
}

const LIVE: SineRunState[] = ['armed', 'running', 'review', 'stopping'];
const DRIVING: SinePhase[] = ['engaged', 'full_level'];

/** What the Sine mock needs of the control service around it: classical shock's. */
export type SineMockHost = ShockMockHost;

export function createSineMock(host: SineMockHost): SineMock {
	let run: Run | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastStateKey = '';
	let imported: LawEntry[] = [];
	const subs = new Map<MockConnection, { rate: number; plots: SineSubscribePlot[]; acc: number }>();
	let liveTimer: ReturnType<typeof setInterval> | null = null;

	const laws = () => [SERVO_BLEND, ...imported];
	const isLive = () => run !== null && LIVE.includes(run.state);
	const inputs = () => host.rows().filter((r) => r.enabled && r.role !== 'drive');
	const controlRows = () => host.rows().filter((r) => r.enabled && r.role === 'control');

	function addLog(kind: LogKind, text: string) {
		const e: LogEntry = { seq: log.length, at_unix: Date.now() / 1000, kind, text };
		log.push(e);
		if (run) host.broadcast('sine.log', { run_id: run.id, entries: [e] });
	}

	function emptyState(): SineState {
		return {
			run_id: null,
			state: 'unconfigured',
			phase: 'idle',
			title: null,
			source: null,
			station: null,
			grid: null,
			clock: null,
			test: null,
			mode: null,
			levels_db: null,
			level_db: null,
			rung: null,
			frequency_hz: null,
			direction: null,
			sweeps: null,
			held: false,
			elapsed_s: null,
			at_level_s: null,
			remaining_s: null,
			alarm: false,
			notch: false,
			identification: null,
			law: null,
			feasibility: null,
			end: null
		};
	}

	/** A dwell and a resonance search plot against station time; a sweep and a step test against frequency. */
	const dwellAxis = (r: Run) => r.config.test.kind === 'dwell' || r.config.test.kind === 'rstd';
	const engaged = (r: Run) => DRIVING.includes(r.phase) || r.phase === 'ramping_down' || (r.state === 'finished' && r.t > 0);

	function sweepsOf(r: Run): SineState['sweeps'] {
		if (r.config.test.kind !== 'sweep') return null;
		const per = r.rung >= r.levels.length - 1 ? r.config.schedule?.full_level : r.config.schedule?.per_level;
		const of = r.mode === 'auto' && per && 'sweeps' in per ? per.sweeps : null;
		return { done: r.sweepsAtLevel, of };
	}

	function stateOf(r: Run | null): SineState {
		if (!r) return emptyState();
		const started = r.state !== 'configured' && r.state !== 'blocked';
		const on = engaged(r);
		const total = r.derived.test_s;
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.config.title ?? null,
			source: r.source,
			station: r.station,
			grid: r.derived.grid,
			clock: started ? 'shared' : null,
			test: r.config.test.kind,
			mode: r.mode,
			levels_db: r.levels,
			level_db: on ? r.commanded : null,
			rung: on ? r.rung : null,
			frequency_hz: on ? round(r.freq, 2) : null,
			direction: on && r.config.test.kind === 'sweep' ? r.direction : null,
			sweeps: on ? sweepsOf(r) : null,
			held: r.held,
			elapsed_s: on ? round(r.testT, 2) : null,
			at_level_s: on ? round(r.atLevelT, 2) : null,
			remaining_s: on && total !== null && r.mode === 'auto' ? round(Math.max(0, total - r.testT), 1) : null,
			alarm: r.alarm,
			notch: r.notch,
			identification: r.identification,
			law: { id: r.law.id, name: r.law.name, version: r.law.version, sha256: r.law.sha256, misses: r.misses, capped_ticks: r.capped, live: { ...r.lawLive } },
			feasibility: r.feasibility,
			end: r.end
		};
	}

	function stateChanged(force = false) {
		const s = stateOf(run);
		const key = JSON.stringify([s.run_id, s.state, s.phase, s.mode, s.level_db, s.held, s.alarm, s.notch, s.end]);
		if (!force && key === lastStateKey) return;
		lastStateKey = key;
		host.broadcast('sine.state', s as unknown as Record<string, unknown>);
	}

	// ---- checking a configuration ---------------------------------------------------------------

	function lawOf(c: SineConfig): LawEntry | null {
		return laws().find((l) => l.id === (c.control?.law?.id ?? 'servo_blend')) ?? null;
	}

	function check(config: SineConfig): { errors: Item[]; warnings: Item[]; derived: SineDerived | null; views: SpectrumView[] } {
		const errors: Item[] = [];
		const warnings: Item[] = [];
		const fs = config.acquisition?.fs_hz ?? FS_DEFAULT;
		if (config.acquisition?.fs_hz != null && !(fs >= 1000 && fs <= 204800)) errors.push(item('out_of_range', 'The rate is from 1000 to 204800 Hz.', 'config.acquisition.fs_hz'));
		checkProfile(config.profile, 'config.profile', errors, warnings);
		const t = config.test;
		const band = bandOf(t);
		if (t.kind === 'dwell') {
			if (!(t.freq_hz > 0)) errors.push(item('out_of_range', 'The dwell frequency is above 0 Hz.', 'config.test.freq_hz'));
			const e = t.end as Record<string, unknown>;
			if (!(isNum(e.seconds) ? e.seconds > 0 : isNum(e.cycles) ? e.cycles > 0 : false)) errors.push(item('out_of_range', 'A dwell lasts some seconds or some cycles, above 0.', 'config.test.end'));
		} else {
			if (!(band[0] > 0)) errors.push(item('out_of_range', 'The band starts above 0 Hz.', 'config.test.band_hz[0]'));
			if (!(band[1] > band[0])) errors.push(item('out_of_range', 'The band ends above where it starts.', 'config.test.band_hz[1]'));
		}
		if (band[1] >= 0.45 * fs) errors.push(item('band_above_grid', `The band ends under 0.45 times the rate, ${round(0.45 * fs, 1)} Hz.`, t.kind === 'dwell' ? 'config.test.freq_hz' : 'config.test.band_hz[1]'));
		if (t.kind === 'sweep') {
			const r = t.rate;
			if (!(r.law === 'log' ? r.oct_per_min > 0 : r.hz_per_s > 0)) errors.push(item('out_of_range', 'The sweep rate is above 0.', 'config.test.rate'));
			if (t.start_hz != null && !(t.start_hz >= band[0] && t.start_hz <= band[1])) errors.push(item('out_of_range', 'The sweep starts inside its band.', 'config.test.start_hz'));
			const sw = t.end.sweeps ?? null;
			const dur = t.end.duration_s ?? null;
			if (sw === null && dur === null) errors.push(item('out_of_range', 'A sweep ends after some sweeps, some seconds, or both.', 'config.test.end'));
			if (sw !== null && !(Number.isInteger(sw) && sw >= 1)) errors.push(item('out_of_range', 'The sweeps are a whole number from 1.', 'config.test.end.sweeps'));
			if (dur !== null && !(dur > 0)) errors.push(item('out_of_range', 'The test time is above 0 s.', 'config.test.end.duration_s'));
		}
		if (t.kind === 'step' && !(t.steps_per_octave > 0)) errors.push(item('out_of_range', 'Steps an octave are above 0.', 'config.test.steps_per_octave'));
		if (!errors.some((e) => e.field?.startsWith('config.profile') || e.field?.startsWith('config.test'))) {
			const rows = config.profile.rows;
			if (rows[0].freq_hz > band[0] + 1e-9 || rows[rows.length - 1].freq_hz < band[1] - 1e-9) errors.push(item('profile_not_covering', `The profile covers ${rows[0].freq_hz} to ${rows[rows.length - 1].freq_hz} Hz; the test runs from ${band[0]} to ${band[1]} Hz.`, 'config.profile'));
		}
		const law = lawOf(config);
		const tick = config.acquisition?.tick_ms ?? 10;
		const floor = law && !law.builtin ? 8 : 2;
		if (!(tick >= floor && tick <= 1000)) errors.push(item('tick_out_of_range', `The control tick is from ${floor} to 1000 ms for ${law && !law.builtin ? 'a law file' : 'a built-in law'}.`, 'config.acquisition.tick_ms'));
		const s = config.schedule ?? {};
		const levels = s.levels_db ?? [-12, -9, -6, -3, 0];
		if (!levels.length) errors.push(item('levels_empty', 'The schedule has no levels.', 'config.schedule.levels_db'));
		for (let i = 1; i < levels.length; i++) if (!(levels[i] > levels[i - 1])) errors.push(item('levels_not_ascending', 'Levels rise from one to the next.', `config.schedule.levels_db[${i}]`));
		const start = s.start_db ?? levels[0];
		if (levels.length && !levels.some((l) => Math.abs(l - start) < 1e-9)) errors.push(item('start_not_a_level', `${start} dB is not a level of the schedule.`, 'config.schedule.start_db'));
		for (const k of ['per_level', 'full_level'] as const) {
			const v = s[k] as Record<string, unknown> | undefined;
			if (!v) continue;
			if ('sweeps' in v && t.kind !== 'sweep') errors.push(item('out_of_range', 'A level held for sweeps needs a sweep test; give seconds.', `config.schedule.${k}`));
			else if (!(isNum(v.sweeps) ? Number.isInteger(v.sweeps) && v.sweeps >= 1 : isNum(v.seconds) && v.seconds > 0)) errors.push(item('out_of_range', 'A level is held for a whole number of sweeps from 1, or some seconds above 0.', `config.schedule.${k}`));
		}
		if (t.kind === 'sweep' && (s.mode ?? 'auto') === 'auto' && t.end.sweeps && levels.length) {
			const per = s.per_level ?? { sweeps: 1 };
			const full = s.full_level ?? { sweeps: 1 };
			const from = Math.max(0, levels.findIndex((l) => Math.abs(l - start) < 1e-9));
			if ('sweeps' in per && 'sweeps' in full) {
				const need = per.sweeps * (levels.length - 1 - from) + full.sweeps;
				if (t.end.sweeps < need) errors.push(item('end_before_full_level', `The test ends after ${t.end.sweeps} sweeps; the schedule needs ${need} to hold full level.`, 'config.test.end.sweeps'));
			}
		}
		const id = config.identification ?? {};
		if (id.level_db !== undefined && !(id.level_db >= -60 && id.level_db <= 0)) errors.push(item('out_of_range', 'The noise burst is from -60 to 0 dB.', 'config.identification.level_db'));
		for (const k of ['gate_coherence', 'trust_coherence', 'trust_h_floor_frac'] as const) {
			const v = id[k];
			if (v !== undefined && !(v >= 0 && v <= 1)) errors.push(item('out_of_range', 'From 0 to 1.', `config.identification.${k}`));
		}
		const c = config.control ?? {};
		const ctl = new Set(controlRows().map((r) => r.index));
		(c.weights ?? []).forEach((w, i) => {
			if (!ctl.has(w.channel)) errors.push(item('weight_unknown_channel', `Channel ${w.channel} is not an enabled control row.`, `config.control.weights[${i}].channel`));
			else if (!(w.weight > 0)) errors.push(item('out_of_range', 'A weight is above 0.', `config.control.weights[${i}].weight`));
		});
		const tr = c.tracking;
		if (tr && !(tr.kind === 'fixed' ? tr.hz > 0 : tr.pct > 0 && tr.pct <= 100 && tr.max_hz > 0)) errors.push(item('out_of_range', 'The tracking bandwidth is above 0 Hz, or a percentage up to 100 with a cap above 0 Hz.', 'config.control.tracking'));
		const ranges: [keyof typeof c, number, number, boolean, string][] = [
			['law_budget', 0, 1, false, 'above 0 and at most 1'],
			['law_misses', 1, 100, true, 'a whole number from 1 to 100'],
			['max_step_db', 1, 3, false, 'from 1 to 3 dB'],
			['max_rise_db_per_s', 0, 10000, false, 'above 0 and at most 10000 dB/s'],
			['divergence_s', 0, 60, false, 'above 0 and at most 60 s'],
			['fault_debounce_ticks', 0, 1e6, true, 'a whole number from 0']
		];
		for (const [k, lo, hi, whole, words] of ranges) {
			const v = c[k] as number | undefined;
			if (v === undefined) continue;
			const open = k === 'law_budget' || k === 'max_rise_db_per_s' || k === 'divergence_s';
			if (!(isNum(v) && (open ? v > lo : v >= lo) && v <= hi && (!whole || Number.isInteger(v)))) errors.push(item('out_of_range', `It is ${words}.`, `config.control.${k}`));
		}
		if (!law) errors.push(item('law_unknown', `No control law "${c.law?.id}" in the catalog.`, 'config.control.law.id'));
		else if (law.world !== 'sine') errors.push(item('law_wrong_world', `${law.name} is not a Sine law.`, 'config.control.law.id'));
		else
			for (const [k, v] of Object.entries(c.law?.params ?? {})) {
				const decl = law.params.find((x) => x.key === k);
				if (!decl) errors.push(item('law_param_unknown', `The law declares no parameter "${k}".`, `config.control.law.params.${k}`));
				else if (numberParam(decl) && !(isNum(v) && v >= decl.min && v <= decl.max)) errors.push(item('law_param_invalid', `${decl.label} is from ${decl.min} to ${decl.max}.`, `config.control.law.params.${k}`));
				else if (decl.kind === 'choice' && !decl.options.includes(String(v))) errors.push(item('law_param_invalid', `${decl.label} is one of ${decl.options.join(', ')}.`, `config.control.law.params.${k}`));
			}
		const lim = config.limits;
		if (!(isNum(lim.drive_vpk_ceiling) && lim.drive_vpk_ceiling > 0 && lim.drive_vpk_ceiling <= 10)) errors.push(item('out_of_range', 'The drive ceiling is above 0 and at most 10 V pk.', 'config.limits.drive_vpk_ceiling'));
		if (!(isNum(lim.drive_vpk_abort) && lim.drive_vpk_abort <= 10)) errors.push(item('out_of_range', 'The drive abort is at most 10 V pk.', 'config.limits.drive_vpk_abort'));
		else if (lim.drive_vpk_abort < lim.drive_vpk_ceiling) errors.push(item('abort_below_ceiling', 'The drive abort is at least the drive ceiling.', 'config.limits.drive_vpk_abort'));
		const ins = new Set(inputs().map((r) => r.index));
		(lim.channels ?? []).forEach((ch, i) => {
			if (ctl.has(ch.channel)) errors.push(item('limit_on_control_channel', `Channel ${ch.channel} is a control row; a limit channel is another input.`, `config.limits.channels[${i}].channel`));
			else if (!ins.has(ch.channel)) errors.push(item('limit_unknown_channel', `Channel ${ch.channel} is not an enabled input of the channel table.`, `config.limits.channels[${i}].channel`));
			checkProfile(ch.profile, `config.limits.channels[${i}].profile`, errors, warnings);
		});
		const n = lim.notch;
		if (n?.drive_floor !== undefined && !(n.drive_floor > 0 && n.drive_floor <= 1)) errors.push(item('out_of_range', 'The drive floor is above 0 and at most 1.', 'config.limits.notch.drive_floor'));
		if (n?.smoothing_ticks !== undefined && !(Number.isInteger(n.smoothing_ticks) && n.smoothing_ticks >= 1)) errors.push(item('out_of_range', 'The smoothing is a whole number of ticks from 1.', 'config.limits.notch.smoothing_ticks'));
		if (n?.release_ramp_db_per_s !== undefined && !(n.release_ramp_db_per_s > 0)) errors.push(item('out_of_range', 'The release is above 0 dB/s.', 'config.limits.notch.release_ramp_db_per_s'));
		if (config.cola?.enabled) {
			const drives = host.rows().filter((r) => r.enabled && r.role === 'drive').length;
			if (host.station().ref.kind === 'sim' || drives < 2) errors.push(item('cola_needs_two_outputs', 'The reference output needs a second drive row and a station with two outputs; the simulator has one.', 'config.cola.enabled'));
			if (config.cola.volts_pk !== undefined && !(config.cola.volts_pk > 0)) errors.push(item('out_of_range', 'The reference output is above 0 V pk.', 'config.cola.volts_pk'));
		}
		const cap = config.capture ?? {};
		if (cap.rule === 'first_seconds' && !(isNum(cap.first_s) && cap.first_s > 0)) errors.push(item('out_of_range', 'The seconds captured at each level are above 0.', 'config.capture.first_s'));
		if (cap.periodic_save_s != null && !(cap.periodic_save_s > 0)) errors.push(item('out_of_range', 'A periodic save is above 0 s, or none.', 'config.capture.periodic_save_s'));
		if (cap.run_name !== undefined && /[\\/]/.test(cap.run_name)) errors.push(item('out_of_range', 'The run folder is a name, not a path.', 'config.capture.run_name'));
		if (!controlRows().length) errors.push({ reason: 'no_control_channel', message: 'The channel table has no enabled control row.' } as Item);
		if (!host.rows().some((r) => r.enabled && r.role === 'drive')) errors.push({ reason: 'no_drive_channel', message: 'The channel table has no enabled drive row.' } as Item);
		const tracking = trackingOf(config);
		if (tracking.verdict === 'refused') errors.push(item('tracking_lag', tracking.message ?? '', 'config.control.tracking'));
		else if (tracking.verdict === 'advisory') warnings.push(item('tracking_lag', tracking.message ?? '', 'config.control.tracking'));
		if (errors.length || !law) return { errors, warnings, derived: null, views: [] };
		const frames = Math.max(1, Math.round((tick / 1000) * fs));
		const tickS = frames / fs;
		const ceiling = lim.drive_vpk_ceiling;
		const derived: SineDerived = {
			band_hz: band,
			grid: { fs_hz: fs, tick_frames: frames, tick_s: round(tickS, 7), actual: false },
			profile: { crossovers: crossovers(config.profile), ...peaksOver(config.profile, band) },
			test_s: testSeconds(config),
			identification_s: round((8 * Math.max(16384, 2 ** Math.ceil(Math.log2(fs / (Math.max(band[0], 1) / 200))))) / fs, 1),
			call_budget_s: round((c.law_budget ?? 0.25) * tickS, 7),
			stop_ramp_db_per_s: STOP_DB_PER_S,
			safe_stop_s: round((db20(ceiling) + 120) / STOP_DB_PER_S, 3),
			tracking,
			law: { id: law.id, name: law.name, version: law.version, sha256: law.sha256, live: [...(law.live ?? [])] },
			feasibility: config.feasibility
				? host.feasibilityExceeds
					? { verdict: 'exceeded', message: "The profile's displacement exceeds the shaker's stroke." }
					: { verdict: 'pass', message: "Within the shaker's ratings." }
				: { verdict: 'not_evaluated', message: 'No shaker was given, so feasibility was not evaluated.' }
		};
		return { errors, warnings, derived, views: profileViews(config.profile, band) };
	}

	// ---- running ----------------------------------------------------------------------------

	function newTrace(grid: number[], dwell: boolean): Trace {
		const n = dwell ? 0 : grid.length;
		const nan = () => Array.from({ length: n }, () => NaN);
		const amp = new Map<number, number[]>();
		const phase = new Map<number, number[]>();
		for (const r of inputs()) {
			amp.set(r.index, nan());
			phase.set(r.index, nan());
		}
		return { x: dwell ? [] : [...grid], measured: nan(), drive: nan(), thd: nan(), amp, phase };
	}

	function newRun(config: SineConfig, derived: SineDerived, views: SpectrumView[]): Run {
		const st = host.station();
		const law = lawOf(config) ?? SERVO_BLEND;
		const levels = [...(config.schedule?.levels_db ?? [-12, -9, -6, -3, 0])];
		const start = config.schedule?.start_db ?? levels[0];
		const rung = Math.max(0, levels.findIndex((l) => Math.abs(l - start) < 1e-9));
		const t = config.test;
		const band = bandOf(t);
		const grid = t.kind === 'dwell' || t.kind === 'rstd' ? [] : logGrid(band[0], band[1]);
		const lawLive: Record<string, number> = {};
		for (const k of law.live ?? []) {
			const p = law.params.find((x) => x.key === k);
			const set = (config.control?.law?.params ?? {})[k];
			if (p && numberParam(p)) lawLive[k] = isNum(set) ? set : p.default;
		}
		return {
			id: `sine-${String(++runCount).padStart(4, '0')}`,
			config: structuredClone(config),
			derived,
			views,
			law,
			lawLive,
			state: 'configured',
			phase: 'idle',
			station: st.ref,
			source: st.source,
			mode: config.schedule?.mode ?? 'auto',
			levels,
			rung,
			commanded: levels[rung],
			level: levels[rung] - 6,
			freq: t.kind === 'dwell' ? t.freq_hz : t.kind === 'sweep' ? (t.start_hz ?? band[0]) : band[0],
			direction: t.kind === 'sweep' ? (t.direction ?? 'up') : 'up',
			held: false,
			sweepsAtLevel: 0,
			sweepsTotal: 0,
			t: 0,
			testT: 0,
			atLevelT: 0,
			phaseT: 0,
			step: 0,
			stepT: 0,
			stopOutcome: 'operator_stop',
			stopBy: undefined,
			pendingTrip: null,
			rampT: 0,
			drive: 0,
			measured: 0,
			target: 0,
			notch: false,
			alarm: false,
			alarms: [],
			overloaded: new Set(),
			misses: 0,
			capped: 0,
			values: {},
			identification: null,
			feasibility: null,
			end: null,
			timer: null,
			trace: newTrace(grid, t.kind === 'dwell' || t.kind === 'rstd'),
			grid
		};
	}

	function setPhase(r: Run, state: SineRunState, phase: SinePhase) {
		r.state = state;
		r.phase = phase;
		r.phaseT = 0;
		stateChanged();
	}

	function publish(r: Run) {
		host.broadcast('sine.live', liveOf(r) as unknown as Record<string, unknown>);
		for (const [conn, sub] of subs) {
			sub.acc += LIVE_MS / 1000;
			if (sub.acc + 1e-9 >= 1 / sub.rate) {
				sub.acc = 0;
				conn.emit('sine.data', dataFor(r, sub.plots) as unknown as Record<string, unknown>);
			}
		}
	}

	function startLive() {
		if (liveTimer) return;
		liveTimer = setInterval(() => {
			if (!run || !LIVE.includes(run.state)) {
				if (liveTimer) clearInterval(liveTimer);
				liveTimer = null;
				return;
			}
			if (run.state === 'armed') return;
			publish(run);
		}, LIVE_MS);
	}

	function arm(r: Run, ack: string | undefined) {
		if (r.config.feasibility && host.feasibilityExceeds && !ack) {
			r.feasibility = { verdict: 'exceeded', message: "The profile's displacement exceeds the shaker's stroke." };
			setPhase(r, 'blocked', 'feasibility_blocked');
			addLog('feasibility', 'feasibility exceeded; arming needs an acknowledgement');
			refuse('refused', 'feasibility_exceeded', "The profile's displacement exceeds the shaker's stroke; arm with an acknowledgement to go on.", { verdict: 'exceeded' });
		}
		if (ack) addLog('override', `armed past feasibility: "${ack}"`);
		r.feasibility = r.derived.feasibility;
		r.derived = { ...r.derived, grid: { ...r.derived.grid, actual: true } };
		setPhase(r, 'armed', 'armed');
		addLog('station', `armed on ${r.station.class}; nothing driven`);
		startLive();
	}

	function start(r: Run) {
		setPhase(r, 'running', 'sysid');
		addLog('state', `identification: a noise burst at ${r.config.identification?.level_db ?? -20} dB`);
		if (r.timer) clearInterval(r.timer);
		r.timer = setInterval(() => tick(r), TICK_MS);
	}

	function finishIdentification(r: Run) {
		const id = r.config.identification ?? {};
		r.identification = { level_db: id.level_db ?? -20, band_coherence: 0.98, gate_coherence: id.gate_coherence ?? 0.5, lag_frames: 0, trusted_frac: 0.97, passed: 0.98 >= (id.gate_coherence ?? 0.5) };
		addLog('gate', 'identification done: band coherence 0.98');
		if (!r.identification.passed) {
			end(r, 'held', 'coherence_hold', null);
			return;
		}
		if (id.review ?? true) {
			setPhase(r, 'review', 'awaiting_review');
			addLog('state', 'holding for review');
		} else engage(r);
	}

	function engage(r: Run) {
		addLog('law', `built ${r.law.name} ${r.law.version}; engaged at ${r.commanded} dB`);
		r.t = 0;
		r.testT = 0;
		r.atLevelT = 0;
		setPhase(r, 'running', 'engaged');
	}

	function endMessage(outcome: SineOutcome, trip: SineTrip | null, by?: string): string {
		switch (outcome) {
			case 'completed':
				return 'The schedule finished.';
			case 'operator_stop':
				return `Stopped by ${by ?? 'the operator'}.`;
			case 'operator_abort':
				return `Aborted by ${by ?? 'the operator'}.`;
			case 'safe_stop':
				return `Nobody held control for ${host.timeoutS} s, so the run stopped with its ramp.`;
			case 'held':
				return 'The band coherence stayed under the gate, so the run never drove at test level.';
			case 'law_fault':
				return trip?.kind === 'law_fault' ? `The control law gave no usable drive ${trip.consecutive} times in a row; the run took its controlled stop.` : 'The control law gave no usable drive.';
			case 'aborted':
				return trip ? `A protection tripped: ${trip.kind.replaceAll('_', ' ')}.` : 'A protection tripped.';
			default:
				return outcome.replaceAll('_', ' ');
		}
	}

	function end(r: Run, outcome: SineOutcome, phase: SinePhase, trip: SineTrip | null, by?: string, ramp_s: number | null = null) {
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		const on = engaged(r) || r.phase === 'ramping_down';
		r.end = { outcome, message: endMessage(outcome, trip, by), at_s: on ? round(r.t, 2) : null, level_db: on ? r.commanded : null, at_hz: on ? round(r.freq, 2) : null, ramp_s, trip };
		if (phase !== 'ramping_down') r.drive = 0;
		setPhase(r, 'finished', phase);
		addLog(outcome === 'aborted' ? 'trip' : 'state', `finished: ${outcome.replaceAll('_', ' ')}`);
		publish(r);
	}

	/** The controlled stop: the drive ramps down at 120 dB a second, then the run ends. */
	function stopRun(r: Run, outcome: SineOutcome, by?: string) {
		if (!DRIVING.includes(r.phase)) {
			end(r, outcome, 'stopped', r.pendingTrip, by);
			return;
		}
		r.stopOutcome = outcome;
		r.stopBy = by;
		r.rampT = 0;
		setPhase(r, 'stopping', 'ramping_down');
	}

	function judge(r: Run) {
		const t = r.config.test;
		const scale = 10 ** (r.level / 20);
		const pt = profileAt(r.config.profile, r.freq);
		r.target = pt.accel_g * 10 ** (r.commanded / 20);
		// the loop is on its target, a little off where the plant moves fast under the tone
		const fast = t.kind === 'sweep' ? Math.abs(Math.log(plantGain(r.freq * 1.01) / plantGain(r.freq))) * 30 : 0;
		const w = r.lawLive.blend_w ?? 0.5;
		let err = 0.12 * Math.sin(r.t * 1.7) + Math.min(0.6, fast) * (1.2 - w) * (r.direction === 'up' ? 1 : -1);
		let measured = pt.accel_g * scale * 10 ** (err / 20);
		let drive = measured / plantGain(r.freq);
		const ceiling = r.config.limits.drive_vpk_ceiling;
		if (drive > ceiling) {
			drive = ceiling;
			measured = drive * plantGain(r.freq);
		}
		r.notch = false;
		for (const ch of r.config.limits.channels ?? []) {
			const amp = measured * 3 * mode(r.freq, 550, 4) * 0.35;
			const bound = profileAt(ch.profile, r.freq).accel_g;
			if (amp > bound) {
				const k = bound / amp;
				measured *= k;
				drive *= k;
				r.notch = true;
			}
		}
		err = db20(measured / r.target);
		r.measured = measured;
		r.drive = drive;
		const alarms: SineAlarm[] = [];
		if (err > pt.alarm[1]) alarms.push({ kind: 'band', side: 'high', error_db: round(err, 2) });
		else if (err < -pt.alarm[0]) alarms.push({ kind: 'band', side: 'low', error_db: round(err, 2) });
		// a control row's overload latches once it reaches 95 % of the abort band's top
		if (measured >= 0.95 * pt.accel_g * scale * 10 ** (pt.abort[1] / 20)) for (const row of controlRows()) r.overloaded.add(row.index);
		for (const channel of r.overloaded) alarms.push({ kind: 'overload', channel });
		r.alarms = alarms;
		r.alarm = alarms.length > 0;
		const step = round(err * -0.3, 3);
		r.values = r.law.builtin ? { servo_step_db: round(step * (1 - w), 3), prediction_step_db: round(step * w, 3), total_step_db: step, servo_level_db: round(db20(Math.max(drive, 1e-9)), 3) } : r.law.diagnostics.length ? { error_db: round(err, 3) } : {};
		if (drive >= r.config.limits.drive_vpk_abort) end(r, 'aborted', 'aborted', { kind: 'drive_vpk', at_hz: round(r.freq, 2), at_level_db: r.level, at_tick: Math.round(r.t / r.derived.grid.tick_s), vpk: round(drive, 4), abort_vpk: r.config.limits.drive_vpk_abort });
	}

	function record(r: Run) {
		const tr = r.trace;
		const ch = (index: number) => {
			const row = host.rows().find((x) => x.index === index);
			const limit = (r.config.limits.channels ?? []).some((c) => c.channel === index);
			const k = row?.role === 'control' ? 1 : limit ? 3 * mode(r.freq, 550, 4) * 0.35 : 0.8 * mode(r.freq, 200, 3);
			const ph = plantPhase(r.freq) - (limit ? 20 : 0);
			return { amp: r.measured * k, phase: ((ph + 540) % 360) - 180 };
		};
		if (dwellAxis(r)) {
			// against station time, the last minute, at most 1200 points
			tr.x.push(r.t);
			tr.measured.push(r.measured);
			tr.drive.push(r.drive);
			tr.thd.push(0.4 + 0.05 * Math.sin(r.t));
			for (const [index, list] of tr.amp) list.push(ch(index).amp);
			for (const [index, list] of tr.phase) list.push(ch(index).phase);
			while (tr.x.length && (tr.x[0] < r.t - DWELL_WINDOW_S || tr.x.length > 1200)) {
				tr.x.shift();
				tr.measured.shift();
				tr.drive.shift();
				tr.thd.shift();
				for (const list of tr.amp.values()) list.shift();
				for (const list of tr.phase.values()) list.shift();
			}
			return;
		}
		const lo = r.grid[0];
		const hi = r.grid[r.grid.length - 1];
		const i = Math.max(0, Math.min(r.grid.length - 1, Math.round(((Math.log(r.freq / lo) / Math.log(hi / lo)) * (r.grid.length - 1)) || 0)));
		tr.measured[i] = r.measured;
		tr.drive[i] = r.drive;
		tr.thd[i] = 0.4 + (0.3 * mode(r.freq, 200, 2.5)) / 3;
		for (const [index, list] of tr.amp) list[i] = ch(index).amp;
		for (const [index, list] of tr.phase) list[i] = ch(index).phase;
	}

	/** Move the ladder on in auto, once this level's sweeps or seconds are done. */
	function ladder(r: Run) {
		if (r.mode !== 'auto' || r.rung >= r.levels.length - 1) return;
		const per = r.config.schedule?.per_level ?? { sweeps: 1 };
		const done = 'sweeps' in per ? r.sweepsAtLevel >= per.sweeps : r.atLevelT >= per.seconds;
		if (!done) return;
		r.rung += 1;
		r.commanded = r.levels[r.rung];
		r.sweepsAtLevel = 0;
		r.atLevelT = 0;
		addLog('level', `level ${r.commanded} dB`);
		stateChanged();
	}

	/** Full level once the level in force reaches the table's last entry, in manual as in auto. */
	function phaseOfLevel(r: Run) {
		if (!DRIVING.includes(r.phase)) return;
		const want: SinePhase = Math.abs(r.level - r.levels[r.levels.length - 1]) < 0.05 ? 'full_level' : 'engaged';
		if (r.phase !== want) {
			r.phase = want;
			stateChanged();
		}
	}

	function advance(r: Run, dt: number) {
		const t = r.config.test;
		if (t.kind === 'sweep') {
			const [lo, hi] = t.band_hz;
			const sign = r.direction === 'up' ? 1 : -1;
			if (t.rate.law === 'log') r.freq *= 2 ** ((sign * t.rate.oct_per_min * dt) / 60);
			else r.freq += sign * t.rate.hz_per_s * dt;
			if (r.freq >= hi || r.freq <= lo) {
				r.freq = Math.min(hi, Math.max(lo, r.freq));
				r.direction = r.direction === 'up' ? 'down' : 'up';
				r.sweepsAtLevel += 1;
				r.sweepsTotal += 1;
				addLog('note', `sweep ${r.sweepsTotal} done at ${r.commanded} dB; now ${r.direction}`);
				stateChanged(true);
			}
			const bySweeps = t.end.sweeps ? r.sweepsTotal >= t.end.sweeps : false;
			const byTime = t.end.duration_s ? r.testT >= t.end.duration_s : false;
			return bySweeps || byTime;
		}
		if (t.kind === 'dwell') return 'seconds' in t.end ? r.testT >= t.end.seconds : r.testT * t.freq_hz >= t.end.cycles;
		if (t.kind === 'step') {
			const steps = stepFreqs(t);
			r.stepT += dt;
			if (r.stepT >= dwellSecondsAt(t.dwell, steps[r.step])) {
				r.step += 1;
				r.stepT = 0;
				if (r.step >= steps.length) return true;
				r.freq = steps[r.step];
			}
			return false;
		}
		return r.testT >= 30;
	}

	function tick(r: Run) {
		if (run !== r || r.state === 'finished') return;
		const dt = (TICK_MS / 1000) * host.speed;
		r.phaseT += dt;
		switch (r.phase) {
			case 'sysid':
				if (r.phaseT >= SYSID_S) setPhase(r, 'running', 'coherence_gate');
				return;
			case 'coherence_gate':
				if (r.phaseT >= GATE_S) finishIdentification(r);
				return;
			case 'ramping_down': {
				r.t += dt;
				r.rampT += dt;
				r.level -= STOP_DB_PER_S * dt;
				r.drive *= 10 ** ((-STOP_DB_PER_S * dt) / 20);
				r.measured = r.drive * plantGain(r.freq);
				if (r.drive <= 1e-6) end(r, r.stopOutcome, 'stopped', r.pendingTrip, r.stopBy, round(r.rampT, 3));
				return;
			}
			case 'engaged':
			case 'full_level':
				break;
			default:
				return;
		}
		r.t += dt;
		// a law file that answers late on purpose: misses in a row stop the run with the ramp
		if (r.law.name === 'Sine misbehave' && r.t >= Number((r.config.control?.law?.params ?? {}).after_s ?? 2)) {
			r.misses += 1;
			addLog('law', `call ${Math.round(r.t / r.derived.grid.tick_s)} over its budget; the drive in force is held`);
			stateChanged(true);
			if (r.misses >= (r.config.control?.law_misses ?? 3)) {
				r.pendingTrip = { kind: 'law_fault', at_hz: round(r.freq, 2), at_level_db: round(r.level, 2), at_tick: Math.round(r.t / r.derived.grid.tick_s), reason: 'over_budget', consecutive: r.misses };
				stopRun(r, 'law_fault');
			}
			return;
		}
		// the drive comes onto a new level at 20 dB a second
		const gap = r.commanded - r.level;
		r.level += Math.sign(gap) * Math.min(Math.abs(gap), 20 * dt);
		phaseOfLevel(r);
		if (!r.held) {
			r.testT += dt;
			r.atLevelT += dt;
			if (advance(r, dt)) {
				judge(r);
				stopRun(r, 'completed');
				return;
			}
			ladder(r);
		}
		judge(r);
		if (run === r && r.state === 'running') record(r);
	}

	// ---- display ------------------------------------------------------------------------------

	const unitOf = (index: number) => host.rows().find((x) => x.index === index)?.units ?? 'g';
	const controlUnit = () => controlRows()[0]?.units ?? 'g';
	/** `Channel 3: Fixture`, or the role without a description. */
	const titleOf = (row: ChannelRow) => `Channel ${row.index}: ${row.description.trim() || row.role}`;
	const xAxis = (r: Run) => (dwellAxis(r) ? { label: 'Time', unit: 's', scale: 'linear' as const } : freqAxis);
	const link = (r: Run) => (dwellAxis(r) ? 'time' : 'frequency');
	/** The x the run has reached, and a series' y on it: no point the run has not reached. */
	function reached(r: Run, ys: number[]): { x: Float64Array; y: Float64Array } {
		const xs = dwellAxis(r) ? r.trace.x : r.grid;
		const keep = ys.map((v, i) => i).filter((i) => Number.isFinite(ys[i]));
		return { x: f64(keep.map((i) => xs[i])), y: f64(keep.map((i) => ys[i])) };
	}

	function referenceSeries(r: Run): Series[] {
		const xs = dwellAxis(r) ? [r.trace.x[0] ?? 0, r.trace.x.at(-1) ?? 0] : r.grid;
		const x = f64(xs);
		const scale = 10 ** (r.commanded / 20);
		const pts = dwellAxis(r) ? xs.map(() => profileAt(r.config.profile, r.freq)) : r.grid.map((f) => profileAt(r.config.profile, f));
		const band = (sign: number, side: 'alarm' | 'abort') => f64(pts.map((p) => p.accel_g * scale * 10 ** ((sign * (side === 'alarm' ? p.alarm : p.abort)[sign > 0 ? 1 : 0]) / 20)));
		return [
			{ name: 'Profile', role: 'reference', x, y: f64(pts.map((p) => p.accel_g * scale)) },
			{ name: 'Alarm high', role: 'tolerance', x, y: band(1, 'alarm') },
			{ name: 'Alarm low', role: 'tolerance', x, y: band(-1, 'alarm') },
			{ name: 'Abort high', role: 'abort', x, y: band(1, 'abort') },
			{ name: 'Abort low', role: 'abort', x, y: band(-1, 'abort') }
		];
	}

	function controlView(r: Run): SpectrumView {
		return { view: 'spectrum', title: 'Control', x: xAxis(r), y: { label: 'Amplitude', unit: controlUnit(), scale: 'log' }, series: [{ name: 'Control', role: 'measured', ...reached(r, r.trace.measured) }, ...referenceSeries(r)], link: link(r) };
	}

	function driveView(r: Run): SpectrumView {
		const series: Series[] = [{ name: 'Drive', role: 'drive', ...reached(r, r.trace.drive) }];
		if (r.config.cola?.enabled) {
			const d = reached(r, r.trace.drive);
			series.push({ name: 'Reference output', role: 'other', x: d.x, y: f64([...d.x].map(() => r.config.cola?.volts_pk ?? 0.1)) });
		}
		return { view: 'spectrum', title: 'Drive', x: xAxis(r), y: { label: 'Drive', unit: 'V', scale: 'log' }, series, link: link(r) };
	}

	function readings(r: Run): View {
		const s = stateOf(r);
		const err = r.target > 0 && r.measured > 0 ? round(db20(r.measured / r.target), 3) : null;
		const inBand = !r.alarm;
		const rows: ReadingRow[] = [{ label: 'Frequency', value: round(r.freq, 2), unit: 'Hz', status: 'info' }];
		if (r.config.test.kind === 'sweep') rows.push({ label: 'Direction', value: r.direction, unit: null, status: 'info' });
		rows.push(
			{ label: 'Level', value: round(r.level, 2), unit: 'dB', status: 'info' },
			{ label: 'Target', value: round(r.target, 4), unit: controlUnit(), status: 'info' },
			{ label: 'Measured', value: round(r.measured, 4), unit: controlUnit(), status: inBand ? 'pass' : 'warn' },
			{ label: 'Error', value: err ?? '-', unit: 'dB', status: inBand ? 'pass' : 'warn' },
			{ label: 'Drive', value: round(r.drive, 4), unit: 'V', status: 'info' }
		);
		if (s.sweeps) rows.push({ label: 'Sweeps', value: s.sweeps.of !== null ? `${s.sweeps.done} of ${s.sweeps.of}` : String(s.sweeps.done), unit: null, status: 'info' });
		rows.push({ label: 'Elapsed', value: round(r.testT, 2), unit: 's', status: 'info' });
		if (s.remaining_s !== null) rows.push({ label: 'Remaining', value: s.remaining_s, unit: 's', status: 'info' });
		return { view: 'reading', title: 'Run', rows };
	}

	function lawReading(r: Run): View {
		const rows: ReadingRow[] = r.law.diagnostics.map((d) => ({ label: d.label, value: r.values[d.key] ?? '-', unit: d.unit || null, status: 'info' }));
		rows.push({ label: 'Unusable calls in a row', value: r.misses, unit: null, status: r.misses ? 'warn' : 'pass' }, { label: 'Ticks a cap bound', value: r.capped, unit: null, status: 'info' });
		return { view: 'reading', title: 'Law', rows };
	}

	function identificationViews(r: Run): View[] {
		const [lo, hi] = r.config.test.kind === 'dwell' ? [r.freq / 4, r.freq * 4] : bandOf(r.config.test);
		const f = logGrid(lo, hi, 160);
		const mag = f.map(plantGain);
		const top = Math.max(...mag);
		const trust = r.config.identification?.trust_coherence ?? 0.5;
		const coh = f.map((_, i) => Math.min(0.999, 0.96 + 0.03 * Math.sin(i / 9)));
		const x = f64(f);
		const ends = f64([lo, hi]);
		return [
			{ view: 'spectrum', title: 'Identification Gain', x: freqAxis, y: { label: 'Gain', unit: `${controlUnit()}/V`, scale: 'log' }, series: [{ name: 'Gain', role: 'measured', x, y: f64(mag) }, { name: 'Trust floor', role: 'tolerance', x: ends, y: f64([1, 1].map(() => top * (r.config.identification?.trust_h_floor_frac ?? 0.05))) }], link: 'frequency' },
			{
				view: 'spectrum',
				title: 'Coherence',
				x: freqAxis,
				y: { label: 'Coherence', unit: null, scale: 'linear' },
				series: [
					{ name: 'Coherence', role: 'measured', x, y: f64(coh) },
					{ name: 'Trusted', role: 'reference', x, y: f64(coh.map((c) => (c >= trust ? c : NaN))) },
					{ name: 'Trust coherence', role: 'tolerance', x: ends, y: f64([trust, trust]) }
				],
				link: 'frequency'
			}
		];
	}

	function resolve(r: Run, sel: SineSubscribePlot['channels']): ChannelRow[] {
		const ins = inputs();
		if (Array.isArray(sel)) return ins.filter((x) => sel.includes(x.index));
		if (sel === 'control') return ins.filter((x) => x.role === 'control');
		if (sel === 'limited') return ins.filter((x) => (r.config.limits.channels ?? []).some((c) => c.channel === x.index));
		return ins;
	}

	function responseViews(r: Run, rows: ChannelRow[], what: 'response' | 'phase'): View[] {
		return rows.map((row) => {
			const series: Series[] = [{ name: `Channel ${row.index}`, role: 'measured', ...reached(r, (what === 'response' ? r.trace.amp : r.trace.phase).get(row.index) ?? []) }];
			const limit = (r.config.limits.channels ?? []).find((c) => c.channel === row.index);
			if (limit && what === 'response') {
				const xs = dwellAxis(r) ? [r.trace.x[0] ?? 0, r.trace.x.at(-1) ?? 0] : r.grid;
				series.push({ name: 'Limit', role: 'limit', x: f64(xs), y: f64(xs.map((x) => profileAt(limit.profile, dwellAxis(r) ? r.freq : x).accel_g)) });
			}
			const y = what === 'response' ? { label: 'Amplitude', unit: unitOf(row.index), scale: 'log' as const } : { label: 'Phase', unit: 'deg', scale: 'linear' as const };
			return { view: 'spectrum', title: titleOf(row), x: xAxis(r), y, series, link: link(r) } as SpectrumView;
		});
	}

	function timeViews(r: Run, rows: ChannelRow[], points: number): View[] {
		const n = Math.min(points, 1024);
		const span = Math.max(3 / r.freq, 0.01);
		const t = Array.from({ length: n }, (_, i) => (i / (n - 1)) * span);
		return rows.map((row) => {
			const amp = lastFinite(r.trace.amp.get(row.index)) ?? r.measured;
			return { view: 'spectrum', title: titleOf(row), x: { label: 'Time', unit: 's', scale: 'linear' }, y: { label: 'Amplitude', unit: unitOf(row.index), scale: 'linear' }, series: [{ name: `Channel ${row.index}`, role: 'measured', x: f64(t), y: f64(t.map((s) => amp * Math.sin(2 * Math.PI * r.freq * s))) }] } as SpectrumView;
		});
	}

	function plotViews(r: Run, pl: SineSubscribePlot): View[] {
		const on = engaged(r);
		switch (pl.quantity) {
			case 'control':
				return on ? [controlView(r)] : [];
			case 'drive':
				return on ? [driveView(r)] : [];
			case 'response':
			case 'phase':
				return on ? responseViews(r, resolve(r, pl.channels ?? 'all'), pl.quantity) : [];
			case 'thd':
				return on ? [{ view: 'spectrum', title: 'Distortion', x: xAxis(r), y: { label: 'THD', unit: '%', scale: 'linear' }, series: [{ name: 'THD', role: 'measured', ...reached(r, r.trace.thd) }], link: link(r) }] : [];
			case 'time':
				return on ? timeViews(r, resolve(r, pl.channels ?? 'all'), pl.points ?? 1024) : [];
			case 'identification':
				return r.identification ? identificationViews(r) : [];
			case 'law':
				return on ? [lawReading(r)] : [];
			case 'readings':
				return on ? [readings(r)] : [];
		}
	}

	function head(r: Run) {
		const s = stateOf(r);
		const on = engaged(r);
		const t = r.config.test;
		const moving = on && !r.held && DRIVING.includes(r.phase) && t.kind === 'sweep';
		const sign = r.direction === 'up' ? 1 : -1;
		const channels: SineLiveChannel[] = inputs().map((row) => {
			const amp = lastFinite(r.trace.amp.get(row.index));
			const ph = lastFinite(r.trace.phase.get(row.index));
			return {
				index: row.index,
				description: row.description,
				role: row.role as SineLiveChannel['role'],
				amplitude: on ? round(amp ?? r.measured, 4) : null,
				phase_deg: on ? round(ph ?? 0, 1) : null,
				rms: on ? round((amp ?? r.measured) / Math.SQRT2, 4) : null,
				unit: row.units,
				overload: row.role === 'control' && r.overloaded.has(row.index)
			};
		});
		return {
			run_id: r.id,
			source: r.source,
			class: r.station.class,
			state: r.state as SineLive['state'],
			phase: r.phase,
			test: t.kind,
			mode: r.mode,
			t_s: on ? round(r.t, 3) : null,
			elapsed_s: s.elapsed_s,
			frequency_hz: s.frequency_hz,
			direction: s.direction,
			...(t.kind === 'sweep' && t.rate.law === 'linear'
				? { sweep_rate_hz_per_s: moving ? sign * t.rate.hz_per_s : 0 }
				: { sweep_rate_oct_per_min: moving && t.kind === 'sweep' && t.rate.law === 'log' ? sign * t.rate.oct_per_min : 0 }),
			sweeps: s.sweeps,
			held: r.held,
			level_db: on ? round(r.level, 2) : null,
			commanded_db: on ? r.commanded : null,
			target: on ? round(r.target, 5) : null,
			measured: on ? round(r.measured, 5) : null,
			error_db: on && r.target > 0 && r.measured > 0 ? round(db20(r.measured / r.target), 3) : null,
			drive_vpk: on ? round(r.drive, 5) : null,
			cola_vpk: r.config.cola?.enabled && on ? (r.config.cola.volts_pk ?? 0.1) : null,
			notch: r.notch,
			alarm: r.alarm,
			channels,
			alarms: r.alarms,
			law: on ? { values: { ...r.values }, misses: r.misses, capped_ticks: r.capped } : null
		};
	}

	function liveOf(r: Run): SineLive {
		const on = engaged(r);
		const empty = (title: string): SpectrumView => ({ view: 'spectrum', title, x: freqAxis, y: { label: title === 'Drive' ? 'Drive' : 'Amplitude', unit: title === 'Drive' ? 'V' : controlUnit(), scale: 'log' }, series: [], link: 'frequency' });
		return { ...head(r), views: [on ? controlView(r) : empty('Control'), on ? driveView(r) : empty('Drive'), readings(r) as SineLive['views'][2]] };
	}

	function dataFor(r: Run, plots: SineSubscribePlot[]) {
		const resolved = (pl: SineSubscribePlot) => (WITH_CHANNELS.includes(pl.quantity) ? resolve(r, pl.channels ?? 'all').map((x) => x.index) : null);
		return { ...head(r), plots: plots.map((pl) => ({ id: pl.id, quantity: pl.quantity, channels: resolved(pl), views: plotViews(r, pl) })) };
	}

	// ---- refusals -----------------------------------------------------------------------------

	function needRun(): Run {
		if (!run) refuse('refused', 'not_configured', 'Sine is not configured; configure a test first.');
		return run;
	}

	function wrongState(r: Run, verb: string): never {
		refuse('refused', 'wrong_state', `${verb} does not apply while the run is ${r.state} (${r.phase.replaceAll('_', ' ')}).`, { state: r.state, phase: r.phase });
	}

	function notWhileOther() {
		const other = host.otherRun();
		if (other) refuse('refused', 'run_active', `Run ${other} holds the station; stop it first.`, { run_id: other });
	}

	function notWhileLive(what: string) {
		if (run && isLive()) refuse('refused', 'run_active', `Run ${run.id} is ${run.state}; stop it before ${what}.`, { run_id: run.id });
	}

	function needSweep(r: Run) {
		if (r.config.test.kind !== 'sweep') refuse('refused', 'not_a_sweep', `The test is a ${r.config.test.kind}; only a sweep turns, jogs or goes to a frequency.`, { test: r.config.test.kind });
	}

	function needRunning(r: Run, verb: string) {
		if (r.state !== 'running' || !DRIVING.includes(r.phase)) wrongState(r, verb);
	}

	function parseSubscribe(params: Record<string, unknown>): { rate: number; plots: SineSubscribePlot[] } | null {
		if (params.default !== undefined) {
			if (params.default !== true) bad('default', '"default" can only be true.');
			if (params.plots !== undefined || params.rate_hz !== undefined) bad('default', '"default" takes no other parameter.');
			return null;
		}
		const rate = params.rate_hz === undefined ? 5 : params.rate_hz;
		if (!(isNum(rate) && rate >= 0.5 && rate <= 10)) bad('rate_hz', '"rate_hz" is a number from 0.5 to 10.');
		if (!Array.isArray(params.plots)) bad('plots', '"plots" is a list of plots.');
		const list = params.plots as unknown[];
		if (list.length > 64) bad('plots', 'At most 64 plots.');
		const seen = new Set<string>();
		const plots = list.map((raw, i): SineSubscribePlot => {
			const at = `plots[${i}]`;
			if (!isObject(raw)) bad(at, `${at} is an object.`);
			const p = raw as Record<string, unknown>;
			const extra = Object.keys(p).find((k) => !['id', 'quantity', 'channels', 'points'].includes(k));
			if (extra) bad(`${at}.${extra}`, `A plot has an id, a quantity, channels and points; not "${extra}".`);
			if (typeof p.id !== 'string' || p.id.length < 1 || p.id.length > 64) bad(`${at}.id`, 'A plot id is 1 to 64 characters.');
			if (seen.has(p.id as string)) bad(`${at}.id`, `The plot id "${p.id}" is used twice.`);
			seen.add(p.id as string);
			if (!QUANTITIES.includes(p.quantity as SineLiveQuantity)) bad(`${at}.quantity`, `"quantity" is one of ${QUANTITIES.join(', ')}.`);
			const quantity = p.quantity as SineLiveQuantity;
			const out: SineSubscribePlot = { id: p.id as string, quantity };
			if (WITH_CHANNELS.includes(quantity)) {
				const ch = p.channels ?? 'all';
				const ok = ch === 'all' || ch === 'control' || ch === 'limited' || (Array.isArray(ch) && ch.every((x) => Number.isInteger(x) && x >= 0) && new Set(ch).size === ch.length);
				if (!ok) bad(`${at}.channels`, '"channels" is a list of channel indexes, or "all", "control" or "limited".');
				out.channels = ch as SineSubscribePlot['channels'];
			} else if (p.channels !== undefined && p.channels !== null) bad(`${at}.channels`, `"channels" does not apply to ${quantity}.`);
			if (quantity === 'time') {
				const pts = p.points ?? 1024;
				if (!(Number.isInteger(pts) && (pts as number) >= 64 && (pts as number) <= 8192)) bad(`${at}.points`, '"points" is a whole number from 64 to 8192.');
				out.points = pts as number;
			} else if (p.points !== undefined && p.points !== null) bad(`${at}.points`, `"points" does not apply to ${quantity}.`);
			return out;
		});
		return { rate: rate as number, plots };
	}

	function shapeOk(config: Record<string, unknown>) {
		for (const key of ['profile', 'test', 'limits'] as const) if (!isObject(config[key])) bad(`config.${key}`, `The configuration has no "${key}".`);
		const t = config.test as Record<string, unknown>;
		if (!['sweep', 'dwell', 'step', 'rstd'].includes(String(t.kind))) bad('config.test.kind', 'A test is "sweep", "dwell", "step" or "rstd".');
		if (!Array.isArray((config.profile as Record<string, unknown>).rows)) bad('config.profile.rows', 'A profile has a list of rows.');
		const lim = config.limits as Record<string, unknown>;
		for (const k of ['drive_vpk_ceiling', 'drive_vpk_abort']) if (!isNum(lim[k])) bad(`config.limits.${k}`, `"${k}" is a number of volts peak; it has no default.`);
	}

	// ---- profiles -----------------------------------------------------------------------------

	/** A header cell as the service compares it: case, spaces, underscores and a unit in parentheses do not matter. */
	const norm = (h: string) => h.toLowerCase().replace(/\(.*?\)/g, '').replace(/[\s_]+/g, '');
	const QCOLS: { q: SineQuantity; names: string[] }[] = [
		{ q: 'accel_g', names: ['accel', 'g', 'gpk', 'accelg'] },
		{ q: 'velocity_ips', names: ['velocity', 'vel', 'ips', 'velocityips'] },
		{ q: 'disp_pp_in', names: ['displacement', 'disp', 'inpkpk', 'disppin', 'dispppin'] }
	];
	const FCOLS = ['freqhz', 'freq', 'frequency', 'hz'];

	function importText(text: string, field: 'text' | 'path'): { profile: SineProfile | null; errors: Item[]; warnings: Item[] } {
		const errors: Item[] = [];
		const warnings: Item[] = [];
		const lines = text.split(/\r?\n/).map((l) => l.replace(/(#|\/\/).*$/, '').trim()).filter((l) => l);
		const split = (l: string) => (/[\t,;]/.test(l) ? l.split(/\s*[\t,;]\s*/) : l.split(/\s+/));
		if (!lines.length) return { profile: null, errors: [item('profile_file', 'There are no rows to read.', field)], warnings };
		const first = split(lines[0]);
		const headed = !first.every((h) => h === '' || Number.isFinite(Number(h)));
		let fi = 0;
		let qcols: { i: number; q: SineQuantity }[] = [{ i: 1, q: 'accel_g' }];
		let alarm = -1;
		let abort = -1;
		if (headed) {
			const h = first.map(norm);
			fi = h.findIndex((x) => FCOLS.includes(x));
			qcols = h.map((x, i) => ({ i, q: QCOLS.find((c) => c.names.includes(x))?.q })).filter((c): c is { i: number; q: SineQuantity } => !!c.q);
			alarm = h.indexOf('alarm');
			abort = h.indexOf('abort');
			if (h.includes('units') || h.includes('unit')) warnings.push(item('units_column_ignored', 'The units column is read and ignored: each quantity column carries its own unit.'));
			if (fi < 0 || !qcols.length) return { profile: null, errors: [item('profile_file', 'The header names no frequency column, or no quantity column.', field)], warnings };
		} else if (first.length !== 2) return { profile: null, errors: [item('profile_file', 'Without a header, a row is a frequency and a value in g peak.', field)], warnings };
		const rows: SineProfileRow[] = [];
		lines.slice(headed ? 1 : 0).forEach((l, row) => {
			const cells = split(l);
			const f = Number(cells[fi]);
			const given = qcols.filter((c) => (cells[c.i] ?? '').trim() !== '');
			if (!Number.isFinite(f)) errors.push(item('profile_file', `Row ${row + 1}: the frequency "${cells[fi] ?? ''}" is not a number.`, undefined, row));
			else if (given.length > 1) errors.push(item('quantity_ambiguous', `Row ${row + 1}: the row gives more than one quantity.`, undefined, row));
			else if (!given.length) errors.push(item('profile_file', `Row ${row + 1}: the row gives no value.`, undefined, row));
			else {
				const v = Number(cells[given[0].i]);
				if (!Number.isFinite(v)) errors.push(item('profile_file', `Row ${row + 1}: the value is not a number.`, undefined, row));
				else {
					const r: SineProfileRow = { freq_hz: f, quantity: given[0].q, value: v };
					const al = alarm >= 0 ? Number(cells[alarm]) : NaN;
					const ab = abort >= 0 ? Number(cells[abort]) : NaN;
					if (Number.isFinite(al)) r.alarm_db = [al, al];
					if (Number.isFinite(ab)) r.abort_db = [ab, ab];
					rows.push(r);
				}
			}
		});
		if (errors.length) return { profile: null, errors, warnings };
		const profile: SineProfile = { label: '', rows, alarm_db: [1.5, 1.5], abort_db: [3, 3] };
		checkProfile(profile, 'profile', errors, warnings);
		for (const e of [...errors, ...warnings]) {
			const m = /^profile\.rows\[(\d+)\]/.exec(e.field ?? '');
			if (m) {
				e.row = Number(m[1]);
				delete e.field;
			}
		}
		return { profile: errors.length ? null : profile, errors, warnings };
	}

	// ---- the methods -----------------------------------------------------------------------------

	return {
		laws,
		isLive,
		liveId: () => (run && isLive() ? run.id : null),
		detach: (conn) => {
			subs.delete(conn);
		},
		seatExpired: () => {
			if (run && isLive() && run.state !== 'stopping') stopRun(run, 'safe_stop');
		},
		methods: {
			'laws.import': (params, conn) => {
				host.own(conn);
				const path = params.path;
				if (!absolute(path) || !path.trim()) bad('path', 'A law file is named by its absolute path on the computer that runs the test.');
				if (!/\.wasm$/i.test(path as string)) refuse('refused', 'law_import_failed', 'The file is not a law file (.wasm).');
				const law = importedLaw(path as string);
				if (!imported.some((l) => l.id === law.id)) imported = [...imported, law];
				return { law };
			},
			'sine.configure': (params, conn) => {
				if (!isObject(params.config)) bad('config', 'Configuring needs a test configuration.');
				shapeOk(params.config as Record<string, unknown>);
				const config = params.config as unknown as SineConfig;
				const checked = check(config);
				if (params.dry_run === true) return { errors: checked.errors, warnings: checked.warnings, derived: checked.derived, views: checked.views };
				host.own(conn);
				notWhileLive('configuring');
				if (checked.errors.length || !checked.derived) refuse('bad_request', 'config_invalid', `The configuration has ${checked.errors.length} error(s).`, { errors: checked.errors, warnings: checked.warnings });
				if (run?.timer) clearInterval(run.timer);
				run = newRun(config, checked.derived, checked.views);
				log = [];
				addLog('state', `configured "${config.title ?? ''}" on ${run.station.class}`);
				stateChanged(true);
				return { state: stateOf(run), derived: checked.derived, warnings: checked.warnings };
			},
			'sine.config': () => (run ? { run_id: run.id, config: run.config, derived: run.derived, views: run.views } : { run_id: null, config: null, derived: null, views: [] }),
			'sine.profile.import': (params) => {
				const hasText = params.text !== undefined;
				const hasPath = params.path !== undefined;
				if (hasText === hasPath) bad('text', 'Give the rows as "text", or a file by its "path"; one of the two.');
				if (hasText) {
					if (typeof params.text !== 'string') bad('text', '"text" is the rows as text.');
					return importText(params.text as string, 'text');
				}
				if (!absolute(params.path)) bad('path', 'A profile file is named by its full path on the computer that runs the test.');
				const path = params.path as string;
				if (/missing|\.bad$/i.test(path)) return { profile: null, errors: [item('profile_file', `Could not read ${path}: no such file.`, 'path')], warnings: [] };
				return importText('freq_hz, disp_pp_in, accel_g\n10, 0.05,\n31.4, , 2.5\n2000, , 2.5\n', 'path');
			},
			'sine.profile.validate': (params) => {
				if (!isObject(params.profile)) bad('profile', 'A profile to check.');
				const profile = params.profile as unknown as SineProfile;
				if (!Array.isArray(profile.rows)) bad('profile.rows', 'A profile has a list of rows.');
				const band = Array.isArray(params.band_hz) && params.band_hz.length === 2 ? (params.band_hz as [number, number]) : null;
				const errors: Item[] = [];
				const warnings: Item[] = [];
				checkProfile(profile, 'profile', errors, warnings);
				if (errors.length) return { errors, warnings, crossovers: [], kinematics: null, views: [] };
				const rows = profile.rows;
				const span: [number, number] = band ?? [rows[0].freq_hz, rows[rows.length - 1].freq_hz];
				if (band && (rows[0].freq_hz > band[0] + 1e-9 || rows[rows.length - 1].freq_hz < band[1] - 1e-9))
					errors.push(item('profile_not_covering', `The profile covers ${rows[0].freq_hz} to ${rows[rows.length - 1].freq_hz} Hz; the test runs from ${band[0]} to ${band[1]} Hz.`, 'band_hz'));
				if (errors.length) return { errors, warnings, crossovers: crossovers(profile), kinematics: null, views: [] };
				const peaks = peaksOver(profile, span);
				const feas = isObject(params.feasibility);
				const kinematics = {
					...peaks,
					verdict: feas ? (host.feasibilityExceeds ? ('exceeded' as const) : ('pass' as const)) : ('not_evaluated' as const),
					message: feas ? (host.feasibilityExceeds ? "The profile's displacement exceeds the shaker's stroke." : "Within the shaker's ratings.") : 'No shaker was given, so the ratings were not checked.'
				};
				return { errors, warnings, crossovers: crossovers(profile), kinematics, views: profileViews(profile, span) };
			},
			'sine.arm': (params, conn) => {
				host.own(conn);
				const r = needRun();
				const ack = params.override_ack;
				if (ack !== undefined && (typeof ack !== 'string' || !ack.trim())) bad('override_ack', "An override acknowledgement is the operator's own words; it cannot be empty.");
				if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.', { outcome: r.end?.outcome });
				if (r.state !== 'configured' && r.state !== 'blocked') wrongState(r, 'Arming');
				notWhileOther();
				arm(r, ack as string | undefined);
				stateChanged();
				return stateOf(r);
			},
			'sine.start': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.', { outcome: r.end?.outcome });
				if (r.state !== 'configured' && r.state !== 'armed') wrongState(r, 'Starting');
				notWhileOther();
				if (r.state === 'configured') arm(r, undefined);
				start(r);
				return stateOf(r);
			},
			'sine.proceed': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				if (r.state !== 'review') wrongState(r, 'Proceeding');
				addLog('note', 'the operator reviewed identification and proceeded');
				engage(r);
				return stateOf(r);
			},
			'sine.level': (params, conn) => {
				host.own(conn);
				const r = needRun();
				const hasDelta = isNum(params.delta_db);
				const hasTo = isNum(params.to_db);
				if (hasDelta === hasTo) bad('delta_db', 'A level change gives either a step in dB or a level of the schedule.');
				if (r.mode !== 'manual') refuse('refused', 'manual_mode_required', 'Level changes need the manual ladder; switch the ladder to manual first.', { mode: r.mode });
				needRunning(r, 'A level change');
				let target: number;
				if (hasTo) {
					const i = r.levels.findIndex((l) => Math.abs(l - (params.to_db as number)) < 1e-9);
					if (i < 0) refuse('bad_request', 'not_a_rung', `${params.to_db} dB is not a level of the schedule (${r.levels.join(', ')} dB).`, { levels_db: r.levels });
					r.rung = i;
					target = r.levels[i];
				} else {
					target = Math.min(r.levels[r.levels.length - 1], Math.max(r.levels[0], r.commanded + (params.delta_db as number)));
					const i = r.levels.findIndex((l) => Math.abs(l - target) < 1e-9);
					if (i >= 0) r.rung = i;
				}
				r.commanded = round(target, 2);
				r.atLevelT = 0;
				r.sweepsAtLevel = 0;
				addLog('level', `level ${r.commanded} dB`);
				stateChanged(true);
				return { level_db: r.commanded };
			},
			'sine.mode': (params, conn) => {
				host.own(conn);
				const r = needRun();
				const m = params.mode;
				if (m !== 'auto' && m !== 'manual') bad('mode', '"mode" is "auto" or "manual".');
				if (m === r.mode) return { mode: m };
				r.mode = m as 'auto' | 'manual';
				addLog('note', `ladder mode ${m}`);
				stateChanged(true);
				return { mode: m };
			},
			'sine.hold': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				if (r.state !== 'running') wrongState(r, 'Hold');
				if (!r.held) {
					r.held = true;
					addLog('note', `held at ${round(r.freq, 1)} Hz`);
				}
				stateChanged(true);
				return stateOf(r);
			},
			'sine.resume': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				if (r.state !== 'running') wrongState(r, 'Resume');
				if (r.held) {
					r.held = false;
					addLog('note', 'resumed');
				}
				stateChanged(true);
				return stateOf(r);
			},
			'sine.reverse': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				needSweep(r);
				needRunning(r, 'Reverse');
				r.direction = r.direction === 'up' ? 'down' : 'up';
				addLog('note', `sweep turned; now ${r.direction}`);
				stateChanged(true);
				return stateOf(r);
			},
			'sine.jog': (params, conn) => {
				host.own(conn);
				const r = needRun();
				if (!isNum(params.delta_hz)) bad('delta_hz', '"delta_hz" is a number of hertz.');
				needSweep(r);
				needRunning(r, 'Jog');
				const [lo, hi] = bandOf(r.config.test);
				r.freq = Math.min(hi, Math.max(lo, r.freq + (params.delta_hz as number)));
				addLog('note', `jogged to ${round(r.freq, 1)} Hz`);
				return { frequency_hz: round(r.freq, 3) };
			},
			'sine.goto': (params, conn) => {
				host.own(conn);
				const r = needRun();
				if (!isNum(params.hz)) bad('hz', '"hz" is a frequency in hertz.');
				needSweep(r);
				needRunning(r, 'Go to');
				const band = bandOf(r.config.test);
				const hz = params.hz as number;
				if (hz < band[0] || hz > band[1]) refuse('refused', 'out_of_band', `${hz} Hz is outside the band, ${band[0]} to ${band[1]} Hz.`, { band_hz: band });
				r.freq = hz;
				addLog('note', `went to ${round(hz, 1)} Hz`);
				return { frequency_hz: round(r.freq, 3) };
			},
			'sine.adjust': (params, conn) => {
				host.own(conn);
				const r = needRun();
				if (typeof params.key !== 'string' || !params.key) bad('key', '"key" names a parameter of the law.');
				if (!isNum(params.value)) bad('value', '"value" is a number.');
				// the service's order: the state, then the key and value, then the law, which is built at proceed
				if (!(r.state === 'running' && DRIVING.includes(r.phase)) && r.state !== 'review') wrongState(r, 'An adjust');
				const key = params.key as string;
				const value = params.value as number;
				const decl = r.law.params.find((p) => p.key === key);
				const no = (why: string, message: string): never => refuse('refused', 'adjust_refused', message, { key, why });
				if (!decl) no('unknown_key', `${r.law.name} has no parameter "${key}".`);
				if (!(r.law.live ?? []).includes(key) || !numberParam(decl!)) no('not_live', `${decl!.label} is fixed for the run; only ${(r.law.live ?? []).join(', ') || 'no parameter'} may change.`);
				const num = decl as Extract<LawParam, { kind: 'number' | 'integer' }>;
				if (value < num.min || value > num.max) no('out_of_range', `${num.label} is from ${num.min} to ${num.max}.`);
				if (r.state === 'review') wrongState(r, 'An adjust');
				if (!r.law.builtin && key === 'kp' && value > 1.5) no('law_refused', 'The law refused it: a proportional gain over 1.5 would not hold the loop.');
				const was = r.lawLive[key];
				r.lawLive[key] = value;
				addLog('law', `${key} ${was} to ${value}`);
				stateChanged(true);
				return { key, value };
			},
			'sine.stop': (_params, conn) => {
				if (run && isLive() && run.state !== 'stopping') {
					const by = host.clientName(conn);
					addLog('note', `stop requested by ${by}`);
					stopRun(run, 'operator_stop', by);
				}
				return stateOf(run);
			},
			'sine.abort': (_params, conn) => {
				if (run && isLive()) {
					const by = host.clientName(conn);
					addLog('note', `abort requested by ${by}`);
					end(run, 'operator_abort', 'aborted', null, by);
				}
				return stateOf(run);
			},
			'sine.reset': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				notWhileLive('resetting');
				const checked = check(r.config);
				run = newRun(r.config, checked.derived ?? r.derived, checked.views.length ? checked.views : r.views);
				log = [];
				addLog('state', `reset; configured "${r.config.title ?? ''}" again`);
				stateChanged(true);
				return stateOf(run);
			},
			'sine.clear': (_params, conn) => {
				host.own(conn);
				notWhileLive('clearing');
				run = null;
				log = [];
				stateChanged(true);
				return stateOf(null);
			},
			'sine.state': () => stateOf(run),
			'sine.log': (params) => {
				const since = isNum(params.since) ? Math.max(0, Math.floor(params.since)) : 0;
				return { entries: log.slice(since), next: log.length };
			},
			'sine.subscribe': (params, conn) => {
				const sub = parseSubscribe(params);
				if (!sub) {
					subs.delete(conn);
					return { default: true, rate_hz: 5, plots: null, quantities: QUANTITIES };
				}
				const entry = { ...sub, acc: 0 };
				subs.set(conn, entry);
				if (run && run.state !== 'configured' && run.state !== 'blocked') {
					const r = run;
					setTimeout(() => {
						if (subs.get(conn) === entry && run === r) conn.emit('sine.data', dataFor(r, sub.plots) as unknown as Record<string, unknown>);
					}, 30);
				}
				return { default: false, rate_hz: sub.rate, plots: sub.plots, quantities: QUANTITIES };
			}
		}
	};
}
