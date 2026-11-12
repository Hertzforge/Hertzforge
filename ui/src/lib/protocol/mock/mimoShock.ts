/**
 * MIMO Shock in mock mode (PROTOCOL.md, "MIMO Shock"): a made-up run that walks every state the
 * screens show. Configure and its dry run check what the protocol names; Start identifies, works
 * out each control's reachable floor and holds for review; the ladder climbs while the verdicts
 * improve pulse by pulse. The simulator's plant picks the story:
 *
 * - `shock-linear-4x4`: a clean run to completion, with one pulse whose arrivals spread past the
 *   window.
 * - `shock-jointed-4x4-moderate`: a control that needs a few pulses at a level, and a step the
 *   ladder halves before full level because the first pulses predict an over-test.
 * - `shock-jointed-4x4-severe`: control 2 grows at -6 dB. With the scaled aborts on, the pulse
 *   over its scaled line ends the run; with them off, it is flagged and the divergence watch
 *   ends the run.
 * - `shock-jointed-6x3` (or any run with more controls than drives): controls whose floors are
 *   above tolerance hold the run before any pulse until the operator acknowledges.
 *
 * A host option makes a run end on an absolute abort line at its second level.
 * Nothing here is a measurement: the numbers are made up so that every screen can be reached.
 */

import type { ChannelRow, DataSource, LogEntry, LogKind, RefusalReason, Series, SpectrumView, StationRef, View } from '../types';
import type {
	MimoShockConfig,
	MimoShockControlVerdict,
	MimoShockDerived,
	MimoShockEnd,
	MimoShockIdentificationFacts,
	MimoShockLawEntry,
	MimoShockOutcome,
	MimoShockPhase,
	MimoShockPulseRow,
	MimoShockQuantity,
	MimoShockReportItem,
	MimoShockReview,
	MimoShockRunState,
	MimoShockState,
	MimoShockSubscribePlot
} from '../mimoShock';
import { MockError, type MockConnection, type MockMethod } from './socket';

export interface MimoShockMockHost {
	speed: number;
	timeoutS: number;
	own(conn: MockConnection): void;
	hasController(): boolean;
	otherRun(): string | null;
	broadcast(topic: string, data: Record<string, unknown>): void;
	rows(): ChannelRow[];
	station(): { source: DataSource; ref: StationRef };
	clientName(conn: MockConnection): string;
	/** The run's second level ends on an absolute abort line. */
	absoluteAbort?: boolean;
}

export interface MimoShockMock {
	methods: Record<string, MockMethod>;
	isLive(): boolean;
	liveId(): string | null;
	detach(conn: MockConnection): void;
	seatExpired(): void;
	laws(): MimoShockLawEntry[];
}

const TICK_MS = 50;
const LIVE_MS = 200;
const SYSID_S = 1.5;
const FIRE_S = 0.25;
const LIVE: MimoShockRunState[] = ['armed', 'running', 'review', 'stopping'];

function refuse(code: 'refused' | 'bad_request', reason: string, message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason as RefusalReason, detail);
}

const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);

// ---------------------------------------------------------------------------------------------
// The law catalog of the shock_mimo world, as a service would list it

const builtIn = (id: string, name: string, summary: string, targets: ('waveform' | 'srs')[], params: MimoShockLawEntry['params'], diagnostics: MimoShockLawEntry['diagnostics']): MimoShockLawEntry => ({
	id,
	world: 'shock_mimo' as MimoShockLawEntry['world'],
	builtin: true,
	name,
	version: '0.1.4',
	author: 'Hertzforge',
	summary,
	sha256: null,
	check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: 'Built in: cleared for hardware.' },
	params,
	diagnostics,
	shapes: null,
	needs: null,
	targets
});

const LAWS: MimoShockLawEntry[] = [
	builtIn(
		'inverse',
		'inverse',
		'Builds every drive through the regularized inverse of the identified plant, correcting a share of the error between pulses.',
		['waveform', 'srs'],
		[
			{ key: 'gain', label: 'Correction gain', help: 'The share of the error each pulse corrects.', kind: 'number', unit: '', min: 0.05, max: 1, step: 0.05, default: 1 },
			{ key: 'lambda_rel', label: 'Regularization', help: 'How strongly the inverse is held back where the plant barely answers.', kind: 'number', unit: '', min: 0.00001, max: 0.1, step: 0.0005, default: 0.001 },
			{ key: 'refresh', label: 'Refresh the model', help: 'Move the identified plant toward what the last pulse showed.', kind: 'toggle', default: false }
		],
		[
			{ key: 'drive_rms_full_level', label: 'Drive rms at full level', unit: 'V', help: 'The drives at full level, rms.', layout: null },
			{ key: 'waveform_error', label: 'Waveform error', unit: '', help: 'The waveform error the law saw.', layout: null }
		]
	),
	builtIn(
		'shape',
		'shape',
		"Corrects each control's target from its SRS ratio, leaving the model's phase alone.",
		['srs'],
		[{ key: 'max_step_db', label: 'Largest change per pulse', help: 'The most any control\'s correction may change in one pulse.', kind: 'number', unit: 'dB', min: 1, max: 12, step: 0.5, default: 6 }],
		[{ key: 'weight', label: 'Weight', unit: '', help: 'The share of the SRS ratio taken this pulse.', layout: null }]
	),
	builtIn(
		'paired',
		'paired',
		"Pairs each control with one drive and scales that drive's spectrum by the wanted over measured magnitude. No plant model.",
		['waveform', 'srs'],
		[{ key: 'smoothing', label: 'Smoothing', help: 'Lines the ratio is smoothed over.', kind: 'integer', unit: 'lines', min: 1, max: 64, step: 1, default: 8 }],
		[{ key: 'ratio_db', label: 'Largest ratio', unit: 'dB', help: 'The largest correction applied.', layout: null }]
	)
];

// ---------------------------------------------------------------------------------------------
// Views, made up for the screen

const timeAxis = { label: 'Time', unit: 's', scale: 'linear' as const };
const freqAxis = { label: 'Natural frequency', unit: 'Hz', scale: 'log' as const };

function halfSine(t: number, T: number): number {
	return t >= 0 && t <= T ? Math.sin((Math.PI * t) / T) : 0;
}

