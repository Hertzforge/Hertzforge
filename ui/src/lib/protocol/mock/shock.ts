/**
 * Mock classical shock (schema/PROTOCOL.md, "Classical Shock" and the classical world of
 * "Control Laws"), on the mock control service's run slot and controller seat.
 *
 * A believable closed-loop run: identification with band-limited random, the review hold, the
 * law's seed, then pulse after pulse with the time between pulses, each judged against the
 * standard's tolerance and corrected by the law. The first pulse at the first level comes in about
 * a quarter low and the correction closes most of the error each pulse, so an automatic ladder
 * that advances on a pass shows a failed pulse, a correction and a pass. The manual ladder fires,
 * steps, trains and bursts on the operator's word. Stop finishes the pulse in flight; abort ends
 * at once; with nobody in control firing pauses, and the run safe-stops after the timeout.
 *
 * Every value is synthetic, made only so the shell has something to draw. Nothing here is the
 * service's pulse math.
 */

import type { ChannelRow, DataSource, LogEntry, LogKind, ReadingRow, RefusalReason, Series, SpectrumView, StationRef, TableView, View } from '../types';
import type { LawEntry } from '../mimo';
import type {
	ShockConfig,
	ShockDerived,
	ShockEnd,
	ShockLive,
	ShockOutcome,
	ShockPhase,
	ShockPulse,
	ShockPulseRow,
	ShockQuantity,
	ShockReportItem,
	ShockRunState,
	ShockStandard,
	ShockState,
	ShockSubscribePlot,
	ShockTrip
} from '../shock';
import { MockError, type MockConnection, type MockMethod } from './socket';

/** What the shock mock needs of the control service around it. */
export interface ShockMockHost {
	/** Run seconds per real second. */
	speed: number;
	/** Seconds a run with nobody in control is kept before its safe stop. */
	timeoutS: number;
	/** Throws `not_controller` unless `conn` holds control. */
	own(conn: MockConnection): void;
	/** Somebody holds control and is connected. */
	hasController(): boolean;
	/** Another run that holds the station (Random, a simulation), or null. */
	otherRun(): string | null;
	broadcast(topic: string, data: Record<string, unknown>): void;
	connections(): Iterable<MockConnection>;
	rows(): ChannelRow[];
	station(): { source: DataSource; ref: StationRef };
	clientName(conn: MockConnection): string;
	feasibilityExceeds: boolean;
}

const G_IPS2 = 386.089;
const TICK_MS = 50;
const LIVE_MS = 200;
const SYSID_S = 2;
const FIRE_S = 0.3;

function refuse(code: 'refused' | 'bad_request', reason: RefusalReason | 'not_yet' | 'wrong_mode' | 'drive_ceiling', message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason as RefusalReason, detail);
}

const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

// ---------------------------------------------------------------------------------------------
// The law catalog

const FD_CORRECT: LawEntry = {
	id: 'fd_correct',
	world: 'shock_classical',
	builtin: true,
	name: 'frequency-domain correction',
	version: '0.1.0',
	author: 'Hertzforge',
	summary: 'Corrects the next drive by the measured error on the pulse grid, through the identified system.',
	sha256: null,
	check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: 'Built in: cleared for hardware.' },
	params: [
		{ key: 'beta', label: 'Correction gain', help: 'The share of the measured error each pulse corrects.', kind: 'number', unit: '', min: 0.05, max: 1, step: 0.05, default: 0.5 },
		{ key: 'max_db_per_pulse', label: 'Largest change per pulse', help: 'The most any line of the drive may change from one pulse to the next.', kind: 'number', unit: 'dB', min: 1, max: 9, step: 0.5, default: 6 },
		{ key: 'initial_backoff_db', label: 'First drive back-off', help: 'How far under the identified drive the first pulse is played.', kind: 'number', unit: 'dB', min: 0, max: 12, step: 1, default: 0 }
	],
	diagnostics: [
		{ key: 'corrected_bins', label: 'Lines corrected', unit: '', help: 'Lines of the drive the correction changed.', layout: null },
		{ key: 'clamped_bins', label: 'Lines held back', unit: '', help: 'Lines whose change was held to the largest change per pulse.', layout: null },
		{ key: 'drive_change_db', label: 'Drive change', unit: 'dB', help: "The drive's change at its peak.", layout: null }
	],
	shapes: null,
	needs: null
};

function importedLaw(path: string): LawEntry {
	let h = 0;
	for (const ch of path) h = (h * 31 + ch.charCodeAt(0)) >>> 0;
	const sha = (h.toString(16).padStart(8, '0') + 'c0ffee').repeat(5).slice(0, 64);
	return {
		id: sha,
		world: 'shock_classical',
		builtin: false,
		name: 'shock template',
		version: '0.1.0',
		author: 'law author',
		summary: 'The classical shock template: the built-in correction with a switch and a model error.',
		sha256: sha,
		check: { status: 'unchecked', date: null, tool_version: null, cases: [], hardware: false, message: 'Not checked: runs on the simulator; check it before it drives hardware.' },
		params: [
			{ key: 'beta', label: 'Correction gain', help: 'The share of the measured error each pulse corrects.', kind: 'number', unit: '', min: 0.05, max: 1, step: 0.05, default: 0.5 },
			{ key: 'correct', label: 'Correct', help: 'Off plays the seed again every pulse.', kind: 'toggle', default: true },
			{ key: 'model_error_db', label: 'Model error', help: 'An error put on the identified system on purpose.', kind: 'number', unit: 'dB', min: -12, max: 12, step: 1, default: 0 }
		],
		diagnostics: [{ key: 'drive_change_db', label: 'Drive change', unit: 'dB', help: "The drive's change at its peak.", layout: null }],
		shapes: null,
		needs: null
	};
}

// ---------------------------------------------------------------------------------------------
// The pulse, made up for the screen

const STANDARD_TOL: Record<string, { amplitude: number; velocity: number }> = {
	mil_std_810_516: { amplitude: 0.15, velocity: 0.1 },
	iec_60068_2_27: { amplitude: 0.2, velocity: 0.15 }
};

function mainShape(p: ShockPulse, x: number): number {
	if (x < 0 || x > 1) return 0;
	switch (p.shape) {
		case 'half_sine':
			return Math.sin(Math.PI * x);
		case 'haversine':
			return (1 - Math.cos(2 * Math.PI * x)) / 2;
		case 'terminal_peak_sawtooth':
			return x < 0.9 ? x / 0.9 : (1 - x) / 0.1;
		case 'initial_peak_sawtooth':
			return x < 0.1 ? x / 0.1 : (1 - x) / 0.9;
		case 'trapezoid': {
			const r = p.rise_frac ?? 0.1;
			const f = p.fall_frac ?? 0.1;
			return x < r ? x / r : x > 1 - f ? (1 - x) / f : 1;
		}
		case 'rectangle':
			return x < 0.05 ? x / 0.05 : x > 0.95 ? (1 - x) / 0.05 : 1;
	}
}

/** The area under the main pulse, a fraction of amplitude times duration. */
function mainArea(p: ShockPulse): number {
	let s = 0;
	const n = 400;
	for (let i = 0; i < n; i++) s += mainShape(p, (i + 0.5) / n);
	return s / n;
}

interface Composite {
	t_ms: Float64Array;
	a: Float64Array;
	/** The shoulder's height, a fraction of the peak. */
	shoulder: number;
}