function pulseView(channel: number, n: number, peak: number, durS: number, sign: number, delayS: number, errDb: number, band: number | null, measured: boolean): SpectrumView {
	const t = new Float64Array(n);
	const ref = new Float64Array(n);
	const meas = new Float64Array(n);
	const span = durS * 4;
	const scale = 10 ** (errDb / 20);
	for (let i = 0; i < n; i++) {
		t[i] = (i / (n - 1)) * span;
		const x = t[i] - durS;
		ref[i] = sign * peak * (halfSine(x - delayS, durS) - 0.08 * halfSine(x + durS, durS) - 0.08 * halfSine(x - durS - delayS, durS));
		meas[i] = sign * peak * scale * halfSine(x - delayS, durS) * (1 + 0.03 * Math.sin(i * 0.7));
	}
	const series: Series[] = [{ name: 'Target', role: 'reference', x: t, y: ref }];
	if (band !== null) {
		const hi = new Float64Array(n);
		const lo = new Float64Array(n);
		for (let i = 0; i < n; i++) {
			hi[i] = ref[i] + band * peak;
			lo[i] = ref[i] - band * peak;
		}
		series.push({ name: 'Tolerance high', role: 'tolerance', x: t, y: hi }, { name: 'Tolerance low', role: 'tolerance', x: t, y: lo });
	}
	if (measured) series.push({ name: 'Measured', role: 'measured', x: t, y: meas });
	return { view: 'spectrum', title: `Pulse ${channel}`, x: timeAxis, y: { label: 'Acceleration', unit: 'g', scale: 'linear' }, series, link: 'time', channel } as SpectrumView;
}

function srsView(channel: number, points: [number, number][], scale: number, tol: [number, number], errDb: number | null, f: [number, number]): SpectrumView {
	const n = 40;
	const x = new Float64Array(n);
	const ref = new Float64Array(n);
	const hi = new Float64Array(n);
	const lo = new Float64Array(n);
	const meas = new Float64Array(n);
	const at = (fr: number) => {
		if (fr <= points[0][0]) return points[0][1];
		for (let i = 1; i < points.length; i++) {
			const [f0, a0] = points[i - 1];
			const [f1, a1] = points[i];
			if (fr <= f1) return a0 * (a1 / a0) ** (Math.log(fr / f0) / Math.log(f1 / f0));
		}
		return points[points.length - 1][1];
	};
	for (let i = 0; i < n; i++) {
		const fr = f[0] * (f[1] / f[0]) ** (i / (n - 1));
		x[i] = fr;
		ref[i] = at(fr) * scale;
		hi[i] = ref[i] * 10 ** (tol[1] / 20);
		lo[i] = ref[i] * 10 ** (tol[0] / 20);
		meas[i] = ref[i] * 10 ** (((errDb ?? 0) * (0.5 + 0.5 * Math.sin(i / 4))) / 20);
	}
	const series: Series[] = [
		{ name: 'Target', role: 'reference', x, y: ref },
		{ name: 'Tolerance high', role: 'tolerance', x, y: hi },
		{ name: 'Tolerance low', role: 'tolerance', x, y: lo }
	];
	if (errDb !== null) series.push({ name: 'Measured', role: 'measured', x, y: meas });
	return { view: 'spectrum', title: `SRS ${channel}`, x: freqAxis, y: { label: 'Peak acceleration', unit: 'g', scale: 'log' }, series, link: 'frequency', channel } as SpectrumView;
}

function drivesView(drives: number[], vpk: number[], n: number, durS: number): SpectrumView {
	const series: Series[] = drives.map((ch, k) => {
		const t = new Float64Array(n);
		const y = new Float64Array(n);
		for (let i = 0; i < n; i++) {
			t[i] = (i / (n - 1)) * durS * 4;
			const x = t[i] - durS;
			y[i] = vpk[k] * (halfSine(x, durS) * Math.cos(k * 0.6) - 0.15 * Math.sin((2 * Math.PI * x) / durS) * Math.exp(-Math.abs(x) / durS));
		}
		return { name: `Drive ${ch}`, role: 'drive', x: t, y };
	});
	return { view: 'spectrum', title: 'Drives', x: timeAxis, y: { label: 'Drive', unit: 'V', scale: 'linear' }, series, link: 'time' };
}

function identificationViews(channel: number, drives: number[], controls: number[], floor: number): SpectrumView[] {
	const n = 120;
	const f = new Float64Array(n);
	for (let i = 0; i < n; i++) f[i] = 5 * (2500 / 5) ** (i / (n - 1));
	const mode = (fr: number, fn: number, q: number) => 1 / Math.sqrt((1 - (fr / fn) ** 2) ** 2 + (fr / (fn * q)) ** 2);
	const gain: Series[] = drives.map((d, k) => ({ name: `Drive ${d}`, role: 'measured', x: f, y: f.map((fr) => 20 * (0.6 * mode(fr, 180 + 40 * k, 4) + 0.3 * mode(fr, 620 + 30 * channel, 6)) * (k === (channel - 1) % drives.length ? 1 : 0.35)) }));
	const coh: Series[] = controls.map((c) => ({ name: `Control ${c}`, role: 'measured', x: f, y: f.map((fr, i) => Math.min(0.999, 0.94 + 0.05 * Math.sin(i / 9 + c) - (fr < 8 ? 0.4 : 0))) }));
	coh.push({ name: 'Floor', role: 'limit', x: f, y: new Float64Array(n).fill(floor) });
	const fx = { label: 'Frequency', unit: 'Hz', scale: 'log' as const };
	return [
		{ view: 'spectrum', title: `Identification Gain ${channel}`, x: fx, y: { label: '|H|', unit: 'g/V', scale: 'log' }, series: gain, link: 'frequency' },
		{ view: 'spectrum', title: 'Coherence', x: fx, y: { label: 'Coherence', unit: null, scale: 'linear' }, series: coh, link: 'frequency' }
	];
}

// ---------------------------------------------------------------------------------------------
// The run

interface Run {
	id: string;
	config: MimoShockConfig;
	derived: MimoShockDerived;
	state: MimoShockRunState;
	phase: MimoShockPhase;
	station: StationRef & { plant: string | null };
	source: DataSource;
	plant: string;
	controls: number[];
	drives: number[];
	levels: number[];
	rung: number | null;
	levelDb: number | null;
	atLevel: number;
	fullPassed: number;
	/** Each control's signed error the next pulse will show, dB. */
	errors: number[];
	risingRun: number;
	lastErr: number | null;
	rows: MimoShockPulseRow[];
	viewsByPulse: Map<number, View[]>;
	t: number;
	phaseT: number;
	nextAt: number | null;
	stopAfterPulse: boolean;
	identification: MimoShockIdentificationFacts | null;
	review: MimoShockReview | null;
	end: MimoShockEnd | null;
	law: MimoShockLawEntry;
	lawParams: Record<string, unknown>;
	halved: boolean;
	timer: ReturnType<typeof setInterval> | null;
}

function plantOf(c: MimoShockConfig): string {
	const nc = c.channels.controls.length;
	const nd = c.channels.drives.length;
	return c.simulator?.plant ?? (nc > nd ? 'shock-jointed-6x3' : 'shock-linear-4x4');
}