function composite(p: ShockPulse, fs: number, scale = 1): Composite {
	const T = p.duration_ms / 1000;
	const comp = p.compensation ?? { shape: 'rectangular', pre_frac: 4, post_frac: 4, symmetric: true };
	const pre = comp.pre_frac * T;
	const post = (comp.symmetric ? comp.pre_frac : comp.post_frac) * T;
	const area = mainArea(p) * T;
	const shapeArea = comp.shape === 'half_sine' ? 2 / Math.PI : comp.shape === 'rounded_rectangular' ? 0.9 : 1;
	const preAmp = area / 2 / (pre * shapeArea);
	const postAmp = area / 2 / (post * shapeArea);
	const total = pre + T + post;
	const n = Math.min(4096, Math.max(256, Math.round(total * Math.min(fs, 20000))));
	const t = new Float64Array(n);
	const a = new Float64Array(n);
	const sign = p.polarity === 'negative' ? -1 : 1;
	const shoulder = (x: number) => (comp.shape === 'half_sine' ? Math.sin(Math.PI * x) : comp.shape === 'rounded_rectangular' ? Math.min(1, 5 * Math.sin(Math.PI * x)) : x >= 0 && x <= 1 ? 1 : 0);
	for (let i = 0; i < n; i++) {
		const s = (i / (n - 1)) * total;
		let v: number;
		if (s < pre) v = -preAmp * shoulder(s / pre);
		else if (s <= pre + T) v = mainShape(p, (s - pre) / T);
		else v = -postAmp * shoulder((s - pre - T) / post);
		t[i] = (s - pre) * 1000;
		a[i] = sign * scale * p.amplitude_g * v;
	}
	return { t_ms: t, a, shoulder: Math.max(preAmp, postAmp) };
}

function integrate(t_ms: Float64Array, y: Float64Array, k: number): Float64Array {
	const out = new Float64Array(y.length);
	for (let i = 1; i < y.length; i++) out[i] = out[i - 1] + ((y[i] + y[i - 1]) / 2) * ((t_ms[i] - t_ms[i - 1]) / 1000) * k;
	return out;
}

const timeAxis = { label: 'Time', unit: 'ms', scale: 'linear' as const };

function pulseView(p: ShockPulse, fs: number, standard: ShockStandard | undefined, scale: number, measured?: Float64Array): SpectrumView {
	const c = composite(p, fs, scale);
	const tol = STANDARD_TOL[standard?.name ?? 'mil_std_810_516'];
	const peak = p.amplitude_g * scale;
	const sign = p.polarity === 'negative' ? -1 : 1;
	const T = p.duration_ms;
	const upper = new Float64Array(c.a.length);
	const lower = new Float64Array(c.a.length);
	for (let i = 0; i < c.a.length; i++) {
		const inMain = c.t_ms[i] >= -0.2 * T && c.t_ms[i] <= 1.2 * T;
		const ref = c.a[i];
		const band = tol.amplitude * peak;
		upper[i] = inMain ? ref + sign * band : sign * band;
		lower[i] = inMain ? ref - sign * band : -sign * band;
	}
	const series: Series[] = [
		{ name: 'Reference', role: 'reference', x: c.t_ms, y: c.a },
		{ name: 'Upper tolerance', role: 'tolerance', x: c.t_ms, y: upper },
		{ name: 'Lower tolerance', role: 'tolerance', x: c.t_ms, y: lower }
	];
	if (measured) series.push({ name: 'Measured', role: 'measured', x: c.t_ms, y: measured });
	return { view: 'spectrum', title: 'Pulse', x: timeAxis, y: { label: 'Acceleration', unit: 'g', scale: 'linear' }, series, link: 'time' };
}

function driveView(t_ms: Float64Array, drive: Float64Array): SpectrumView {
	return { view: 'spectrum', title: 'Drive', x: timeAxis, y: { label: 'Drive', unit: 'V', scale: 'linear' }, series: [{ name: 'Drive played', role: 'drive', x: t_ms, y: drive }], link: 'time' };
}

function kinematicViews(p: ShockPulse, fs: number, scale: number, measured?: Float64Array): SpectrumView[] {
	const c = composite(p, fs, scale);
	const v = integrate(c.t_ms, c.a, G_IPS2);
	const d = integrate(c.t_ms, v, 1);
	const vSeries: Series[] = [{ name: 'Reference', role: 'reference', x: c.t_ms, y: v }];
	const dSeries: Series[] = [{ name: 'Reference', role: 'reference', x: c.t_ms, y: d }];
	if (measured) {
		const mv = integrate(c.t_ms, measured, G_IPS2);
		vSeries.push({ name: 'Measured', role: 'measured', x: c.t_ms, y: mv });
		dSeries.push({ name: 'Measured', role: 'measured', x: c.t_ms, y: integrate(c.t_ms, mv, 1) });
	}
	return [
		{ view: 'spectrum', title: 'Velocity', x: timeAxis, y: { label: 'Velocity', unit: 'in/s', scale: 'linear' }, series: vSeries, link: 'time' },
		{ view: 'spectrum', title: 'Displacement', x: timeAxis, y: { label: 'Displacement', unit: 'in', scale: 'linear' }, series: dSeries, link: 'time' }
	];
}

function referenceFacts(p: ShockPulse, fs: number) {
	const c = composite(p, fs);
	const v = integrate(c.t_ms, c.a, G_IPS2);
	const d = integrate(c.t_ms, v, 1);
	let dmin = Infinity;
	let dmax = -Infinity;
	for (const x of d) {
		dmin = Math.min(dmin, x);
		dmax = Math.max(dmax, x);
	}
	const T = p.duration_ms / 1000;
	const start = c.t_ms.findIndex((t) => t >= 0);
	return {
		peak_g: p.amplitude_g,
		velocity_change_ips: round(mainArea(p) * T * p.amplitude_g * G_IPS2, 2),
		displacement_pk_pk_in: round(dmax - dmin, 4),
		main_start: Math.max(0, start),
		main_len: Math.round(T * fs),
		shoulder: c.shoulder
	};
}

function quietWindow(p: ShockPulse, standard: ShockStandard | undefined, shoulder: number) {
	const limit = STANDARD_TOL[standard?.name ?? 'mil_std_810_516'].amplitude;
	const worst = round(shoulder, 3);
	return { worst_frac: worst, limit_frac: limit, inside: worst <= limit };
}

function srsView(p: ShockPulse, scale: number, tolDb: number, f: [number, number], measuredRatio?: number): SpectrumView {
	const n = 60;
	const freq = new Float64Array(n);
	const ref = new Float64Array(n);
	const up = new Float64Array(n);
	const lo = new Float64Array(n);
	const meas = new Float64Array(n);
	const A = p.amplitude_g * scale;
	const dv = mainArea(p) * (p.duration_ms / 1000) * A * G_IPS2;
	for (let i = 0; i < n; i++) {
		const fr = f[0] * (f[1] / f[0]) ** (i / (n - 1));
		const u = (2 * Math.PI * fr * dv) / (G_IPS2 * A);
		const s = (A * u * (1 + 0.6 * Math.exp(-(Math.log(u) ** 2)))) / (1 + u);
		freq[i] = fr;
		ref[i] = s;
		up[i] = s * 10 ** (tolDb / 20);
		lo[i] = s * 10 ** (-tolDb / 20);
		meas[i] = s * (measuredRatio ?? 1) * (1 + 0.04 * Math.sin(i * 0.9));
	}
	const series: Series[] = [
		{ name: 'Reference', role: 'reference', x: freq, y: ref },
		{ name: 'Upper band', role: 'tolerance', x: freq, y: up },
		{ name: 'Lower band', role: 'tolerance', x: freq, y: lo }
	];
	if (measuredRatio !== undefined) series.push({ name: 'Measured', role: 'measured', x: freq, y: meas });
	return { view: 'spectrum', title: 'SRS', x: { label: 'Frequency', unit: 'Hz', scale: 'log' }, y: { label: 'Peak acceleration', unit: 'g', scale: 'log' }, series, link: 'srs' };
}