export function createMimoShockMock(host: MimoShockMockHost): MimoShockMock {
	let run: Run | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastKey = '';
	const subs = new Map<MockConnection, { rate: number; plots: MimoShockSubscribePlot[] }>();
	let liveTimer: ReturnType<typeof setInterval> | null = null;
	const isLive = () => run !== null && LIVE.includes(run.state);

	function addLog(kind: LogKind, text: string) {
		const e: LogEntry = { seq: log.length, at_unix: Date.now() / 1000, kind, text };
		log.push(e);
		if (run) host.broadcast('mimo_shock.log', { run_id: run.id, entries: [e] });
	}

	function counts(r: Run) {
		const at = r.levelDb === null ? [] : r.rows.filter((x) => Math.abs(x.level_db - (r.levelDb as number)) < 1e-9);
		const full = r.levelDb !== null && Math.abs(r.levelDb) < 1e-9;
		return { fired: r.rows.length, passed: r.rows.filter((x) => x.passed).length, at_level: at.length, of_level: r.levelDb === null ? 0 : full ? Math.max(r.config.schedule.max_pulses_per_level ?? 3, r.config.schedule.full_level_pulses ?? 3) : (r.config.schedule.max_pulses_per_level ?? 3) };
	}

	function stateOf(r: Run | null): MimoShockState {
		if (!r) {
			return {
				run_id: null, state: 'unconfigured', phase: 'idle', title: null, source: null, station: null, grid: null, clock: null,
				target: null, timing: null, verdict: null, mode: null, levels_db: null, level_db: null, rung: null,
				pulses: { fired: 0, passed: 0, at_level: 0, of_level: 0 }, next_pulse_s: null, elapsed_s: null,
				identification: null, review: null, last_pulse: null, law: null, end: null, scaled_aborts: true
			};
		}
		const started = r.state !== 'configured';
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.config.title,
			source: r.source,
			station: r.station,
			grid: { ...r.derived.grid, actual: started },
			clock: started ? 'shared' : null,
			target: r.config.target.kind,
			timing: r.config.timing?.mode ?? 'free',
			verdict: r.config.verdict ?? 'all',
			mode: r.config.schedule.mode,
			levels_db: r.levels,
			level_db: r.levelDb,
			rung: r.rung,
			pulses: counts(r),
			next_pulse_s: r.phase !== 'waiting' ? null : r.nextAt === null ? (r.config.schedule.mode === 'manual' ? 0 : null) : round(Math.max(0, r.nextAt - r.t), 2),
			elapsed_s: started ? round(r.t, 2) : null,
			identification: r.identification,
			review: r.review,
			last_pulse: r.rows.at(-1) ?? null,
			law: { id: r.law.id, name: r.law.name, version: r.law.version, sha256: r.law.sha256, params: r.lawParams, misses: 0 },
			end: r.end,
			scaled_aborts: r.config.control?.scaled_aborts ?? true
		};
	}

	function stateChanged(force = false) {
		const s = stateOf(run);
		const key = JSON.stringify({ ...s, next_pulse_s: null, elapsed_s: null });
		if (!force && key === lastKey) return;
		lastKey = key;
		host.broadcast('mimo_shock.state', s as unknown as Record<string, unknown>);
	}

	function liveOf(r: Run) {
		const s = stateOf(r);
		return { run_id: r.id, source: r.source, state: r.state, phase: r.phase, mode: s.mode, level_db: r.levelDb, rung: r.rung, pulses: s.pulses, next_pulse_s: s.next_pulse_s };
	}

	// ---- checking a configuration ---------------------------------------------------------------

	function check(c: MimoShockConfig): { errors: MimoShockReportItem[]; warnings: MimoShockReportItem[]; derived: MimoShockDerived | null; views: View[] } {
		const errors: MimoShockReportItem[] = [];
		const warnings: MimoShockReportItem[] = [];
		const item = (reason: MimoShockReportItem['reason'], message: string, field: string): MimoShockReportItem => ({ reason, message, field });
		const rows = host.rows();
		const nc = c.channels.controls.length;
		const nd = c.channels.drives.length;
		if (nc < 1 || nc > 16) errors.push(item('controls_out_of_range', 'From 1 to 16 controls.', 'config.channels.controls'));
		if (nd < 1 || nd > 8) errors.push(item('drives_out_of_range', 'From 1 to 8 drives.', 'config.channels.drives'));
		const named = [...c.channels.controls.map((x) => x.channel), ...c.channels.drives.map((x) => x.channel), ...(c.channels.limits ?? []).map((x) => x.channel)];
		if (new Set(named).size !== named.length) errors.push(item('duplicate_channel', 'A channel is named twice among the controls, drives and limits.', 'config.channels'));
		c.channels.controls.forEach((x, i) => {
			if (!rows.some((r) => r.index === x.channel && r.enabled && r.role === 'control')) errors.push(item('control_unknown_channel', `Channel ${x.channel} is not an enabled control row.`, `config.channels.controls[${i}].channel`));
		});
		const driveRows = rows.filter((r) => r.enabled && r.role === 'drive').map((r) => r.index);
		c.channels.drives.forEach((x, i) => {
			if (!driveRows.includes(x.channel)) errors.push(item('drive_unknown_channel', `Channel ${x.channel} is not an enabled drive row.`, `config.channels.drives[${i}].channel`));
			if (!(x.ceiling_vpk > 0 && x.ceiling_vpk <= 10)) errors.push(item('out_of_range', 'The ceiling is above 0 and at most 10 V pk.', `config.channels.drives[${i}].ceiling_vpk`));
			if (!(x.abort_vpk > x.ceiling_vpk && x.abort_vpk <= 10)) errors.push(item('out_of_range', 'The abort is above the ceiling and at most 10 V pk.', `config.channels.drives[${i}].abort_vpk`));
		});
		if (driveRows.length !== nd) errors.push(item('drive_unknown_channel', 'Every enabled drive row is a drive of the run, in the table\'s order.', 'config.channels.drives'));
		const t = c.target;
		if (t.kind === 'waveform') {
			if (t.pulses.length !== nc) errors.push(item('targets_length_mismatch', `${t.pulses.length} targets for ${nc} controls; give one target per control.`, 'config.target.pulses'));
			t.pulses.forEach((p, i) => {
				if (!(p.amplitude_g > 0 && p.amplitude_g <= 2000)) errors.push(item('out_of_range', 'The amplitude is above 0 and at most 2000 g.', `config.target.pulses[${i}].amplitude_g`));
				if (!(p.duration_ms >= 0.1 && p.duration_ms <= 100)) errors.push(item('out_of_range', 'The duration is from 0.1 to 100 ms.', `config.target.pulses[${i}].duration_ms`));
			});
			if (c.verdict === 'global') errors.push(item('verdict_needs_srs', 'The global verdict judges an SRS target; this target is a waveform.', 'config.verdict'));
		} else {
			if (t.controls.length !== nc) errors.push(item('targets_length_mismatch', `${t.controls.length} targets for ${nc} controls; give one target per control.`, 'config.target.controls'));
			if (!(t.bands.f_hi_hz > t.bands.f_lo_hz)) errors.push(item('bands_empty', 'The band grid holds no bands.', 'config.target.bands'));
		}
		const timing = c.timing ?? { mode: 'free' };
		if (timing.mode === 'hold' && (timing.delays_s?.length ?? 0) !== nc) errors.push(item('delays_length_mismatch', 'Give one delay for every control.', 'config.timing.delays_s'));
		const w = timing.window_s ?? 0.002;
		if (!(w > 0 && w <= 0.1)) errors.push(item('out_of_range', 'The window is above 0 and at most 100 ms.', 'config.timing.window_s'));
		const s = c.schedule;
		if (!s.levels_db.length) errors.push(item('levels_empty', 'The schedule has no levels.', 'config.schedule.levels_db'));
		for (let i = 1; i < s.levels_db.length; i++) if (s.levels_db[i] <= s.levels_db[i - 1]) errors.push(item('levels_not_ascending', 'The levels rise from one to the next.', `config.schedule.levels_db[${i}]`));
		const fs = c.acquisition?.fs_hz ?? 51200;
		const band = c.acquisition?.band_hz ?? [5, Math.min(5000, 0.4 * fs)];
		if (band[1] >= fs / 2) errors.push(item('band_above_grid', `The band's top edge is below half the rate, ${fs / 2} Hz.`, 'config.acquisition.band_hz'));
		const plant = plantOf(c);
		if (!['shock-linear-4x4', 'shock-jointed-4x4-moderate', 'shock-jointed-4x4-severe', 'shock-jointed-6x3'].includes(plant)) errors.push(item('plant_unknown', `No simulator plant "${plant}".`, 'config.simulator.plant'));
		const lawId = c.control?.law?.id ?? (t.kind === 'srs' ? 'shape' : 'inverse');
		const law = LAWS.find((l) => l.id === lawId);
		if (!law) errors.push(item('law_unknown', `No control law "${lawId}" in the catalog.`, 'config.control.law.id'));
		else {
			if (law.targets && !law.targets.includes(t.kind)) errors.push(item('law_wrong_target', `${law.name} does not accept a ${t.kind === 'srs' ? 'SRS' : 'pulse'} target.`, 'config.control.law.id'));
			for (const [k, v] of Object.entries(c.control?.law?.params ?? {})) {
				const decl = law.params.find((x) => x.key === k);
				if (!decl) errors.push(item('law_param_unknown', `The law declares no parameter "${k}".`, `config.control.law.params.${k}`));
				else if ((decl.kind === 'number' || decl.kind === 'integer') && (typeof v !== 'number' || v < decl.min || v > decl.max)) errors.push(item('law_param_invalid', `${decl.label} is from ${decl.min} to ${decl.max}.`, `config.control.law.params.${k}`));
			}
		}
		if (c.control?.scaled_aborts === false) warnings.push(item('out_of_range', 'The scaled abort lines are off: a pulse over its level\'s abort line is flagged and the run goes on without climbing. The absolute lines still stop the run.', 'config.control.scaled_aborts'));
		if (errors.length) return { errors, warnings, derived: null, views: [] };
		const record = c.acquisition?.record_len ?? 8192;
		const derived: MimoShockDerived = {
			grid: { fs_hz: fs, record_len: record, lines: record / 2 + 1, actual: false },
			controls: c.channels.controls.map((x, i) => ({ channel: x.channel, peak: t.kind === 'waveform' ? t.pulses[i]?.amplitude_g ?? 0 : 24 })),
			spacing_s: Math.max(c.spacing?.cooling_s ?? 1, c.spacing?.settling_s ?? 0.5, c.spacing?.compute_s ?? 0.25),
			call_budget_s: (c.control?.law_budget ?? 0.25) * Math.max(c.spacing?.cooling_s ?? 1, c.spacing?.settling_s ?? 0.5, c.spacing?.compute_s ?? 0.25)
		};
		return { errors, warnings, derived, views: targetViews(c, null) };
	}

	function targetViews(c: MimoShockConfig, r: Run | null): View[] {
		const out: View[] = [];
		const t = c.target;
		c.channels.controls.forEach((x, i) => {
			if (t.kind === 'waveform') {
				const p = t.pulses[i];
				if (p) out.push({ ...pulseView(x.channel, 256, p.amplitude_g, p.duration_ms / 1000, p.polarity === 'negative' ? -1 : 1, 0, 0, null, false), title: `Target ${x.channel}` } as View);
			} else {
				const tc = t.controls[i];
				const pts = tc?.target.kind === 'breakpoints' ? tc.target.points : [[t.bands.f_lo_hz, 10], [t.bands.f_hi_hz, 10]] as [number, number][];
				out.push({ ...pulseView(x.channel, 256, Math.max(...pts.map((q) => q[1])) / 2, 0.004, tc?.polarity === 'negative' ? -1 : 1, 0, 0, null, false), title: `Target ${x.channel}` } as View);
				out.push({ ...srsView(x.channel, pts, 1, t.tolerance_db ?? [-3, 3], null, [t.bands.f_lo_hz, t.bands.f_hi_hz]), title: `Target SRS ${x.channel}` } as View);
			}
		});
		void r;
		return out;
	}

	// ---- the pulses ---------------------------------------------------------------------------

	const tolOf = (r: Run) => (r.config.target.kind === 'srs' ? (r.config.target.tolerance_db?.[1] ?? 3) : 1.2);
	const absLineOf = (r: Run) => r.config.control?.abort_db ?? (r.config.target.kind === 'srs' ? 6 : 2.28);

	function pulseViews(r: Run, row: MimoShockPulseRow, points = 256): View[] {
		const out: View[] = [];
		const t = r.config.target;
		const scale = 10 ** (row.level_db / 20);
		const tol = t.kind === 'srs' ? null : 0.15;
		r.controls.forEach((ch, i) => {
			const v = row.controls[i];
			if (t.kind === 'waveform') {
				const p = t.pulses[i];
				out.push(pulseView(ch, points, p.amplitude_g * scale, p.duration_ms / 1000, p.polarity === 'negative' ? -1 : 1, v.delay_s, v.over_test_db, tol, true));
			} else {
				const tc = t.controls[i];
				const pts = tc.target.kind === 'breakpoints' ? tc.target.points : ([[t.bands.f_lo_hz, 10], [t.bands.f_hi_hz, 10]] as [number, number][]);
				out.push(pulseView(ch, points, (Math.max(...pts.map((q) => q[1])) / 2) * scale, 0.004, tc.polarity === 'negative' ? -1 : 1, v.delay_s, v.over_test_db, null, true));
				out.push(srsView(ch, pts, scale, t.tolerance_db ?? [-3, 3], v.over_test_db, [t.bands.f_lo_hz, t.bands.f_hi_hz]));
			}
		});
		out.push(drivesView(r.drives, row.drives.map((d) => d.vpk), points, t.kind === 'waveform' ? (t.pulses[0]?.duration_ms ?? 11) / 1000 : 0.004));
		return out;
	}

	/** The next level once a level has passed: the table's next, or a halved step on the moderate plant. */
	function nextLevel(r: Run): { db: number; rung: number | null; note: MimoShockPulseRow['step_note'] } {
		const i = r.rung ?? r.levels.findIndex((l) => l > (r.levelDb as number)) - 1;
		const planned = r.levels[Math.min(i + 1, r.levels.length - 1)];
		const from = r.levelDb as number;
		if (r.plant === 'shock-jointed-4x4-moderate' && !r.halved && Math.abs(planned - -3) < 1e-9) {
			r.halved = true;
			const taken = (from + planned) / 2;
			return { db: taken, rung: null, note: { from_db: from, planned_db: planned, taken_db: taken, predicted_planned_db: 1.6, predicted_taken_db: 0.9, tolerance_db: tolOf(r) } };
		}
		const rung = r.levels.findIndex((l) => Math.abs(l - planned) < 1e-9);
		return { db: planned, rung, note: null };
	}

	function firePulse(r: Run) {
		const level = r.levelDb as number;
		const tol = tolOf(r);
		const scaledOn = r.config.control?.scaled_aborts ?? true;
		const severe = r.plant === 'shock-jointed-4x4-severe';
		const n = r.rows.length;
		// the severe plant: control 2 grows once the level reaches -6 dB
		if (severe && level >= -6 - 1e-9 && r.errors.length > 1) r.errors[1] = Math.max(r.errors[1], 1.0) + 1.4;
		if (host.absoluteAbort && r.rung === 1) r.errors[0] = absLineOf(r) + 0.6;
		const windowS = r.config.timing?.window_s ?? 0.002;
		const spread = r.plant === 'shock-linear-4x4' && n === 1 ? windowS * 1.4 : 0.0002;
		const scaledLine = 2.0;
		const controls: MimoShockControlVerdict[] = r.controls.map((ch, i) => {
			const e = round(r.errors[i] ?? 0, 2);
			const delay = round(0.0002 + (i === r.controls.length - 1 ? spread : 0.0001 * i), 5);
			const reasons: MimoShockControlVerdict['reasons'] = [];
			if (Math.abs(e) > tol) reasons.push('tolerance');
			const held = r.config.timing?.mode === 'hold';
			const timingOk = !held || Math.abs(delay - (r.config.timing?.delays_s?.[i] ?? 0)) <= windowS;
			if (!timingOk) reasons.push('timing');
			const fullLine = absLineOf(r);
			return {
				channel: ch,
				passed: reasons.length === 0,
				reasons,
				error_db: round(-20 + Math.abs(e) * 4, 1),
				over_test_db: e,
				delay_s: delay,
				peak: round(20 * 10 ** (level / 20) * 10 ** (e / 20), 2),
				peak_ratio: round(10 ** (e / 20), 3),
				timing_ok: timingOk,
				polarity_ok: true,
				duration_ratio: r.config.target.kind === 'srs' ? 1.02 : null,
				duration_ok: true,
				energy_db: round(e * 0.8, 2),
				rms_duration_ratio: 1.01,
				velocity_ips: round(12 * 10 ** (level / 20), 2),
				displacement_pk_pk_in: round(0.18 * 10 ** (level / 20), 3),
				predicted_peak_ratio: round(10 ** ((e * 0.9) / 20), 3),
				srs: r.config.target.kind === 'srs' ? { worst_err_db: e, worst_band_hz: 400, bands: 16, passed_bands: Math.abs(e) > tol ? 13 : 16 } : null,
				scaled_abort_margin_db: round(scaledLine - e, 2),
				absolute_abort_margin_db: round(fullLine - (e + level), 2)
			};
		});
		const crossings = controls.filter((c) => (c.scaled_abort_margin_db ?? 1) < 0).map((c) => ({ channel: c.channel, over_db: round(-(c.scaled_abort_margin_db as number), 2) }));
		const passed = controls.every((c) => c.passed);
		const row: MimoShockPulseRow = {
			index: n,
			scheduled_db: level,
			level_db: level,
			passed,
			at_s: round(r.t, 2),
			drives: r.drives.map((ch, k) => ({ channel: ch, vpk: round(0.6 * 10 ** (level / 20) * (1 + 0.1 * k), 3), limit_scale: 1 })),
			controls,
			over_test_db: Math.max(...controls.map((c) => c.over_test_db)),
			global_srs_error_db: r.config.target.kind === 'srs' ? round(controls.reduce((a, c) => a + Math.abs(c.over_test_db), 0) / controls.length, 2) : null,
			arrival_spread_s: round(Math.max(...controls.map((c) => c.delay_s)) - Math.min(...controls.map((c) => c.delay_s)), 5),
			warnings: [],
			step_note: null,
			scaled_crossings: crossings,
			law: { used: true, miss: null, call_ms: 2.8, delays_s: null, values: r.law.diagnostics.map((d, k) => ({ key: d.key, value: round(0.5 / (n + 1 + k), 3) })) }
		};
		if (row.arrival_spread_s > windowS) row.warnings.push(`The controls' arrivals spread over ${round(row.arrival_spread_s * 1000, 2)} ms, past the ${round(windowS * 1000, 2)} ms window.`);
		if (r.config.verdict === 'global' && row.global_srs_error_db !== null) row.passed = row.global_srs_error_db <= tol;
		r.rows.push(row);
		// the law improves every control that is not growing
		r.errors = r.errors.map((e, i) => (severe && i === 1 && level >= -6 - 1e-9 ? e : round(e * 0.4, 3)));
		addLog('pulse', `Pulse ${n + 1} at ${level} dB: ${row.passed ? 'passed' : 'did not pass'}.`);

		// the abort lines
		const absolute = controls.find((c) => (c.absolute_abort_margin_db ?? 1) < 0);
		if (absolute) {
			finishPulse(r, row);
			return end(r, 'aborted', 'control_abort', `Control ${absolute.channel} went over its absolute abort line.`);
		}
		if (crossings.length) {
			if (scaledOn) {
				finishPulse(r, row);
				return end(r, 'aborted', 'control_abort_scaled', `Control ${crossings[0].channel} went over its abort line scaled to ${level} dB.`);
			}
			addLog('alarm', `Over the scaled abort line: ${crossings.map((x) => `control ${x.channel} by ${x.over_db} dB`).join(', ')}.`);
		}
		// the divergence watch, on the worst error at this level
		const worst = Math.max(...controls.map((c) => Math.abs(c.over_test_db)));
		r.risingRun = r.lastErr !== null && worst > r.lastErr + 0.1 ? r.risingRun + 1 : 0;
		r.lastErr = worst;
		if (r.risingRun >= Math.max(1, (r.config.control?.divergence_pulses ?? 3) - 1)) {
			finishPulse(r, row);
			return end(r, 'safe_stop', 'divergence', 'The waveform error rose on consecutive pulses at one level.');
		}
		// the ladder
		const full = Math.abs(level) < 1e-9;
		r.atLevel += 1;
		if (row.passed) {
			if (full) {
				r.fullPassed += 1;
				if (r.fullPassed >= (r.config.schedule.full_level_pulses ?? 3)) {
					finishPulse(r, row);
					return end(r, 'completed', null, 'Full level passed its pulses.');
				}
			} else {
				const next = nextLevel(r);
				row.step_note = next.note;
				r.levelDb = next.db;
				r.rung = next.rung;
				r.atLevel = 0;
				r.lastErr = null;
				r.risingRun = 0;
				r.errors = r.errors.map((e, i) => (i === 0 ? e + 0.6 : e + 0.2));
			}
		} else if (r.atLevel >= (r.config.schedule.max_pulses_per_level ?? 3) && !full) {
			finishPulse(r, row);
			return end(r, 'not_converged', 'not_converged', `${r.atLevel} pulses at ${level} dB without every control passing.`);
		}
		if (r.rows.length >= (r.config.control?.max_pulses ?? 100)) {
			finishPulse(r, row);
			return end(r, 'safe_stop', 'max_pulses', 'The run reached its cap on pulses.');
		}
		finishPulse(r, row);
		if (r.stopAfterPulse) return end(r, 'operator_stop', 'operator_stop', 'Stopped by the operator.');
		r.phase = 'waiting';
		r.phaseT = 0;
		r.nextAt = r.config.schedule.mode === 'manual' ? null : r.t + r.derived.spacing_s;
		stateChanged();
	}

	function finishPulse(r: Run, row: MimoShockPulseRow) {
		const views = pulseViews(r, row);
		r.viewsByPulse.set(row.index, views);
		host.broadcast('mimo_shock.pulse', { run_id: r.id, pulse: row, views } as unknown as Record<string, unknown>);
		sendData();
	}

	function end(r: Run, outcome: MimoShockOutcome, reason: string | null, message: string) {
		r.state = 'finished';
		r.phase = outcome === 'operator_abort' || outcome === 'aborted' ? 'aborted' : outcome === 'held' ? 'held' : 'stopped';
		const last = r.rows.at(-1);
		r.end = { outcome, reason, message, at_s: round(r.t, 2), level_db: last?.level_db ?? r.levelDb, pulse: last ? last.index : null };
		r.nextAt = null;
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		addLog('state', `The run ended: ${message}`);
		stateChanged(true);
		host.broadcast('mimo_shock.live', liveOf(r) as unknown as Record<string, unknown>);
		stopLive();
	}

	function identify(r: Run) {
		const nc = r.controls.length;
		const nd = r.drives.length;
		const ctl = r.config.identification ?? {};
		r.identification = {
			realizations: ctl.frames ?? Math.max(64, 10 * nd),
			drive_vrms: r.drives.map(() => ctl.drive_vrms ?? 0.05),
			drive_vpk: r.drives.map(() => round((ctl.drive_vrms ?? 0.05) * 3.6, 3)),
			coherence_mean: r.controls.map((_, i) => round(0.998 - 0.001 * i, 3)),
			coverage: r.controls.map(() => 0.999),
			trusted_lines: 1596,
			band_lines: 1596,
			condition_max: nc > nd ? 7.5 : 3.1,
			coherence_floor: ctl.coherence_floor ?? 0.5,
			passed: true
		};
		const short = nc > nd;
		const tol = tolOf(r);
		r.review = {
			controls: r.controls.map((ch, i) => {
				const floor = short && i % 2 === 0 ? round(tol + 1.2 + 4 * i, 1) : r.config.target.kind === 'srs' ? 1.1 : -31.2;
				return { channel: ch, reachable: short && i % 2 === 0 ? 0.92 : 0.9998, floor_db: floor, reachable_ok: !(short && i % 2 === 0), predicted_peak_ratio: null };
			}),
			hold: null,
			acknowledged: null
		};
		const bad = r.review.controls.filter((c) => !c.reachable_ok);
		if (bad.length && (r.config.verdict ?? 'all') === 'all') {
			const names = bad.map((c) => c.channel);
			const list = names.length === 1 ? `Control ${names[0]} cannot` : `Controls ${names.slice(0, -1).join(', ')} and ${names[names.length - 1]} cannot`;
			r.review.hold = { reason: 'unreachable_target', message: `${list} reach ${names.length === 1 ? 'its target' : 'their targets'}: the best reachable response is ${bad.map((c) => c.floor_db).join(' and ')} dB from it, outside the tolerance. Proceed with an acknowledgement, or judge the run by the global verdict.` };
		}
		addLog('state', 'Identification done.');
		if (r.review.hold || (ctl.review ?? true)) {
			r.state = 'review';
			r.phase = 'awaiting_review';
		} else seed(r);
		sendData();
		stateChanged(true);
	}

	function seed(r: Run) {
		r.state = 'running';
		r.phase = 'waiting';
		r.rung = 0;
		r.levelDb = r.levels[0];
		r.atLevel = 0;
		r.review = r.review ? { ...r.review, controls: r.review.controls.map((c) => ({ ...c, predicted_peak_ratio: 1.01 })) } : null;
		r.nextAt = r.config.schedule.mode === 'manual' ? null : r.t + 0.2;
		addLog('law', `The law ${r.law.name} was seeded at ${r.levelDb} dB.`);
	}

	function tick(r: Run) {
		const dt = (TICK_MS / 1000) * host.speed;
		r.t += dt;
		r.phaseT += dt;
		if (r.state === 'running' && r.phase === 'sysid' && r.phaseT >= SYSID_S) identify(r);
		else if ((r.state === 'running' || r.state === 'stopping') && r.phase === 'waiting' && r.nextAt !== null && r.t >= r.nextAt) {
			r.phase = 'firing';
			r.phaseT = 0;
			stateChanged();
		} else if (r.phase === 'firing' && r.phaseT >= FIRE_S) firePulse(r);
	}

	function startLive() {
		if (liveTimer) return;
		liveTimer = setInterval(() => {
			if (run && isLive()) host.broadcast('mimo_shock.live', liveOf(run) as unknown as Record<string, unknown>);
		}, LIVE_MS);
	}

	function stopLive() {
		if (liveTimer) clearInterval(liveTimer);
		liveTimer = null;
	}

	// ---- the display ----------------------------------------------------------------------------

	function tableOf(title: string, columns: string[], rows: string[][]): View {
		return { view: 'table', title, columns, rows } as View;
	}

	function plotViews(q: MimoShockQuantity, p: MimoShockSubscribePlot): View[] {
		const r = run;
		if (!r) return [];
		const last = r.rows.at(-1);
		const views = last ? (r.viewsByPulse.get(last.index) ?? []) : [];
		const fmt = (v: number | null | undefined, d = 2) => (v === null || v === undefined ? '' : String(round(v, d)));
		switch (q) {
			case 'pulse':
				return views.filter((v) => /^Pulse /.test((v as SpectrumView).title) && (p.channel === undefined || (v as SpectrumView & { channel?: number }).channel === p.channel));
			case 'srs':
				return views.filter((v) => /^SRS /.test((v as SpectrumView).title) && (p.channel === undefined || (v as SpectrumView & { channel?: number }).channel === p.channel));
			case 'drives':
				return views.filter((v) => (v as SpectrumView).title === 'Drives');
			case 'verdicts':
				return last ? [tableOf('Verdicts', ['Control', 'Passed', 'Failed on', 'Error (dB)', 'Over-test (dB)', 'Delay (ms)', 'Peak ratio'], last.controls.map((c) => [String(c.channel), c.passed ? 'pass' : 'fail', c.reasons.join(', '), fmt(c.error_db, 1), fmt(c.over_test_db), fmt(c.delay_s * 1000, 2), fmt(c.peak_ratio, 3)]))] : [];
			case 'ladder':
				return [tableOf('Ladder', ['Level (dB)', 'Fired', 'Passed', 'Now'], r.levels.map((l) => {
					const at = r.rows.filter((x) => Math.abs(x.level_db - l) < 1e-9);
					return [String(l), String(at.length), String(at.filter((x) => x.passed).length), r.levelDb !== null && Math.abs(r.levelDb - l) < 1e-9 ? 'in force' : ''];
				}))];
			case 'identification':
				return r.identification ? identificationViews(p.channel ?? r.controls[0], r.drives, r.controls, r.identification.coherence_floor) : [];
			case 'review':
				return r.review ? [tableOf('Reachable Floors', ['Control', 'Reachable', 'Floor (dB)', 'Reachable within tolerance', 'Predicted peak ratio'], r.review.controls.map((c) => [String(c.channel), fmt(c.reachable, 4), fmt(c.floor_db, 1), c.reachable_ok ? 'yes' : 'no', fmt(c.predicted_peak_ratio, 2)]))] : [];
			case 'law':
				return last ? [{ view: 'reading', title: 'Law', rows: last.law.values.map((v) => ({ label: r.law.diagnostics.find((d) => d.key === v.key)?.label ?? v.key, value: v.value, unit: r.law.diagnostics.find((d) => d.key === v.key)?.unit || null, status: 'info' })) } as View] : [];
			case 'readings': {
				const s = stateOf(r);
				return [{ view: 'reading', title: 'Run', rows: [
					{ label: 'Level', value: r.levelDb ?? 0, unit: 'dB', status: 'info' },
					{ label: 'Pulses fired', value: s.pulses.fired, unit: null, status: 'info' },
					{ label: 'Pulses passed', value: s.pulses.passed, unit: null, status: 'info' },
					{ label: 'Last over-test', value: last?.over_test_db ?? 0, unit: 'dB', status: last && !last.passed ? 'fail' : 'info' },
					{ label: 'Next pulse', value: s.next_pulse_s ?? 0, unit: 's', status: 'info' }
				] } as View];
			}
			case 'pulses':
				return [tableOf('Pulses', ['Pulse', 'Level (dB)', 'Passed', 'Over-test (dB)', 'Spread (ms)'], r.rows.map((x) => [String(x.index + 1), String(x.level_db), x.passed ? 'pass' : 'fail', fmt(x.over_test_db), fmt(x.arrival_spread_s * 1000, 2)]))];
		}
		return [];
	}

	function dataFor(conn: MockConnection) {
		const sub = subs.get(conn);
		if (!sub || !run) return;
		conn.emit('mimo_shock.data', { ...liveOf(run), plots: sub.plots.map((p) => ({ id: p.id, quantity: p.quantity, views: plotViews(p.quantity, p) })) });
	}

	function sendData() {
		for (const c of subs.keys()) dataFor(c);
	}

	const DEFAULT_PLOTS: MimoShockSubscribePlot[] = [
		{ id: 'pulse', quantity: 'pulse', points: 1024 },
		{ id: 'drives', quantity: 'drives', points: 1024 },
		{ id: 'verdicts', quantity: 'verdicts' },
		{ id: 'ladder', quantity: 'ladder' },
		{ id: 'readings', quantity: 'readings' }
	];
	const QUANTITIES: MimoShockQuantity[] = ['pulse', 'srs', 'drives', 'verdicts', 'ladder', 'identification', 'review', 'law', 'readings', 'pulses'];

	// ---- methods --------------------------------------------------------------------------------

	const needRun = (): Run => run ?? refuse('refused', 'wrong_state', 'No MIMO Shock run is configured.');
	const wrong = (r: Run, verb: string): never => refuse('refused', 'wrong_state', `${verb} does not apply while the run is ${r.state}.`, { state: r.state, phase: r.phase });

	function newRun(config: MimoShockConfig, derived: MimoShockDerived): Run {
		runCount += 1;
		const st = host.station();
		const plant = plantOf(config);
		const lawId = config.control?.law?.id ?? (config.target.kind === 'srs' ? 'shape' : 'inverse');
		const law = LAWS.find((l) => l.id === lawId) ?? LAWS[0];
		const params: Record<string, unknown> = {};
		for (const p of law.params) params[p.key] = config.control?.law?.params?.[p.key] ?? (p as { default: unknown }).default;
		return {
			id: `ms-${String(runCount).padStart(4, '0')}`,
			config,
			derived,
			state: 'configured',
			phase: 'idle',
			station: { ...st.ref, plant: st.ref.kind === 'sim' ? plant : null },
			source: st.source,
			plant,
			controls: config.channels.controls.map((c) => c.channel),
			drives: config.channels.drives.map((d) => d.channel),
			levels: [...config.schedule.levels_db],
			rung: null,
			levelDb: null,
			atLevel: 0,
			fullPassed: 0,
			errors: config.channels.controls.map((_, i) => (i === 1 ? 2.6 : i === 0 ? -1.8 : 1.4 - 0.3 * i)),
			risingRun: 0,
			lastErr: null,
			rows: [],
			viewsByPulse: new Map(),
			t: 0,
			phaseT: 0,
			nextAt: null,
			stopAfterPulse: false,
			identification: null,
			review: null,
			end: null,
			law,
			lawParams: params,
			halved: false,
			timer: null
		};
	}

	const methods: Record<string, MockMethod> = {
		'mimo_shock.configure': (params, conn) => {
			if (!isObject(params.config)) refuse('bad_request', 'bad_params', 'Give the configuration.', { param: 'config' });
			const config = params.config as unknown as MimoShockConfig;
			const dry = params.dry_run === true;
			if (!dry) host.own(conn);
			if (!dry && isLive()) wrong(run as Run, 'Configure');
			const res = check(config);
			if (dry) return res;
			if (res.errors.length) refuse('bad_request', 'config_invalid', `The configuration has ${res.errors.length} ${res.errors.length === 1 ? 'error' : 'errors'}.`, { errors: res.errors, warnings: res.warnings });
			if (run?.timer) clearInterval(run.timer);
			run = newRun(config, res.derived as MimoShockDerived);
			log = [];
			addLog('state', 'Configured.');
			stateChanged(true);
			return { state: stateOf(run), derived: res.derived, warnings: res.warnings };
		},
		'mimo_shock.config': () => ({ run_id: run?.id ?? null, config: run?.config ?? null, derived: run?.derived ?? null, views: run ? targetViews(run.config, run) : [] }),
		'mimo_shock.state': () => stateOf(run),
		'mimo_shock.arm': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'configured') wrong(r, 'Arm');
			const other = host.otherRun();
			if (other) refuse('refused', 'run_active', `Run ${other} holds the station.`, { run_id: other });
			r.state = 'armed';
			r.phase = 'armed';
			addLog('station', 'Armed: the station is open; nothing is driven.');
			startLive();
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_shock.start': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'configured' && r.state !== 'armed') wrong(r, 'Start');
			const other = host.otherRun();
			if (other) refuse('refused', 'run_active', `Run ${other} holds the station.`, { run_id: other });
			r.state = 'running';
			r.phase = 'sysid';
			r.phaseT = 0;
			r.timer = setInterval(() => tick(r), TICK_MS);
			startLive();
			addLog('state', 'Identification: band-limited random on every drive at once.');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_shock.proceed': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'review') wrong(r, 'Proceed');
			const ack = typeof params.acknowledge === 'string' ? params.acknowledge.trim() : '';
			if (r.review?.hold && !ack) refuse('refused', 'unreachable_target', r.review.hold.message, { controls: r.review.controls });
			if (r.review?.hold && ack) {
				r.review = { ...r.review, acknowledged: ack };
				addLog('override', `Proceeded past the unreachable hold: "${ack}"`);
			}
			seed(r);
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_shock.fire': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'running') wrong(r, 'Fire');
			if (r.config.schedule.mode !== 'manual') refuse('refused', 'wrong_mode', 'The ladder fires itself in auto.');
			if (r.phase !== 'waiting') refuse('refused', 'not_yet', 'A pulse is in flight.', { seconds: FIRE_S });
			r.phase = 'firing';
			r.phaseT = 0;
			stateChanged();
			return stateOf(r);
		},
		'mimo_shock.stop': () => {
			const r = needRun();
			if (!isLive()) wrong(r, 'Stop');
			if (r.state === 'armed' || r.state === 'review' || r.phase === 'sysid' || (r.phase === 'waiting' && r.state === 'running')) end(r, 'operator_stop', 'operator_stop', 'Stopped by the operator.');
			else {
				r.state = 'stopping';
				r.stopAfterPulse = true;
				stateChanged(true);
			}
			return stateOf(r);
		},
		'mimo_shock.abort': () => {
			const r = needRun();
			if (!isLive()) wrong(r, 'Abort');
			end(r, 'operator_abort', 'operator_abort', 'Aborted by the operator.');
			return stateOf(r);
		},
		'mimo_shock.reset': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (isLive()) wrong(r, 'New Run');
			run = newRun(r.config, r.derived);
			log = [];
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_shock.clear': (_p, conn) => {
			host.own(conn);
			if (isLive()) wrong(run as Run, 'Clear');
			run = null;
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_shock.log': (params) => {
			const since = typeof params.since === 'number' ? params.since : 0;
			return { entries: log.filter((e) => e.seq >= since), next: log.length };
		},
		'mimo_shock.pulses': (params) => {
			const since = typeof params.since === 'number' ? params.since : 0;
			const rows = run?.rows ?? [];
			return { pulses: rows.filter((x) => x.index >= since), next: rows.length };
		},
		'mimo_shock.pulse': (params) => {
			const r = needRun();
			const row = r.rows.find((x) => x.index === params.index);
			if (!row) refuse('bad_request', 'bad_params', `No pulse ${String(params.index)}.`, { param: 'index' });
			return { pulse: row, views: pulseViews(r, row as MimoShockPulseRow, typeof params.points === 'number' ? params.points : 1024) };
		},
		'mimo_shock.subscribe': (params, conn) => {
			let plots: MimoShockSubscribePlot[];
			let rate = 2;
			if (params.default === true) plots = DEFAULT_PLOTS;
			else {
				if (!Array.isArray(params.plots)) refuse('bad_request', 'bad_params', 'Give the plots, or ask for the default.', { param: 'plots' });
				plots = (params.plots as MimoShockSubscribePlot[]).map((p) => ({ ...p }));
				for (const p of plots) if (!QUANTITIES.includes(p.quantity)) refuse('bad_request', 'bad_params', `No quantity "${p.quantity}".`, { param: 'plots' });
				rate = typeof params.rate_hz === 'number' ? params.rate_hz : 2;
			}
			subs.set(conn, { rate, plots });
			queueMicrotask(() => dataFor(conn));
			return { default: params.default === true, rate_hz: rate, plots: plots.map((p) => ({ ...p, points: p.points ?? (p.quantity === 'pulse' || p.quantity === 'drives' ? 1024 : undefined) })), quantities: QUANTITIES };
		},
		'laws.list': () => ({ laws: LAWS, problems: [] })
	};

	return {
		methods,
		isLive,
		liveId: () => (isLive() && run ? run.id : null),
		detach: (conn) => {
			subs.delete(conn);
		},
		seatExpired: () => {
			if (run && isLive()) end(run, 'safe_stop', 'orphan', 'Nobody held control.');
		},
		laws: () => LAWS
	};
}