/** The simulated station's plant: two modes, 200 and 550 Hz at Q 2.5. */
function plantMag(f: number): number {
	const mode = (fn: number, q: number) => 1 / Math.sqrt((1 - (f / fn) ** 2) ** 2 + (f / (fn * q)) ** 2);
	return 0.6 * mode(200, 2.5) + 0.4 * mode(550, 2.5);
}

function identificationViews(band: [number, number], coherence: number, floor: number): SpectrumView[] {
	const n = 200;
	const f = new Float64Array(n);
	const mag = new Float64Array(n);
	const ph = new Float64Array(n);
	const coh = new Float64Array(n);
	const trusted = new Float64Array(n);
	const fl = new Float64Array(n).fill(floor);
	for (let i = 0; i < n; i++) {
		const fr = 5 * (8000 / 5) ** (i / (n - 1));
		f[i] = fr;
		mag[i] = plantMag(fr) * 40;
		ph[i] = -180 * (Math.atan(fr / 200) + Math.atan(fr / 550)) / Math.PI;
		const inBand = fr >= band[0] && fr <= band[1];
		coh[i] = inBand ? Math.min(0.999, coherence + 0.04 * Math.sin(i / 7)) : 0.3;
		trusted[i] = inBand && coh[i] >= floor ? 1 : 0;
	}
	const freq = { label: 'Frequency', unit: 'Hz', scale: 'log' as const };
	return [
		{ view: 'spectrum', title: 'Identification Gain', x: freq, y: { label: '|H|', unit: 'g/V', scale: 'log' }, series: [{ name: '|H|', role: 'measured', x: f, y: mag }], link: 'frequency' },
		{ view: 'spectrum', title: 'Identification Phase', x: freq, y: { label: 'Phase', unit: 'deg', scale: 'linear' }, series: [{ name: 'Phase', role: 'measured', x: f, y: ph }], link: 'frequency' },
		{
			view: 'spectrum',
			title: 'Coherence',
			x: freq,
			y: { label: 'Coherence', unit: null, scale: 'linear' },
			series: [
				{ name: 'Coherence', role: 'measured', x: f, y: coh },
				{ name: 'Trusted bins', role: 'other', x: f, y: trusted },
				{ name: 'Floor', role: 'limit', x: f, y: fl }
			],
			link: 'frequency'
		}
	];
}

function spectrumView(p: ShockPulse, scale: number, ratio: number): SpectrumView {
	const n = 160;
	const f = new Float64Array(n);
	const r = new Float64Array(n);
	const y = new Float64Array(n);
	const T = p.duration_ms / 1000;
	for (let i = 0; i < n; i++) {
		const fr = 5 * (5000 / 5) ** (i / (n - 1));
		const x = fr * T;
		const env = (p.amplitude_g * scale * T) / (1 + (2 * x) ** 2);
		f[i] = fr;
		r[i] = env;
		y[i] = env * ratio * (1 + 0.05 * Math.cos(i / 5));
	}
	return {
		view: 'spectrum',
		title: 'Pulse Spectrum',
		x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
		y: { label: 'Magnitude', unit: 'g s', scale: 'log' },
		series: [
			{ name: '|R|', role: 'reference', x: f, y: r },
			{ name: '|Y|', role: 'measured', x: f, y }
		],
		link: 'frequency'
	};
}

// ---------------------------------------------------------------------------------------------
// The run

interface Run {
	id: string;
	config: ShockConfig;
	derived: ShockDerived;
	views: View[];
	state: ShockRunState;
	phase: ShockPhase;
	station: StationRef;
	source: DataSource;
	mode: 'auto' | 'manual';
	levels: number[];
	pulsesPer: number[];
	rung: number;
	levelDb: number;
	firedAtLevel: number;
	passedAtLevel: number;
	failsInRow: number;
	/** The law's remaining error on the next pulse, a fraction (negative: low). */
	error: number;
	rows: ShockPulseRow[];
	lastViews: View[];
	t: number;
	phaseT: number;
	nextAt: number | null;
	/** Pulses still to fire in a train or burst. */
	queue: { origin: 'train' | 'gunfire'; left: number; spacing: number } | null;
	/** Stop once the pulse in flight is judged, ending with this outcome. */
	stopAfterPulse: boolean;
	stopOutcome: ShockOutcome;
	stopBy: string | undefined;
	/** The pulse in flight: where it came from and the drive-limit scale applied. */
	pending: { origin: ShockPulseRow['origin']; limitScale: number } | null;
	identification: ShockState['identification'];
	prediction: ShockState['prediction'];
	feasibility: ShockState['feasibility'];
	end: ShockEnd | null;
	alarm: boolean;
	timer: ReturnType<typeof setInterval> | null;
	law: LawEntry;
	orphanAt: number | null;
}

export interface ShockMock {
	methods: Record<string, MockMethod>;
	isLive(): boolean;
	/** The run's id while it is live, for another run's `run_active` refusal. */
	liveId(): string | null;
	detach(conn: MockConnection): void;
	/** Nobody held control for the timeout: the run safe-stops. */
	seatExpired(): void;
}

const LIVE: ShockRunState[] = ['armed', 'running', 'review', 'stopping'];

export function createShockMock(host: ShockMockHost): ShockMock {
	let run: Run | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastStateKey = '';
	let imported: LawEntry[] = [];
	const subs = new Map<MockConnection, { rate: number; plots: ShockSubscribePlot[] }>();
	let liveTimer: ReturnType<typeof setInterval> | null = null;

	const laws = () => [FD_CORRECT, ...imported];
	const isLive = () => run !== null && LIVE.includes(run.state);

	function addLog(kind: LogKind, text: string) {
		const e: LogEntry = { seq: log.length, at_unix: Date.now() / 1000, kind, text };
		log.push(e);
		if (run) host.broadcast('shock.log', { run_id: run.id, entries: [e] });
	}

	function stateOf(r: Run | null): ShockState {
		if (!r) {
			return {
				run_id: null,
				state: 'unconfigured',
				phase: 'idle',
				title: null,
				source: null,
				station: null,
				grid: null,
				clock: null,
				mode: null,
				levels_db: null,
				level_db: null,
				rung: null,
				pulses: { fired: 0, passed: 0, at_level: 0, of_level: 0 },
				next_pulse_s: null,
				elapsed_s: null,
				alarm: false,
				identification: null,
				prediction: null,
				last_pulse: null,
				law: null,
				feasibility: null,
				end: null
			};
		}
		const started = r.state !== 'configured' && r.state !== 'blocked';
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.config.title,
			source: r.source,
			station: r.station,
			grid: r.derived.grid,
			clock: started ? 'shared' : null,
			mode: r.mode,
			levels_db: r.levels,
			level_db: r.levelDb,
			rung: r.levels.findIndex((l) => Math.abs(l - r.levelDb) < 1e-9) >= 0 ? r.levels.findIndex((l) => Math.abs(l - r.levelDb) < 1e-9) : null,
			pulses: { fired: r.rows.length, passed: r.rows.filter((x) => x.passed).length, at_level: r.firedAtLevel, of_level: r.pulsesPer[r.rung] ?? 1 },
			next_pulse_s: r.phase !== 'waiting' ? null : r.nextAt === null ? (r.mode === 'manual' ? 0 : null) : round(Math.max(0, r.nextAt - r.t), 2),
			elapsed_s: started ? round(r.t, 2) : null,
			alarm: r.alarm,
			identification: r.identification,
			prediction: r.prediction,
			last_pulse: r.rows.at(-1) ?? null,
			law: { id: r.law.id, name: r.law.name, version: r.law.version, sha256: r.law.sha256, misses: 0 },
			feasibility: r.feasibility,
			end: r.end
		};
	}

	function stateChanged(force = false) {
		const s = stateOf(run);
		const key = JSON.stringify({ ...s, next_pulse_s: null, elapsed_s: null });
		if (!force && key === lastStateKey) return;
		lastStateKey = key;
		host.broadcast('shock.state', s as unknown as Record<string, unknown>);
	}

	function liveOf(r: Run): ShockLive {
		const s = stateOf(r);
		return {
			run_id: r.id,
			source: r.source,
			state: r.state,
			phase: r.phase,
			mode: r.mode,
			level_db: r.levelDb,
			rung: s.rung,
			pulses: s.pulses,
			next_pulse_s: s.next_pulse_s,
			fire: r.phase === 'firing' ? { done: Math.min(10, Math.round((r.phaseT / FIRE_S) * 10)), of: 10 } : null
		};
	}

	// ---- checking a configuration ---------------------------------------------------------------

	function check(config: ShockConfig): { errors: ShockReportItem[]; warnings: ShockReportItem[]; derived: ShockDerived | null; views: View[] } {
		const errors: ShockReportItem[] = [];
		const warnings: ShockReportItem[] = [];
		const item = (reason: ShockReportItem['reason'], message: string, field: string): ShockReportItem => ({ reason, message, field });
		const p = config.pulse;
		if (!(p.amplitude_g > 0 && p.amplitude_g <= 2000)) errors.push(item('out_of_range', 'The amplitude is above 0 and at most 2000 g.', 'config.pulse.amplitude_g'));
		if (!(p.duration_ms >= 0.1 && p.duration_ms <= 100)) errors.push(item('out_of_range', 'The duration is from 0.1 to 100 ms.', 'config.pulse.duration_ms'));
		const s = config.schedule;
		if (!s.levels_db.length) errors.push(item('levels_empty', 'The schedule has no levels.', 'config.schedule.levels_db'));
		for (let i = 1; i < s.levels_db.length; i++) if (s.levels_db[i] < s.levels_db[i - 1]) errors.push(item('levels_not_ascending', 'Levels rise, or stay level, from one to the next.', `config.schedule.levels_db[${i}]`));
		if (s.levels_db.length && Math.abs(s.levels_db[s.levels_db.length - 1]) > 1e-9) errors.push(item('out_of_range', 'The last level is full level, 0 dB.', `config.schedule.levels_db[${s.levels_db.length - 1}]`));
		if (s.pulses.length !== 1 && s.pulses.length !== s.levels_db.length) errors.push(item('pulses_length_mismatch', 'Give one pulse count for every level, or one per level.', 'config.schedule.pulses'));
		const lim = config.limits;
		if (!(lim.drive_vpk_ceiling > 0 && lim.drive_vpk_ceiling <= 10)) errors.push(item('out_of_range', 'The drive ceiling is above 0 and at most 10 V pk.', 'config.limits.drive_vpk_ceiling'));
		if (!(lim.drive_vpk_abort > lim.drive_vpk_ceiling)) errors.push(item('out_of_range', 'The drive abort is above the drive ceiling.', 'config.limits.drive_vpk_abort'));
		const fs = config.acquisition?.fs_hz ?? 51200;
		const idBand = config.identification?.band_hz ?? [5, 5000];
		if (idBand[1] > 0.45 * fs) errors.push(item('band_above_grid', `The identification band's top edge is at most ${round(0.45 * fs, 0)} Hz at this rate.`, 'config.identification.band_hz'));
		const rows = host.rows();
		if (!rows.some((r) => r.enabled && r.role === 'control')) errors.push({ reason: 'no_control_channel', message: 'The channel table has no enabled control row.' });
		if (!rows.some((r) => r.enabled && r.role === 'drive')) errors.push({ reason: 'no_drive_channel', message: 'The channel table has no enabled drive row.' });
		const lawId = config.control?.law?.id ?? 'fd_correct';
		const law = laws().find((l) => l.id === lawId);
		if (!law) errors.push(item('law_unknown', `No control law "${lawId}" in the catalog.`, 'config.control.law.id'));
		else {
			for (const [k, v] of Object.entries(config.control?.law?.params ?? {})) {
				const decl = law.params.find((x) => x.key === k);
				if (!decl) errors.push(item('law_param_unknown', `The law declares no parameter "${k}".`, `config.control.law.params.${k}`));
				else if ((decl.kind === 'number' || decl.kind === 'integer') && (typeof v !== 'number' || v < decl.min || v > decl.max)) errors.push(item('law_param_invalid', `${decl.label} is from ${decl.min} to ${decl.max}.`, `config.control.law.params.${k}`));
			}
		}
		const spacing = Math.max(config.spacing?.cooling_s ?? 1, config.spacing?.settling_s ?? 0.5, config.spacing?.compute_s ?? 0.25);
		if (config.train && config.train.count > 1 && config.train.spacing_s < spacing) errors.push(item('spacing_below_floor', `A train's pulses are at least the time between pulses apart, ${spacing} s.`, 'config.train.spacing_s'));
		if (config.gunfire && 60 / config.gunfire.rounds_per_min < spacing) errors.push(item('spacing_below_floor', `Rounds ${round(60 / config.gunfire.rounds_per_min, 3)} s apart are closer than the time between pulses, ${spacing} s.`, 'config.gunfire.rounds_per_min'));
		(config.limits.monitors ?? []).forEach((m, i) => {
			if (!rows.some((r) => r.enabled && r.index === m.channel && r.role !== 'drive')) errors.push(item('limit_unknown_channel', `Channel ${m.channel} is not an enabled input of the channel table.`, `config.limits.monitors[${i}].channel`));
		});
		const facts = referenceFacts(p, fs);
		const qw = quietWindow(p, config.standard, facts.shoulder);
		if (!qw.inside) warnings.push(item('quiet_window_exceeded', `The compensation leaves the standard's quiet window: its worst deviation is ${round(qw.worst_frac * 100, 1)}% of the peak, over ${round(qw.limit_frac * 100, 0)}%.`, 'config.pulse.compensation'));
		if (errors.length) return { errors, warnings, derived: null, views: [] };
		const T = p.duration_ms / 1000;
		const comp = p.compensation ?? { pre_frac: 4, post_frac: 4, symmetric: true, shape: 'rectangular' };
		const total = T * (1 + comp.pre_frac + (comp.symmetric ? comp.pre_frac : comp.post_frac)) * 1.5;
		const len = 2 ** Math.ceil(Math.log2(Math.max(256, total * fs)));
		const derived: ShockDerived = {
			grid: { fs_hz: fs, record_len: len, actual: false },
			reference: { peak_g: facts.peak_g, velocity_change_ips: facts.velocity_change_ips, displacement_pk_pk_in: facts.displacement_pk_pk_in, main_start: facts.main_start, main_len: facts.main_len },
			quiet_window: qw,
			spacing_s: spacing,
			call_budget_s: round((config.control?.law_budget ?? 0.25) * spacing, 3),
			feasibility: config.feasibility
				? host.feasibilityExceeds
					? { verdict: 'exceeded', message: "The pulse's displacement exceeds the shaker's stroke." }
					: { verdict: 'pass', message: "Within the shaker's ratings." }
				: { verdict: 'not_evaluated', message: 'No shaker was given, so feasibility was not evaluated.' }
		};
		return { errors, warnings, derived, views: [pulseView(p, fs, config.standard, 1), ...kinematicViews(p, fs, 1)] };
	}

	// ---- running ----------------------------------------------------------------------------

	function newRun(config: ShockConfig, derived: ShockDerived, views: View[]): Run {
		const st = host.station();
		const law = laws().find((l) => l.id === (config.control?.law?.id ?? 'fd_correct')) ?? FD_CORRECT;
		const levels = [...config.schedule.levels_db];
		const pulsesPer = config.schedule.pulses.length === 1 ? levels.map(() => config.schedule.pulses[0]) : [...config.schedule.pulses];
		const startDb = config.schedule.mode === 'manual' ? (config.schedule.start_db ?? levels[0]) : levels[0];
		return {
			id: `s-${String(++runCount).padStart(4, '0')}`,
			config: structuredClone(config),
			derived,
			views,
			state: 'configured',
			phase: 'idle',
			station: st.ref,
			source: st.source,
			mode: config.schedule.mode,
			levels,
			pulsesPer,
			rung: Math.max(0, levels.findIndex((l) => Math.abs(l - startDb) < 1e-9)),
			levelDb: startDb,
			firedAtLevel: 0,
			passedAtLevel: 0,
			failsInRow: 0,
			error: -0.25,
			rows: [],
			lastViews: [],
			t: 0,
			phaseT: 0,
			nextAt: null,
			queue: null,
			stopAfterPulse: false,
			stopOutcome: 'operator_stop',
			stopBy: undefined,
			pending: null,
			identification: null,
			prediction: null,
			feasibility: null,
			end: null,
			alarm: false,
			timer: null,
			law,
			orphanAt: null
		};
	}

	function setPhase(r: Run, state: ShockRunState, phase: ShockPhase) {
		r.state = state;
		r.phase = phase;
		r.phaseT = 0;
		stateChanged();
	}

	function startLive() {
		if (liveTimer) return;
		liveTimer = setInterval(() => {
			if (!run || !LIVE.includes(run.state)) {
				if (liveTimer) clearInterval(liveTimer);
				liveTimer = null;
				return;
			}
			host.broadcast('shock.live', liveOf(run) as unknown as Record<string, unknown>);
		}, LIVE_MS);
	}

	function arm(r: Run, ack: string | undefined) {
		if (r.config.feasibility && host.feasibilityExceeds && !ack) {
			r.feasibility = { verdict: 'exceeded', message: "The pulse's displacement exceeds the shaker's stroke." };
			setPhase(r, 'blocked', 'feasibility_blocked');
			addLog('feasibility', 'feasibility exceeded; arming needs an acknowledgement');
			refuse('refused', 'feasibility_exceeded', "The pulse's displacement exceeds the shaker's stroke; arm with an acknowledgement to go on.", { verdict: 'exceeded' });
		}
		if (ack) addLog('override', `armed past feasibility: "${ack}"`);
		r.feasibility = r.derived.feasibility.verdict === 'exceeded' && ack ? r.derived.feasibility : r.derived.feasibility;
		r.derived = { ...r.derived, grid: { ...r.derived.grid, actual: true } };
		setPhase(r, 'armed', 'armed');
		addLog('station', `armed on ${r.station.class}; nothing driven`);
		startLive();
	}

	function start(r: Run) {
		setPhase(r, 'running', 'sysid');
		const id = r.config.identification ?? {};
		addLog('state', `identification at ${id.drive_vrms ?? 0.05} V rms for ${id.duration_s ?? 4} s`);
		if (r.timer) clearInterval(r.timer);
		r.timer = setInterval(() => tick(r), TICK_MS);
	}

	function finishIdentification(r: Run) {
		const id = r.config.identification ?? {};
		const vrms = id.drive_vrms ?? 0.05;
		const min = id.min_mean_coherence ?? 0.8;
		r.identification = { drive_vrms: vrms, drive_vpk: round(vrms * 4.2, 3), coherence_mean: 0.994, min_mean_coherence: min, passed: true };
		const fullVpk = round(0.02 * r.config.pulse.amplitude_g, 3);
		r.prediction = {
			drive_vpk: fullVpk,
			drive_vpk_ceiling: r.config.limits.drive_vpk_ceiling,
			drive_vpk_abort: r.config.limits.drive_vpk_abort,
			response_peak_g: round(r.config.pulse.amplitude_g * 1.005, 2),
			velocity_pk_ips: round(r.derived.reference.velocity_change_ips / 2, 2),
			displacement_pk_pk_in: r.derived.reference.displacement_pk_pk_in
		};
		addLog('gate', 'identification done: mean coherence 0.994 in the band');
		if (r.config.identification?.review ?? true) {
			setPhase(r, 'review', 'awaiting_review');
			addLog('state', 'holding for review');
		} else proceed(r);
	}

	function proceed(r: Run) {
		const p = r.prediction;
		if (p && p.drive_vpk > p.drive_vpk_abort) {
			end(r, 'aborted', 'aborted', { kind: 'drive_vpk', pulse: 0, at_level_db: 0, vpk: p.drive_vpk, abort_vpk: p.drive_vpk_abort, predicted: true });
			return;
		}
		addLog('note', `seeded ${r.law.name} ${r.law.version}`);
		r.nextAt = r.mode === 'auto' ? r.t + r.derived.spacing_s : null;
		setPhase(r, 'running', 'waiting');
	}

	function vpkAt(r: Run, levelDb: number): number {
		return (r.prediction?.drive_vpk ?? 0.4) * 10 ** (levelDb / 20);
	}

	function fire(r: Run, origin: ShockPulseRow['origin']) {
		const v = vpkAt(r, r.levelDb);
		const ceiling = r.config.limits.drive_vpk_ceiling;
		if (v > r.config.limits.drive_vpk_abort) {
			end(r, 'aborted', 'aborted', { kind: 'drive_vpk', pulse: r.rows.length + 1, at_level_db: r.levelDb, vpk: round(v), abort_vpk: r.config.limits.drive_vpk_abort, predicted: true });
			return;
		}
		r.pending = { origin, limitScale: v > ceiling ? ceiling / v : 1 };
		setPhase(r, 'running', 'firing');
	}

	function judge(r: Run) {
		const pend = r.pending ?? { origin: 'ladder' as const, limitScale: 1 };
		r.pending = null;
		const tol = STANDARD_TOL[r.config.standard?.name ?? 'mil_std_810_516'];
		const scale = 10 ** (r.levelDb / 20);
		const ratio = (1 + r.error) * pend.limitScale;
		const peak = r.config.pulse.amplitude_g * scale * ratio;
		const errPct = round((ratio - 1) * 100, 2);
		const envelope = Math.abs(ratio - 1) <= tol.amplitude;
		const velocity = Math.abs(ratio - 1) <= tol.velocity * 1.4;
		const passed = envelope && velocity;
		const row: ShockPulseRow = {
			index: r.rows.length,
			origin: pend.origin,
			scheduled_db: r.levels[r.rung] ?? r.levelDb,
			commanded_db: r.levelDb,
			limit_scale: round(pend.limitScale, 4),
			drive_vpk: round(vpkAt(r, r.levelDb) * pend.limitScale),
			peak_g: round(peak, 3),
			peak_ratio: round(ratio, 4),
			error_pct: Math.abs(errPct),
			velocity_change_ips: round(r.derived.reference.velocity_change_ips * scale * ratio, 2),
			velocity_change_ratio: round(ratio, 4),
			verdicts: { envelope, velocity, substitution: true },
			passed,
			srs: { bands: 21, passed_bands: passed ? 21 : 15 },
			at_s: round(r.t, 2),
			law: pend.origin === 'ladder' ? { used: true, miss: null, capped_bins: 0, peak_capped: false, call_ms: 1.3 } : null
		};
		r.rows.push(row);
		r.firedAtLevel += 1;
		if (passed) r.passedAtLevel += 1;
		r.failsInRow = passed ? 0 : r.failsInRow + 1;
		const p = r.config.pulse;
		const fs = r.derived.grid.fs_hz;
		const c = composite(p, fs, scale);
		const measured = new Float64Array(c.a.length);
		const drive = new Float64Array(c.a.length);
		for (let i = 0; i < c.a.length; i++) {
			measured[i] = c.a[i] * ratio + 0.01 * p.amplitude_g * scale * Math.sin(i * 1.7);
			drive[i] = (c.a[i] / (p.amplitude_g || 1)) * vpkAt(r, r.levelDb) * pend.limitScale;
		}
		const srs = r.config.acquisition?.srs ?? {};
		const pulse = pulseView(p, fs, r.config.standard, scale, measured);
		const dv = driveView(c.t_ms, drive);
		const srsV = srsView(p, scale, srs.band_tol_db ?? 3, [srs.f_lo_hz ?? 20, srs.f_hi_hz ?? 2000], ratio);
		// The event carries the Pulse, Drive and SRS views; `shock.pulse` adds the kinematics.
		const views: View[] = [pulse, dv, srsV];
		r.lastViews = [pulse, dv, ...kinematicViews(p, fs, scale, measured), srsV];
		r.views = views;
		pulseViews.set(row.index, r.lastViews);
		addLog('pulse', `pulse ${row.index + 1} at ${r.levelDb} dB: peak ${row.peak_g} g, ${passed ? 'pass' : `fail (${errPct}%)`}`);
		for (const c2 of host.connections()) c2.emit('shock.pulse', { run_id: r.id, pulse: row, views } as unknown as Record<string, unknown>);
		// the law closes most of the error each pulse
		if (pend.origin === 'ladder') r.error *= 0.3;
		publishData(r);
		const full = Math.abs(r.levelDb) < 1e-9;
		const abortEvery = r.config.standard?.tolerance_abort === 'every_level';
		const over = r.config.standard?.overshoot_abort ?? tol.amplitude * 2;
		if (ratio > 1 + over) {
			end(r, 'aborted', 'aborted', { kind: 'overshoot', pulse: row.index + 1, at_level_db: r.levelDb, peak_ratio: row.peak_ratio ?? 0, limit_ratio: 1 + over });
			return;
		}
		if (!passed && ratio > 1 && (full || abortEvery)) {
			end(r, 'aborted', 'aborted', { kind: 'tolerance', pulse: row.index + 1, at_level_db: r.levelDb, envelope_pass: envelope, velocity_change_pass: velocity, error_pct: Math.abs(errPct), worst_deviation_frac: Math.abs(ratio - 1) });
			return;
		}
		if (!passed) r.alarm = true;
		const maxPer = r.config.schedule.max_pulses_per_level ?? 5;
		const onPass = r.config.schedule.advance === 'on_pass';
		if (r.stopAfterPulse) {
			end(r, r.stopOutcome, 'stopped', null, r.stopBy);
			return;
		}
		if (r.queue && r.queue.left > 0) {
			r.queue.left -= 1;
			r.nextAt = r.t + r.queue.spacing;
			setPhase(r, 'running', 'waiting');
			return;
		}
		r.queue = null;
		if (r.mode === 'auto') {
			const need = r.pulsesPer[r.rung] ?? 1;
			const done = onPass ? r.passedAtLevel >= need : r.firedAtLevel >= need;
			if (onPass && !done && r.firedAtLevel >= maxPer) {
				end(r, 'not_converged', 'stopped', { kind: 'not_converged', pulse: row.index + 1, at_level_db: r.levelDb, limit: 'per_level', pulses: r.firedAtLevel });
				return;
			}
			if (done) {
				if (r.rung >= r.levels.length - 1) {
					end(r, 'completed', 'stopped', null);
					return;
				}
				r.rung += 1;
				r.levelDb = r.levels[r.rung];
				r.firedAtLevel = 0;
				r.passedAtLevel = 0;
				addLog('level', `level ${r.levelDb} dB`);
			}
			r.nextAt = r.t + r.derived.spacing_s;
			setPhase(r, 'running', 'waiting');
		} else {
			if (onPass && r.failsInRow >= maxPer) {
				end(r, 'not_converged', 'stopped', { kind: 'not_converged', pulse: row.index + 1, at_level_db: r.levelDb, limit: 'per_level', pulses: r.failsInRow });
				return;
			}
			r.nextAt = r.t + r.derived.spacing_s;
			setPhase(r, 'running', 'waiting');
		}
	}

	/** The pulse in flight finishes and nothing more fires. */
	function stopAfterPulse(r: Run, outcome: ShockOutcome, by?: string) {
		r.stopAfterPulse = true;
		r.stopOutcome = outcome;
		r.stopBy = by;
		r.state = 'stopping';
		r.phase = 'stopping';
		stateChanged();
	}

	function endMessage(outcome: ShockOutcome, trip: ShockTrip | null, by?: string): string {
		switch (outcome) {
			case 'completed':
				return 'The schedule finished.';
			case 'operator_stop':
				return `Stopped by ${by ?? 'the operator'}; the pulse in flight finished.`;
			case 'operator_abort':
				return `Aborted by ${by ?? 'the operator'}.`;
			case 'safe_stop':
				return `Nobody held control for ${host.timeoutS} s, so the run stopped.`;
			case 'not_converged':
				return trip?.kind === 'not_converged' ? `${trip.pulses} pulses at ${trip.at_level_db} dB without a pass.` : 'The iteration limit ended the run.';
			case 'aborted':
				return trip ? `A protection tripped: ${trip.kind.replaceAll('_', ' ')}.` : 'A protection tripped.';
			default:
				return outcome.replaceAll('_', ' ');
		}
	}

	function end(r: Run, outcome: ShockOutcome, phase: ShockPhase, trip: ShockTrip | null, by?: string) {
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		r.end = { outcome, message: endMessage(outcome, trip, by), at_s: round(r.t, 2), level_db: r.levelDb, pulse: r.rows.length ? r.rows.length : null, trip };
		r.nextAt = null;
		setPhase(r, 'finished', phase);
		addLog(outcome === 'aborted' ? 'trip' : 'state', `finished: ${outcome.replaceAll('_', ' ')}`);
		host.broadcast('shock.live', liveOf(r) as unknown as Record<string, unknown>);
	}

	function tick(r: Run) {
		if (run !== r || r.state === 'finished') return;
		const dt = (TICK_MS / 1000) * host.speed;
		// Nobody holds control: firing pauses; the clock of the safe stop is the seat's.
		if (!host.hasController() && r.phase === 'waiting') return;
		r.t += dt;
		r.phaseT += dt;
		switch (r.phase) {
			case 'sysid':
				if (r.phaseT >= SYSID_S) finishIdentification(r);
				break;
			case 'waiting':
				if (r.nextAt !== null && r.t >= r.nextAt && (r.mode === 'auto' || r.queue)) fire(r, r.queue ? r.queue.origin : 'ladder');
				break;
			case 'firing':
			case 'stopping':
				if (r.pending && r.phaseT >= FIRE_S) judge(r);
				break;
		}
	}

	// ---- display ------------------------------------------------------------------------------

	const pulseViews = new Map<number, View[]>();

	function pulsesTable(r: Run): TableView {
		return {
			view: 'table',
			title: 'Pulses',
			columns: ['Pulse', 'Level', 'Peak', 'Error', 'Result'],
			rows: r.rows.map((x) => [String(x.index + 1), `${x.commanded_db} dB`, `${x.peak_g ?? '-'} g`, `${x.error_pct ?? '-'}%`, x.passed ? 'Pass' : 'Fail'])
		};
	}

	function readings(r: Run): View {
		const rows: ReadingRow[] = [
			{ label: 'Pulses fired', value: r.rows.length, status: 'info' },
			{ label: 'Pulses passed', value: r.rows.filter((x) => x.passed).length, status: 'info' },
			{ label: 'Level', value: r.levelDb, unit: 'dB', status: 'info' },
			{ label: 'Predicted drive at full level', value: r.prediction?.drive_vpk ?? '-', unit: 'V pk', status: 'info' },
			{ label: 'Time between pulses', value: r.derived.spacing_s, unit: 's', status: 'info' }
		];
		return { view: 'reading', title: 'Run', rows };
	}

	function lawReading(r: Run): View {
		const last = r.rows.at(-1);
		return {
			view: 'reading',
			title: 'Control Law',
			rows: [
				{ label: 'Lines corrected', value: last ? 412 : '-', status: 'info' },
				{ label: 'Lines held back', value: last?.law?.capped_bins ?? '-', status: 'info' },
				{ label: 'Drive change', value: last ? round(20 * Math.log10(1 / Math.max(0.01, 1 + r.error)), 2) : '-', unit: 'dB', status: 'info' }
			]
		};
	}

	function plotViews(r: Run, q: ShockQuantity): View[] {
		const p = r.config.pulse;
		const fs = r.derived.grid.fs_hz;
		const scale = 10 ** (r.levelDb / 20);
		const last = (title: string) => r.lastViews.filter((v) => v.view === 'spectrum' && v.title === title);
		switch (q) {
			case 'pulse':
				return r.lastViews.length ? [...last('Pulse'), ...last('Drive')] : [pulseView(p, fs, r.config.standard, scale)];
			case 'velocity':
				return r.lastViews.length ? last('Velocity') : [kinematicViews(p, fs, scale)[0]];
			case 'displacement':
				return r.lastViews.length ? last('Displacement') : [kinematicViews(p, fs, scale)[1]];
			case 'srs': {
				const srs = r.config.acquisition?.srs ?? {};
				return r.lastViews.length ? last('SRS') : [srsView(p, scale, srs.band_tol_db ?? 3, [srs.f_lo_hz ?? 20, srs.f_hi_hz ?? 2000])];
			}
			case 'spectrum':
				return [spectrumView(p, scale, r.rows.at(-1)?.peak_ratio ?? 1)];
			case 'identification':
				return r.identification ? identificationViews(r.config.control?.band_hz ?? [5, 5000], 0.97, r.config.control?.coherence_floor ?? 0.9) : [];
			case 'law':
				return [lawReading(r)];
			case 'readings':
				return [readings(r)];
			case 'pulses':
				return [pulsesTable(r)];
		}
	}

	function dataFor(r: Run, plots: ShockSubscribePlot[]) {
		return { ...liveOf(r), plots: plots.map((pl) => ({ id: pl.id, quantity: pl.quantity, views: plotViews(r, pl.quantity) })) };
	}

	function publishData(r: Run) {
		for (const [conn, sub] of subs) conn.emit('shock.data', dataFor(r, sub.plots) as unknown as Record<string, unknown>);
	}

	// ---- refusals -----------------------------------------------------------------------------

	function needRun(): Run {
		if (!run) refuse('refused', 'not_configured', 'Classical shock is not configured; configure a test first.');
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

	function manualRunning(r: Run, verb: string) {
		if (r.mode !== 'manual') refuse('refused', 'wrong_mode', `${verb} is for the manual ladder; this run's schedule fires itself.`, { mode: r.mode });
		if (r.state !== 'running') wrongState(r, verb);
	}

	function readyToFire(r: Run, verb: string) {
		if (r.phase === 'firing') refuse('refused', 'not_yet', 'A pulse is playing.', { seconds: round(FIRE_S - r.phaseT, 2) });
		if (r.phase !== 'waiting') wrongState(r, verb);
		if (r.nextAt !== null && r.t < r.nextAt) refuse('refused', 'not_yet', `The time between pulses has not passed; the next pulse may fire in ${round(r.nextAt - r.t, 1)} s.`, { seconds: round(r.nextAt - r.t, 2) });
	}

	const lawsList = (params: Record<string, unknown>) => {
		const world = params.world;
		const all = laws();
		return { laws: world === undefined || world === null || world === 'shock_classical' ? all : [], problems: [] };
	};

	return {
		isLive,
		liveId: () => (run && isLive() ? run.id : null),
		detach: (conn) => {
			subs.delete(conn);
		},
		seatExpired: () => {
			if (run && isLive()) {
				if (run.phase === 'firing') stopAfterPulse(run, 'safe_stop');
				else end(run, 'safe_stop', 'stopped', null);
			}
		},
		methods: {
			'laws.list': (params) => lawsList(params),
			'laws.import': (params, conn) => {
				host.own(conn);
				const path = params.path;
				if (typeof path !== 'string' || !path.trim() || !/^(\/|[A-Za-z]:\\)/.test(path)) refuse('bad_request', 'bad_params', 'A law file is named by its absolute path on the computer that runs the test.', { param: 'path' });
				if (!/\.wasm$/i.test(path)) refuse('refused', 'law_import_failed', 'The file is not a law file (.wasm).');
				const law = importedLaw(path);
				if (!imported.some((l) => l.id === law.id)) imported = [...imported, law];
				return { law };
			},
			'shock.configure': (params, conn) => {
				if (!isObject(params.config)) refuse('bad_request', 'bad_params', 'Configuring needs a test configuration.', { param: 'config' });
				const config = params.config as unknown as ShockConfig;
				for (const key of ['pulse', 'schedule', 'limits'] as const) if (!isObject(config[key])) refuse('bad_request', 'bad_params', `the configuration has no "${key}".`, { param: `config.${key}` });
				const checked = check(config);
				if (params.dry_run === true) return checked;
				host.own(conn);
				notWhileLive('configuring');
				if (checked.errors.length || !checked.derived) refuse('bad_request', 'config_invalid', `The configuration has ${checked.errors.length} error(s).`, { errors: checked.errors, warnings: checked.warnings });
				if (run?.timer) clearInterval(run.timer);
				run = newRun(config, checked.derived, checked.views);
				pulseViews.clear();
				log = [];
				addLog('state', `configured "${config.title}" on ${run.station.class}`);
				stateChanged(true);
				return { state: stateOf(run), derived: checked.derived, warnings: checked.warnings };
			},
			'shock.config': () => (run ? { run_id: run.id, config: run.config, derived: run.derived, views: [pulseView(run.config.pulse, run.derived.grid.fs_hz, run.config.standard, 1), ...kinematicViews(run.config.pulse, run.derived.grid.fs_hz, 1)] } : { run_id: null, config: null, derived: null, views: [] }),
			'shock.preview': (params) => {
				if (!isObject(params.pulse)) refuse('bad_request', 'bad_params', 'A preview needs a pulse.', { param: 'pulse' });
				const p = params.pulse as unknown as ShockPulse;
				if (!(p.amplitude_g > 0) || !(p.duration_ms > 0)) refuse('bad_request', 'bad_params', 'The pulse needs an amplitude and a duration above 0.', { param: 'pulse.amplitude_g' });
				const fs = typeof params.fs_hz === 'number' ? params.fs_hz : 51200;
				const standard = (isObject(params.standard) ? params.standard : undefined) as ShockStandard | undefined;
				const facts = referenceFacts(p, fs);
				const { shoulder, ...reference } = facts;
				return { derived: { reference, quiet_window: quietWindow(p, standard, shoulder) }, views: [pulseView(p, fs, standard, 1), ...kinematicViews(p, fs, 1)] };
			},
			'shock.arm': (params, conn) => {
				host.own(conn);
				const r = needRun();
				const ack = params.override_ack;
				if (ack !== undefined && (typeof ack !== 'string' || !ack.trim())) refuse('bad_request', 'bad_params', "An override acknowledgement is the operator's own words; it cannot be empty.", { param: 'override_ack' });
				if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.', { outcome: r.end?.outcome });
				notWhileLive('arming again');
				notWhileOther();
				arm(r, ack as string | undefined);
				stateChanged();
				return stateOf(r);
			},
			'shock.start': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.', { outcome: r.end?.outcome });
				if (r.state !== 'configured' && r.state !== 'armed') wrongState(r, 'Starting');
				notWhileOther();
				if (r.state === 'configured') arm(r, undefined);
				start(r);
				return stateOf(r);
			},
			'shock.proceed': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				if (r.state !== 'review') wrongState(r, 'Proceeding');
				addLog('note', 'the operator reviewed identification and proceeded');
				proceed(r);
				return stateOf(r);
			},
			'shock.fire': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				manualRunning(r, 'Fire');
				readyToFire(r, 'Fire');
				fire(r, 'ladder');
				return stateOf(r);
			},
			'shock.level': (params, conn) => {
				host.own(conn);
				const r = needRun();
				if (r.mode !== 'manual') refuse('refused', 'wrong_mode', 'Level moves are for the manual ladder.', { mode: r.mode });
				if (r.state !== 'running' && r.state !== 'review') wrongState(r, 'A level move');
				const hasDelta = typeof params.delta_db === 'number' && Number.isFinite(params.delta_db);
				const hasRung = Number.isInteger(params.rung);
				if (hasDelta === hasRung) refuse('bad_request', 'bad_params', 'A level move gives either a step in dB or a level of the table.', { param: 'delta_db' });
				let target: number;
				if (hasRung) {
					const i = params.rung as number;
					if (i < 0 || i >= r.levels.length) refuse('bad_request', 'not_a_rung', `Level ${i} is not in the table (0 to ${r.levels.length - 1}).`, { levels_db: r.levels });
					target = r.levels[i];
				} else target = Math.min(r.levels[r.levels.length - 1], Math.max(r.levels[0], r.levelDb + (params.delta_db as number)));
				const vpk = vpkAt(r, target);
				if (vpk > r.config.limits.drive_vpk_abort) refuse('refused', 'drive_ceiling', `At ${target} dB the identified drive would be ${round(vpk)} V pk, over the drive abort of ${r.config.limits.drive_vpk_abort} V pk.`, { vpk: round(vpk), abort_vpk: r.config.limits.drive_vpk_abort, level_db: target });
				r.levelDb = round(target, 2);
				const i = r.levels.findIndex((l) => Math.abs(l - r.levelDb) < 1e-9);
				if (i >= 0) r.rung = i;
				r.firedAtLevel = 0;
				r.passedAtLevel = 0;
				r.failsInRow = 0;
				addLog('level', `level ${r.levelDb} dB`);
				stateChanged(true);
				return { level_db: r.levelDb };
			},
			'shock.train': (params, conn) => {
				host.own(conn);
				const r = needRun();
				manualRunning(r, 'A train');
				readyToFire(r, 'A train');
				const count = Number.isInteger(params.count) ? (params.count as number) : (r.config.train?.count ?? 1);
				const spacing = typeof params.spacing_s === 'number' ? Math.max(params.spacing_s, r.derived.spacing_s) : Math.max(r.config.train?.spacing_s ?? r.derived.spacing_s, r.derived.spacing_s);
				r.queue = { origin: 'train', left: Math.max(0, count - 1), spacing };
				addLog('note', `train of ${count} pulses, ${spacing} s apart`);
				fire(r, 'train');
				return stateOf(r);
			},
			'shock.burst': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				manualRunning(r, 'A burst');
				if (!r.config.gunfire) refuse('refused', 'wrong_state', 'This run has no gunfire burst configured.');
				readyToFire(r, 'A burst');
				const g = r.config.gunfire;
				r.queue = { origin: 'gunfire', left: Math.max(0, g.rounds - 1), spacing: 60 / g.rounds_per_min };
				addLog('note', `gunfire burst of ${g.rounds} rounds at ${g.rounds_per_min} per minute`);
				fire(r, 'gunfire');
				return stateOf(r);
			},
			'shock.stop': (_params, conn) => {
				if (run && isLive()) {
					const by = host.clientName(conn);
					addLog('note', `stop requested by ${by}`);
					if (run.phase === 'firing') stopAfterPulse(run, 'operator_stop', by);
					else end(run, 'operator_stop', 'stopped', null, by);
				}
				return stateOf(run);
			},
			'shock.abort': (_params, conn) => {
				if (run && isLive()) {
					const by = host.clientName(conn);
					addLog('note', `abort requested by ${by}`);
					end(run, 'operator_abort', 'aborted', null, by);
				}
				return stateOf(run);
			},
			'shock.reset': (_params, conn) => {
				host.own(conn);
				const r = needRun();
				notWhileLive('resetting');
				const checked = check(r.config);
				run = newRun(r.config, checked.derived ?? r.derived, checked.views);
				pulseViews.clear();
				log = [];
				addLog('state', `reset; configured "${r.config.title}" again`);
				stateChanged(true);
				return stateOf(run);
			},
			'shock.clear': (_params, conn) => {
				host.own(conn);
				notWhileLive('clearing');
				run = null;
				pulseViews.clear();
				log = [];
				stateChanged(true);
				return stateOf(null);
			},
			'shock.state': () => stateOf(run),
			'shock.log': (params) => {
				const since = typeof params.since === 'number' ? Math.max(0, Math.floor(params.since)) : 0;
				return { entries: log.slice(since), next: log.length };
			},
			'shock.pulses': (params) => {
				const since = typeof params.since === 'number' ? Math.max(0, Math.floor(params.since)) : 0;
				const rows = run?.rows ?? [];
				return { pulses: rows.slice(since), next: rows.length };
			},
			'shock.pulse': (params) => {
				const r = needRun();
				const i = params.index;
				if (!Number.isInteger(i) || (i as number) < 0 || (i as number) >= r.rows.length) refuse('bad_request', 'bad_params', `No pulse ${String(i)} in this run.`, { param: 'index' });
				return { pulse: r.rows[i as number], views: pulseViews.get(i as number) ?? [] };
			},
			'shock.subscribe': (params, conn) => {
				if (params.default === true) {
					subs.delete(conn);
					return { default: true, rate_hz: 5, plots: null, quantities: ['pulse', 'velocity', 'displacement', 'srs', 'spectrum', 'identification', 'law', 'readings', 'pulses'] };
				}
				if (!Array.isArray(params.plots)) refuse('bad_request', 'bad_params', 'A subscription lists its plots, or asks for the default.', { param: 'plots' });
				const plots = (params.plots as ShockSubscribePlot[]).filter((p) => typeof p.id === 'string');
				const rate = typeof params.rate_hz === 'number' ? params.rate_hz : 2;
				subs.set(conn, { rate, plots });
				if (run && run.state !== 'configured' && run.state !== 'unconfigured') {
					const r = run;
					setTimeout(() => {
						if (subs.has(conn) && run === r) conn.emit('shock.data', dataFor(r, plots) as unknown as Record<string, unknown>);
					}, 30);
				}
				return { default: false, rate_hz: rate, plots, quantities: ['pulse', 'velocity', 'displacement', 'srs', 'spectrum', 'identification', 'law', 'readings', 'pulses'] };
			}
		}
	};
}

