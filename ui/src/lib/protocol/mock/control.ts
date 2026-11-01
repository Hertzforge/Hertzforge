/**
 * Mock control service.
 *
 * Two surfaces, sharing one run slot as the real service does:
 *
 * - `sim.*`: a fake random-vibration simulation that streams `random.display` at 5 Hz. One
 *   profile is built to abort, so the shell's abort path can be seen without hardware.
 * - The hardware and Random control methods (`session.*`, `station.*`, `channels.*`,
 *   `random.*`; schema/PROTOCOL.md, "Hardware and Random Control"): controller ownership with
 *   the safe stop of an unattended run, a station list with a simulator and an NI chassis, the
 *   channel table with revisions and named validation, and a believable closed-loop Random run
 *   (system identification, the coherence gate, the review hold, a soft start, the level ladder,
 *   full level, controlled stop, abort and protection trips) streaming `random.state`,
 *   `random.live` and `random.log`.
 *
 * Every value it produces is simulated. Its NI devices are driver-simulated unless a test asks
 * otherwise, so in the shell everything it sends is tagged `SIM`.
 */

import type {
	Alarm,
	BandDb,
	ChannelRow,
	ChannelsSetParams,
	ClockDomain,
	ControlParams,
	DataSource,
	Derived,
	FeasibilityVerdict,
	Grid,
	LadderMode,
	LiveChannel,
	LogEntry,
	LogKind,
	NiDevice,
	NiStation,
	RandomConfig,
	RandomLive,
	RandomOutcome,
	RandomPhase,
	RandomRunState,
	RandomState,
	ReadingRow,
	RefusalReason,
	ReportItem,
	RunState,
	SessionState,
	SimProfile,
	SimStation,
	SpectrumView,
	StationClass,
	StationList,
	StationRef,
	StationSelected,
	Trip,
	ValidationReport
} from '../types';
import { MockConnection, MockError, type MockServer } from './socket';
import { binEdges } from './bins';
import {
	breakpointArrays,
	frequencyLines,
	gauss,
	hashSeed,
	QUAL_SPEC,
	rms,
	rng,
	scaleSpec,
	sdofT2,
	specAt,
	specRms,
	WORKMANSHIP_SPEC,
	type Breakpoints
} from './synth';

export interface ControlServerOptions {
	/** Run seconds per real second for `random.*` runs (1 = real time). */
	speed?: number;
	/** Seconds an unattended live run, or a vanished controller's seat, is kept. */
	timeoutS?: number;
	/** Present the NI devices as physical (`HARDWARE`). Default: driver-simulated, tagged `SIM`. */
	niReal?: boolean;
	/** The NI-DAQmx runtime is installed. Default true. */
	niInstalled?: boolean;
	/** Run seconds after engaging at which a resonance near 420 Hz grows until the envelope abort trips. */
	abortDemoAtS?: number;
	/** Feasibility, when given, comes back exceeded, so the blocked path and the override can be seen. */
	feasibilityExceeds?: boolean;
	/** Band coherence system identification reaches (default 0.94). */
	coherence?: number;
}

// ---------------------------------------------------------------------------------------------
// sim.* (unchanged surface)

interface Profile extends SimProfile {
	spec: Breakpoints;
	/** Elapsed time (s) at which a resonance starts to grow past the abort limit. */
	growsAt?: number;
}

const PROFILES: Profile[] = [
	{ id: 'qual', title: 'Qualification Random', summary: '20 to 2000 Hz, 0.04 g²/Hz plateau, 6.06 g rms at 0 dB.', spec: QUAL_SPEC },
	{ id: 'workmanship', title: 'Workmanship Screen', summary: '20 to 2000 Hz, 0.04 g²/Hz plateau to 500 Hz.', spec: WORKMANSHIP_SPEC },
	{
		id: 'abort-demo',
		title: 'Abort Demonstration',
		summary: 'Qualification profile with a resonance near 420 Hz that grows until the abort limit trips.',
		spec: QUAL_SPEC,
		growsAt: 4
	}
];

const TOLERANCE_DB = 3;
const ABORT_DB = 6;
const FRAME_MS = 200;

interface SimRun {
	id: string;
	profile: Profile;
	targetDb: number;
	speed: number;
	elapsed: number;
	frames: number;
	timer: ReturnType<typeof setInterval>;
}

// ---------------------------------------------------------------------------------------------
// Hardware and Random control: constants and defaults

const HEARTBEAT_S = 5;
const TIMEOUT_S = 30;
const SIM_INPUTS = 8;
const SIM_OUTPUTS = 1;
/** The simulator's plant: two resonances, as the real service's. */
const PLANT: [number, number][] = [
	[200, 8],
	[550, 10]
];
const G_IN_PER_S2 = 386.089;
const MIN_LINES = 8;
const MAX_LOOPS_PER_FRAME = 20;
const GATE_S = 0.5;
const SOFT_START_DEFAULT_S = 5;

/** The protocol's control defaults (PROTOCOL.md, "Random Test Configuration"). */
const CONTROL_DEFAULTS = {
	composite: 'weighted_avg',
	weights: [] as { channel: number; weight: number }[],
	control_dof: 120,
	damping: 4,
	max_lines_out: 10,
	max_loops_out: 5,
	max_sigma: 5 as number | null,
	drive_ceiling_vrms: 10,
	drive_abort_vrms: null as number | null,
	sysid_level_db: -20,
	coherence_floor: 0.5,
	review_after_sysid: true,
	pretest: { averages: 4, max_vrms: 1 as number | null, min_vrms: 0.01 as number | null, shaped: false },
	soft_start_db: 9,
	soft_start_relax_db: 6,
	level_ramp_s: 3,
	shutdown_ramp_s: 3
};
type Control = typeof CONTROL_DEFAULTS;

function withDefaults(c: ControlParams | undefined): Control {
	const given = c ?? {};
	return { ...CONTROL_DEFAULTS, ...given, pretest: { ...CONTROL_DEFAULTS.pretest, ...(given.pretest ?? {}) } } as Control;
}

const NO_ABORT = { accel_g: null, velocity_ips: null, displacement_mil: null };

function newRow(index: number, description: string, role: ChannelRow['role'], units: string): ChannelRow {
	return {
		index,
		enabled: true,
		description,
		role,
		physical: null,
		interface: 'single_ended',
		iepe_bias_v: null,
		units,
		sensitivity_mv_per_eu: units === 'V' ? 1000 : 100,
		range_max: units === 'V' ? 10 : 100,
		polarity: 'positive',
		control_group: null,
		abort: { ...NO_ABORT }
	};
}

/** The scan the mock reports: one chassis, an input module and an output module. */
function scanDevices(real: boolean): NiDevice[] {
	const sn = (s: string) => (real ? s : '');
	const ch = (prefix: string) => [0, 1, 2, 3].map((k) => ({ id: `${prefix}${k}`, label: `${prefix}${k}` }));
	return [
		{ id: 'cDAQ1', product: 'cDAQ-9178', serial: sn('01C2D3E4'), simulated: !real, parent: null, slot: null, timing_family: 'cDAQ1', inputs: [], outputs: [] },
		{ id: 'cDAQ1Mod1', product: 'NI 9234', serial: sn('01A2B3C4'), simulated: !real, parent: 'cDAQ1', slot: 1, timing_family: 'cDAQ1', inputs: ch('ai'), outputs: [] },
		{ id: 'cDAQ1Mod2', product: 'NI 9263', serial: sn('01A2B3C5'), simulated: !real, parent: 'cDAQ1', slot: 2, timing_family: 'cDAQ1', inputs: [], outputs: ch('ao') }
	];
}

/** The input module's rate ladder: 51.2 kHz divided by 1 to 31. */
function coerceRate(fs: number): number {
	let best = 51200;
	for (let n = 1; n <= 31; n++) {
		const r = 51200 / n;
		if (Math.abs(r - fs) < Math.abs(best - fs)) best = r;
	}
	return best;
}

const fmt = (x: number, digits = 1) => `${x >= 0 ? '+' : ''}${x.toFixed(digits)}`;
const round = (x: number, digits: number) => Number(x.toFixed(digits));

function item(reason: ReportItem['reason'], message: string, where: Partial<ReportItem> = {}): ReportItem {
	return { reason, message, ...where };
}

function refuse(code: 'refused' | 'bad_request', reason: RefusalReason, message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason, detail);
}

// ---------------------------------------------------------------------------------------------
// The profile: anchors, power-law segments, rms, per-line bands

interface Resolved {
	/** [freq_hz, psd] per row, slopes resolved. */
	anchors: [number, number][];
	/** Each row's alarm and abort bands, the profile's where a row has none. */
	alarm: BandDb[];
	abort: BandDb[];
}

function resolveProfile(config: RandomConfig): Resolved {
	const rows = config.profile.breakpoints;
	const alarmDefault = config.profile.alarm_db ?? [3, 3];
	const abortDefault = config.profile.abort_db ?? [6, 6];
	const anchors: [number, number][] = [];
	for (const row of rows) {
		if (row.psd_g2_hz !== undefined) anchors.push([row.freq_hz, row.psd_g2_hz]);
		else {
			const [f0, p0] = anchors[anchors.length - 1];
			anchors.push([row.freq_hz, p0 * (row.freq_hz / f0) ** ((row.slope_db_oct ?? 0) / (10 * Math.log10(2)))]);
		}
	}
	return {
		anchors,
		alarm: rows.map((r) => r.alarm_db ?? alarmDefault),
		abort: rows.map((r) => r.abort_db ?? abortDefault)
	};
}

/** The band of the segment a line sits in: its lower row's own, else the profile's. */
function bandAt(resolved: Resolved, bands: BandDb[], f: number): BandDb {
	let i = 0;
	while (i + 1 < resolved.anchors.length && resolved.anchors[i + 1][0] <= f) i++;
	return bands[Math.min(i, Math.max(0, resolved.anchors.length - 2))] ?? bands[0];
}

/** Velocity (in/s) and displacement (mil) rms of an acceleration PSD, integrated on a log grid. */
function kinematics(spec: Breakpoints): { velocity: number; displacement: number } {
	const f0 = spec[0][0];
	const f1 = spec[spec.length - 1][0];
	const n = 4000;
	let v = 0;
	let d = 0;
	for (let i = 0; i < n; i++) {
		const fa = f0 * (f1 / f0) ** (i / n);
		const fb = f0 * (f1 / f0) ** ((i + 1) / n);
		const f = Math.sqrt(fa * fb);
		const g = specAt(spec, f) * (fb - fa);
		const w = 2 * Math.PI * f;
		v += (g * G_IN_PER_S2 ** 2) / w ** 2;
		d += (g * G_IN_PER_S2 ** 2) / w ** 4;
	}
	return { velocity: Math.sqrt(v), displacement: Math.sqrt(d) * 1000 };
}

function plantT2(f: number): number {
	return PLANT.reduce((acc, [fn, q]) => acc + sdofT2(f, fn, q), 0) / PLANT.length;
}

// ---------------------------------------------------------------------------------------------
// Validation (the item reasons of PROTOCOL.md, "Refusals")

interface Selection {
	kind: 'sim' | 'ni';
	devices: string[];
}

function validateTable(rows: ChannelRow[], selection: Selection, scan: NiDevice[]): ValidationReport {
	const errors: ReportItem[] = [];
	const warnings: ReportItem[] = [];
	const seen = new Set<number>();
	let anyEnabled = false;
	let control = 0;
	let reference = 0;
	for (const r of rows) {
		if (seen.has(r.index)) errors.push(item('duplicate_index', `duplicate channel index ${r.index}`, { channel: r.index }));
		seen.add(r.index);
		if (r.role === 'control') control++;
		if (r.role === 'reference') reference++;
		if (r.enabled) {
			anyEnabled = true;
			if (r.role === 'drive') {
				warnings.push(item('drive_channel_present', `channel ${r.index} is a drive output (sensitivity/range are advisory)`, { channel: r.index }));
			} else {
				if (r.sensitivity_mv_per_eu === 0) errors.push(item('zero_sensitivity', `channel ${r.index} has zero sensitivity`, { channel: r.index }));
				if (!(r.range_max > 0)) errors.push(item('non_positive_range', `channel ${r.index} has non-positive range`, { channel: r.index }));
			}
		}
		if (r.interface === 'iepe' && r.iepe_bias_v === null) {
			warnings.push(item('iepe_without_bias', `channel ${r.index} is IEPE but has no bias voltage set`, { channel: r.index }));
		}
		if (r.interface === 'single_ended' && r.iepe_bias_v !== null) {
			warnings.push(item('bias_on_single_ended', `channel ${r.index} is single-ended but has an IEPE bias set (ignored)`, { channel: r.index }));
		}
	}
	if (!anyEnabled) errors.push(item('no_channel_enabled', 'no channels are enabled', { channel: null }));
	const groups = [...new Set(rows.flatMap((r) => (r.control_group === null ? [] : [r.control_group])))].sort((a, b) => a - b);
	for (let g = 0; g < groups.length; g++) {
		if (groups[g] !== g) {
			errors.push(item('control_group_gap', `control-group ids are not contiguous (missing group ${g})`, { channel: null }));
			break;
		}
	}
	if (reference > 0 && control === 0) {
		errors.push(item('reference_without_control', 'reference channels exist but no control channel closes the loop', { channel: null }));
	}
	if (selection.kind === 'ni') {
		const used = new Map<string, number>();
		for (const r of rows.filter((x) => x.enabled)) {
			const where = { channel: r.index };
			if (!r.physical) {
				errors.push(item('channel_unbound', `channel ${r.index} is not bound to a physical channel; on NI hardware every enabled channel must be bound to one`, where));
				continue;
			}
			const key = `${r.physical.device}/${r.physical.channel}`;
			const device = scan.find((d) => d.id === r.physical?.device && selection.devices.includes(d.id));
			const isInput = device?.inputs.some((c) => c.id === r.physical?.channel) ?? false;
			const isOutput = device?.outputs.some((c) => c.id === r.physical?.channel) ?? false;
			if (!isInput && !isOutput) {
				errors.push(item('physical_channel_unknown', `channel ${r.index} is bound to ${key}, which the selected devices do not have`, where));
			} else if ((r.role === 'drive') !== isOutput) {
				errors.push(
					item('physical_channel_wrong_direction', `channel ${r.index} is ${r.role === 'drive' ? 'a drive output' : 'an input'} bound to ${key}, which is ${isOutput ? 'an output' : 'an input'}`, where)
				);
			}
			const other = used.get(key);
			if (other !== undefined) {
				errors.push(item('physical_channel_shared', `channel ${r.index} is bound to ${key}, which channel ${other} already uses; channels are never shared`, where));
			} else used.set(key, r.index);
		}
	}
	return { errors, warnings };
}

interface Checked {
	errors: ReportItem[];
	warnings: ReportItem[];
	derived: Derived | null;
}

function gridFor(config: RandomConfig, band: [number, number], fs: number, actual: boolean): Grid {
	const { n_pts, n_avg } = config.acquisition;
	const df = fs / n_pts;
	let lines = 0;
	for (let k = 1; k * df <= fs / 2; k++) if (k * df >= band[0] && k * df <= band[1]) lines++;
	return { fs_hz: round(fs, 4), n_pts, df_hz: round(df, 6), lines, loop_s: round((n_avg * n_pts) / fs, 4), actual };
}

function checkConfig(config: RandomConfig, rows: ChannelRow[]): Checked {
	const errors: ReportItem[] = [];
	const warnings: ReportItem[] = [];
	const range = (field: string, value: number | null | undefined, ok: (v: number) => boolean, text: string) => {
		if (value !== undefined && value !== null && !ok(value)) errors.push(item('out_of_range', `${field} is ${value}; ${text}`, { field }));
	};

	// profile
	const bp = config.profile.breakpoints ?? [];
	if (bp.length === 0) errors.push(item('profile_empty', 'breakpoint table is empty', { field: 'profile.breakpoints' }));
	bp.forEach((row, i) => {
		const anchor = row.psd_g2_hz !== undefined;
		const slope = row.slope_db_oct !== undefined;
		if (anchor === slope) errors.push(item('row_level_ambiguous', `row ${i}: give a PSD or a slope, not ${anchor ? 'both' : 'neither'}`, { row: i }));
		if (i === 0 && !anchor) errors.push(item('first_row_not_anchor', 'first breakpoint row must be a PSD anchor, not a slope', { row: 0 }));
		if (!(row.freq_hz > 0)) errors.push(item('frequency_not_positive', `row ${i}: frequency ${row.freq_hz} Hz must be strictly positive`, { row: i }));
		if (i > 0 && !(row.freq_hz > bp[i - 1].freq_hz)) {
			errors.push(item('frequency_not_ascending', `row ${i}: frequency ${row.freq_hz} Hz is not strictly greater than the previous row`, { row: i }));
		}
		if (anchor && (row.psd_g2_hz as number) < 0) errors.push(item('psd_negative', `row ${i}: PSD anchor ${row.psd_g2_hz} g²/Hz must not be negative`, { row: i }));
		const alarm = row.alarm_db ?? config.profile.alarm_db ?? [3, 3];
		const abort = row.abort_db ?? config.profile.abort_db ?? [6, 6];
		if (alarm[0] > abort[0] || alarm[1] > abort[1]) {
			warnings.push(item('alarm_wider_than_abort', `row ${i}: alarm band is wider than the abort band (alarm should trip first)`, { row: i }));
		}
	});

	// schedule
	const levels = config.schedule.levels ?? [];
	if (levels.length === 0) errors.push(item('levels_empty', 'the level schedule is empty', { field: 'schedule.levels' }));
	for (let i = 1; i < levels.length; i++) {
		if (levels[i].db < levels[i - 1].db) {
			errors.push(item('levels_not_ascending', `level ${i} (${levels[i].db} dB) is below the level before it`, { field: `schedule.levels.${i}` }));
		}
	}
	levels.slice(0, -1).forEach((l, i) => {
		if (l.dwell_s === undefined) errors.push(item('dwell_missing', `level ${i} (${l.db} dB) has no dwell_s; every level but the last needs one`, { field: `schedule.levels.${i}` }));
	});

	// control
	const c = withDefaults(config.control);
	range('control.damping', c.damping, (v) => v >= 0 && v <= 10, 'it runs from 0 to 10');
	range('control.coherence_floor', c.coherence_floor, (v) => v >= 0 && v <= 1, 'it runs from 0 to 1');
	range('control.drive_ceiling_vrms', c.drive_ceiling_vrms, (v) => v > 0, 'it must be above 0');
	range('control.drive_abort_vrms', c.drive_abort_vrms, (v) => v > 0, 'it must be above 0, or null for none');
	range('control.max_loops_out', c.max_loops_out, (v) => Number.isInteger(v) && v >= 1, 'it must be a whole number of at least 1');
	range('control.max_lines_out', c.max_lines_out, (v) => Number.isInteger(v) && v >= 0, 'it must be a whole number of at least 0');
	range('control.control_dof', c.control_dof, (v) => v >= 2, 'it must be at least 2');
	range('control.sysid_level_db', c.sysid_level_db, (v) => v >= -60 && v <= 0, 'it runs from -60 to 0 dB');
	range('control.max_sigma', c.max_sigma, (v) => v >= 1, 'it must be at least 1, or null for no clip');
	range('control.pretest.averages', c.pretest.averages, (v) => Number.isInteger(v) && v >= 1, 'it must be a whole number of at least 1');
	for (const key of ['soft_start_db', 'soft_start_relax_db', 'level_ramp_s', 'shutdown_ramp_s'] as const) {
		range(`control.${key}`, c[key], (v) => v >= 0, 'it must not be negative');
	}
	const controlRows = rows.filter((r) => r.enabled && r.role === 'control');
	if (controlRows.length === 0) errors.push(item('no_control_channel', 'the channel table has no enabled control channel', { field: 'control' }));
	for (const w of c.weights) {
		if (!controlRows.some((r) => r.index === w.channel)) {
			errors.push(item('weight_unknown_channel', `a weight names channel ${w.channel}, which is not an enabled control channel`, { channel: w.channel }));
		}
	}
	const weightSum = controlRows.reduce((s, r) => s + (c.weights.find((w) => w.channel === r.index)?.weight ?? 1), 0);
	if (controlRows.length > 0 && !(weightSum > 0)) errors.push(item('weights_sum_not_positive', 'the control weights sum to zero or less', { field: 'control.weights' }));

	// limits
	(config.limits ?? []).forEach((lim, i) => {
		if (!rows.some((r) => r.enabled && r.role !== 'drive' && r.index === lim.channel)) {
			errors.push(item('limit_unknown_channel', `limit ${i} names channel ${lim.channel}, which is not an enabled input`, { channel: lim.channel }));
		}
		for (let k = 1; k < lim.rows.length; k++) {
			if (!(lim.rows[k].freq_hz > lim.rows[k - 1].freq_hz)) {
				errors.push(item('limit_rows_not_ascending', `limit ${i}: row ${k} is not above the row before`, { field: `limits.${i}.rows.${k}` }));
				break;
			}
		}
	});

	// acquisition
	const { fs_hz, n_pts } = config.acquisition;
	if (!(n_pts > 0 && (n_pts & (n_pts - 1)) === 0)) errors.push(item('n_pts_not_power_of_two', `n_pts ${n_pts} is not a power of two`, { field: 'acquisition.n_pts' }));
	if (errors.length > 0) return { errors, warnings, derived: null };

	const band: [number, number] = [bp[0].freq_hz, bp[bp.length - 1].freq_hz];
	const top = fs_hz / 2.56;
	if (band[1] > top) {
		errors.push(
			item('band_above_grid', `the profile reaches ${band[1]} Hz but the grid at ${fs_hz} Hz resolves only to ${top.toFixed(1)} Hz; raise fs_hz or lower the top breakpoint`, {
				field: 'acquisition.fs_hz'
			})
		);
	}
	const grid = gridFor(config, band, fs_hz, false);
	if (grid.lines < MIN_LINES) {
		errors.push(item('too_few_lines', `${grid.lines} lines fall in ${band[0]} to ${band[1]} Hz at ${grid.df_hz} Hz spacing; at least ${MIN_LINES} are needed`, { field: 'acquisition.n_pts' }));
	}
	if (errors.length > 0) return { errors, warnings, derived: null };

	const spec = resolveProfile(config).anchors;
	const k = kinematics(spec);
	return {
		errors,
		warnings,
		derived: {
			grms_g: round(specRms(spec), 3),
			velocity_rms_ips: round(k.velocity, 3),
			displacement_rms_mil: round(k.displacement, 2),
			band_hz: band,
			grid,
			control_channels: controlRows.map((r) => r.index)
		}
	};
}

// ---------------------------------------------------------------------------------------------
// One Random run

interface Lines {
	f: Float64Array;
	edges: Float64Array;
	/** Reference at 0 dB, and each line's bands in dB. */
	ref: Float64Array;
	alarm: BandDb[];
	abort: BandDb[];
}

interface RandomRun {
	id: string;
	config: RandomConfig;
	ctl: Control;
	derived: Derived;
	state: RandomRunState;
	phase: RandomPhase;
	mode: LadderMode;
	station: StationRef;
	source: DataSource;
	stationClass: StationClass;
	grid: Grid;
	clock: ClockDomain | null;
	feasibility: FeasibilityVerdict | null;
	end: { outcome: RandomOutcome; message: string; at_s: number | null; level_db: number | null; trip: Trip | null } | null;
	lines: Lines | null;
	/** Run seconds since start, and in the present phase. */
	t: number | null;
	phaseT: number;
	engagedT: number;
	/** The level the loop runs at now, the commanded level it walks to, and the walk rate (dB/s). */
	level: number;
	target: number;
	rate: number;
	levelIndex: number;
	atLevelT: number;
	stopOutcome: RandomOutcome;
	/** The client that stopped or aborted the run, named in the log and the end message. */
	endedBy: string | null;
	loops: number;
	loopAcc: number;
	coherence: number;
	overload: Set<number>;
	alarmLines: number;
	abortLines: number;
	worstAlarmDb: number;
	controlPsd: Float64Array | null;
	drivePsd: Float64Array | null;
	controlRms: number | null;
	driveRms: number | null;
	channelRms: Map<number, number>;
	timer: ReturnType<typeof setInterval> | null;
}

const LIVE_STATES: RandomRunState[] = ['armed', 'running', 'review', 'stopping'];
const DRIVING: RandomPhase[] = ['engaged', 'full_level'];

export function createControlServer(options: ControlServerOptions = {}): MockServer {
	const speed = options.speed ?? 1;
	const timeoutS = options.timeoutS ?? TIMEOUT_S;
	const niReal = options.niReal ?? false;
	const niInstalled = options.niInstalled ?? true;
	const connections = new Set<MockConnection>();
	const names = new Map<MockConnection, string>();
	let clients = 0;

	// ---- sim.* --------------------------------------------------------------------------------
	let simState: RunState = 'idle';
	let simRun: SimRun | null = null;
	let lastSimId: string | null = null;
	let simCount = 0;
	const f = frequencyLines();

	function broadcast(topic: string, data: Record<string, unknown>) {
		for (const c of connections) c.emit(topic, data);
	}

	/** Controllers start low and step up: -6 dB, then +3 dB every 1.5 s to the target. */
	function currentLevel(r: SimRun): number {
		const steps = Math.floor(r.elapsed / 1.5);
		return Math.min(r.targetDb, r.targetDb - 6 + 3 * steps);
	}

	function frame(r: SimRun, final: RunState, abortNote?: string) {
		const level = currentLevel(r);
		const spec = scaleSpec(r.profile.spec, level);
		const next = rng(hashSeed(r.id, r.frames));
		const scatterDb = 1.1 / Math.sqrt(1 + r.frames / 3) + 0.35;
		const growth = r.profile.growsAt !== undefined ? Math.max(0, r.elapsed - r.profile.growsAt) * 1.5 : 0;
		const control = new Float64Array(f.length);
		let out = 0;
		let peakDev = 0;
		let peakF = f[0];
		// The abort test uses the estimate without its scatter, as a controller's averaged one would.
		let trendPeak = 0;
		let trendF = f[0];
		for (let i = 0; i < f.length; i++) {
			const ripple = 0.35 * Math.sin(Math.log(f[i]) * 6.1 + r.frames * 0.05);
			const bump = growth * Math.exp(-(((Math.log(f[i]) - Math.log(420)) / 0.05) ** 2));
			const trend = ripple + bump;
			const dev = trend + scatterDb * gauss(next);
			if (Math.abs(trend) > Math.abs(trendPeak)) {
				trendPeak = trend;
				trendF = f[i];
			}
			control[i] = specAt(spec, f[i]) * 10 ** (dev / 10);
			if (Math.abs(dev) > TOLERANCE_DB) out++;
			if (Math.abs(dev) > Math.abs(peakDev)) {
				peakDev = dev;
				peakF = f[i];
			}
		}
		const controlRms = rms(f, control);
		const refRms = specRms(spec);
		const errDb = 20 * Math.log10(controlRms / refRms);
		const band = (d: number) => breakpointArrays(scaleSpec(spec, d));

		const spectrum: SpectrumView = {
			view: 'spectrum',
			title: 'Control Against Reference',
			x: { label: 'Frequency', unit: 'Hz', scale: 'log' },
			y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' },
			series: [
				{ name: 'Control', role: 'measured', x: f.slice(), y: control, shape: 'steps', edges: binEdges(f) },
				{ name: 'Reference', role: 'reference', ...breakpointArrays(spec) },
				{ name: `+${TOLERANCE_DB} dB Tolerance`, role: 'tolerance', ...band(TOLERANCE_DB) },
				{ name: `-${TOLERANCE_DB} dB Tolerance`, role: 'tolerance', ...band(-TOLERANCE_DB) },
				{ name: `+${ABORT_DB} dB Abort`, role: 'abort', ...band(ABORT_DB) },
				{ name: `-${ABORT_DB} dB Abort`, role: 'abort', ...band(-ABORT_DB) }
			],
			link: 'live-frequency'
		};
		const rows: ReadingRow[] = [
			{ label: 'Control rms', value: Number(controlRms.toFixed(2)), unit: 'g', status: Math.abs(errDb) <= 1 ? 'pass' : Math.abs(errDb) <= 3 ? 'warn' : 'fail' },
			{ label: 'Reference rms', value: Number(refRms.toFixed(2)), unit: 'g', status: 'info' },
			{ label: 'Control error', value: `${errDb >= 0 ? '+' : ''}${errDb.toFixed(2)}`, unit: 'dB', status: Math.abs(errDb) <= 1 ? 'pass' : 'warn' },
			{ label: 'Level', value: `${level >= 0 ? '+' : ''}${level.toFixed(1)} of ${r.targetDb >= 0 ? '+' : ''}${r.targetDb.toFixed(1)}`, unit: 'dB', status: level < r.targetDb ? 'info' : 'pass' },
			{ label: 'Lines out of tolerance', value: out, unit: null, status: out === 0 ? 'pass' : out < 12 ? 'warn' : 'fail' },
			{
				label: 'Largest deviation',
				value: `${peakDev >= 0 ? '+' : ''}${peakDev.toFixed(1)} dB at ${peakF} Hz`,
				unit: null,
				status: Math.abs(peakDev) <= TOLERANCE_DB ? 'pass' : Math.abs(peakDev) <= ABORT_DB ? 'warn' : 'fail'
			},
			{ label: 'Drive rms', value: Number((0.42 * 10 ** (level / 20) * (1 + 0.02 * gauss(next))).toFixed(3)), unit: 'V', status: 'info' }
		];
		if (abortNote) rows.unshift({ label: 'Abort', value: abortNote, unit: null, status: 'fail' });
		return {
			data: {
				run_id: r.id,
				source: 'SIM',
				state: final,
				elapsed_s: Number(r.elapsed.toFixed(1)),
				level_db: level,
				views: [spectrum, { view: 'reading', title: 'Control Readings', rows }]
			},
			trendPeak,
			trendF
		};
	}

	function simTick() {
		const r = simRun;
		if (!r) return;
		r.elapsed += r.speed === 0 ? 1.0 : FRAME_MS / 1000;
		r.frames += 1;
		const { data, trendPeak, trendF } = frame(r, 'running');
		if (Math.abs(trendPeak) > ABORT_DB) {
			const note = `Control exceeded the ${ABORT_DB} dB abort limit at ${trendF} Hz (${trendPeak >= 0 ? '+' : ''}${trendPeak.toFixed(2)} dB).`;
			simFinish(r, note);
			return;
		}
		broadcast('random.display', data);
	}

	function simFinish(r: SimRun, abortNote?: string) {
		clearInterval(r.timer);
		simRun = null;
		simState = 'stopped';
		broadcast('random.display', frame(r, 'stopped', abortNote).data);
	}

	// ---- controller and observers -------------------------------------------------------------
	let seat: { conn: MockConnection | null; name: string; lease: string } | null = null;
	let leases = 0;
	let seatDeadline: number | null = null;
	let seatTimer: ReturnType<typeof setTimeout> | null = null;

	const isLive = () => random !== null && LIVE_STATES.includes(random.state);
	const orphanS = () => (seatDeadline !== null && isLive() ? round(Math.max(0, (seatDeadline - Date.now()) / 1000), 1) : null);
	const seatView = () => (seat ? { name: seat.name, present: seat.conn !== null } : null);

	function sessionChanged() {
		broadcast('session.changed', { controller: seatView(), orphan_s: orphanS() });
	}

	function armSeatClock() {
		if (seatTimer) clearTimeout(seatTimer);
		seatDeadline = Date.now() + timeoutS * 1000;
		seatTimer = setTimeout(seatExpired, timeoutS * 1000);
	}

	function disarmSeatClock() {
		if (seatTimer) clearTimeout(seatTimer);
		seatTimer = null;
		seatDeadline = null;
	}

	function seatExpired() {
		seatTimer = null;
		seatDeadline = null;
		if (seat && seat.conn === null) seat = null;
		if (!seat && random && isLive()) requestStop(random, 'safe_stop');
		sessionChanged();
	}

	function own(conn: MockConnection) {
		if (seat?.conn === conn) return;
		const holder = seat?.name ?? null;
		refuse(
			'refused',
			'not_controller',
			holder ? `${holder} controls this service; take control once it is released.` : 'This screen does not hold control; take control first.',
			{ holder }
		);
	}

	/** How the log names a connection: the controller by its claimed name, others by the peer. */
	function clientName(conn: MockConnection): string {
		return seat?.conn === conn ? `${seat.name} (controller)` : `${names.get(conn) ?? 'a client'} (observer)`;
	}

	// ---- stations and the channel table -------------------------------------------------------
	let selection: Selection = { kind: 'sim', devices: [] };
	const scan = niInstalled ? scanDevices(niReal) : [];
	const scannedAt = Math.floor(Date.now() / 1000);
	let revision = 1;
	let rows: ChannelRow[] = [newRow(1, 'Control', 'control', 'g'), newRow(2, 'Drive', 'drive', 'V')];

	const niClass = (): StationClass => (niReal ? 'ni_real' : 'ni_simulated');
	const simStation: SimStation = { kind: 'sim', title: 'Simulator', available: true, class: 'sim', inputs: SIM_INPUTS, outputs: SIM_OUTPUTS, clock: 'shared' };

	function niStation(): NiStation {
		if (!niInstalled) {
			return {
				kind: 'ni',
				title: 'NI-DAQmx',
				available: false,
				unavailable: {
					reason: 'ni_driver_unavailable',
					message: 'The NI-DAQmx runtime is not installed on this computer, so no NI station can be used. Install it, or run on the Simulator.'
				},
				scanned_unix: null,
				devices: []
			};
		}
		return { kind: 'ni', title: 'NI-DAQmx', available: true, unavailable: null, scanned_unix: scannedAt, devices: scan };
	}

	function selected(): StationSelected {
		if (selection.kind === 'sim') return { kind: 'sim', devices: [], class: 'sim', source: 'SIM', inputs: SIM_INPUTS, outputs: SIM_OUTPUTS, timing_family: null };
		const devices = scan.filter((d) => selection.devices.includes(d.id));
		const cls = niClass();
		return {
			kind: 'ni',
			devices: [...selection.devices],
			class: cls,
			source: cls === 'ni_real' ? 'HARDWARE' : 'SIM',
			inputs: devices.reduce((n, d) => n + d.inputs.length, 0),
			outputs: devices.reduce((n, d) => n + d.outputs.length, 0),
			timing_family: devices[0]?.timing_family ?? null
		};
	}

	const stationList = (): StationList => ({ stations: [simStation, niStation()], selected: selected() });
	const tableReport = () => validateTable(rows, selection, scan);
	const channelsResult = () => ({ revision, channels: structuredClone(rows), report: tableReport() });

	// ---- the Random run -----------------------------------------------------------------------
	let random: RandomRun | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastStateKey = '';

	function addLog(kind: LogKind, text: string) {
		const entry: LogEntry = { seq: log.length, at_unix: round(Date.now() / 1000, 3), kind, text };
		log.push(entry);
		broadcast('random.log', { entries: [entry] });
	}

	function timingOf(r: RandomRun): { at: number | null; remaining: number | null } {
		if (!DRIVING.includes(r.phase)) return { at: null, remaining: null };
		const at = round(r.atLevelT, 2);
		if (r.mode === 'manual') return { at, remaining: null };
		const levels = r.config.schedule.levels;
		const hold = r.phase === 'full_level' ? r.config.schedule.full_level_s : (levels[r.levelIndex].dwell_s ?? 0);
		return { at, remaining: round(Math.max(0, hold - r.atLevelT), 2) };
	}

	function stateOf(r: RandomRun | null): RandomState {
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
				full_level_s: null,
				elapsed_s: null,
				at_level_s: null,
				remaining_at_level_s: null,
				alarm: false,
				feasibility: null,
				end: null
			};
		}
		const driving = r.state === 'running' || r.state === 'stopping';
		const timing = timingOf(r);
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.config.title,
			source: r.source,
			station: r.station,
			grid: r.grid,
			clock: r.clock,
			mode: r.mode,
			levels_db: r.config.schedule.levels.map((l) => l.db),
			level_db: driving ? round(r.phase === 'sysid' || r.phase === 'coherence_gate' ? r.ctl.sysid_level_db : r.target, 2) : null,
			full_level_s: r.config.schedule.full_level_s,
			elapsed_s: r.t === null ? null : round(r.t, 2),
			at_level_s: timing.at,
			remaining_at_level_s: timing.remaining,
			alarm: r.alarmLines > 0 || r.overload.size > 0,
			feasibility: r.feasibility,
			end: r.end
		};
	}

	/** Send `random.state` when anything the run bar shows has changed. */
	function stateChanged(force = false) {
		const state = stateOf(random);
		const key = JSON.stringify([state.run_id, state.state, state.phase, state.mode, state.level_db, state.alarm, state.end, state.feasibility, state.clock]);
		if (!force && key === lastStateKey) return;
		lastStateKey = key;
		broadcast('random.state', state as unknown as Record<string, unknown>);
	}

	function setPhase(r: RandomRun, state: RandomRunState, phase: RandomPhase) {
		if (r.state === state && r.phase === phase) return;
		if (r.phase !== phase) addLog('state', `${r.phase.replaceAll('_', ' ')} to ${phase.replaceAll('_', ' ')}`);
		r.state = state;
		r.phase = phase;
		r.phaseT = 0;
	}

	function buildLines(r: RandomRun): Lines {
		const resolved = resolveProfile(r.config);
		const [lo, hi] = r.derived.band_hz;
		const df = r.grid.fs_hz / r.grid.n_pts;
		const freqs: number[] = [];
		for (let k = 1; k * df <= r.grid.fs_hz / 2; k++) if (k * df >= lo && k * df <= hi) freqs.push(k * df);
		const fl = Float64Array.from(freqs);
		return {
			f: fl,
			edges: binEdges(fl),
			ref: Float64Array.from(freqs, (x) => specAt(resolved.anchors, x)),
			alarm: freqs.map((x) => bandAt(resolved, resolved.alarm, x)),
			abort: freqs.map((x) => bandAt(resolved, resolved.abort, x))
		};
	}

	function newRun(config: RandomConfig, derived: Derived): RandomRun {
		const sel = selected();
		const devices = scan.filter((d) => sel.devices.includes(d.id)).map((d) => ({ id: d.id, serial: d.serial }));
		const ctl = withDefaults(config.control);
		return {
			id: `r-${String(++runCount).padStart(4, '0')}`,
			config: structuredClone(config),
			ctl,
			derived,
			state: 'configured',
			phase: 'idle',
			mode: config.schedule.mode,
			station: { kind: sel.kind, class: sel.class, devices },
			source: sel.source,
			stationClass: sel.class,
			grid: derived.grid,
			clock: null,
			feasibility: null,
			end: null,
			lines: null,
			t: null,
			phaseT: 0,
			engagedT: 0,
			level: ctl.sysid_level_db,
			target: ctl.sysid_level_db,
			rate: Infinity,
			levelIndex: 0,
			atLevelT: 0,
			stopOutcome: 'operator_stop',
			endedBy: null,
			loops: 0,
			loopAcc: 0,
			coherence: options.coherence ?? 0.94,
			overload: new Set(),
			alarmLines: 0,
			abortLines: 0,
			worstAlarmDb: 0,
			controlPsd: null,
			drivePsd: null,
			controlRms: null,
			driveRms: null,
			channelRms: new Map(),
			timer: null
		};
	}

	/** Feasibility, the station's actual rate and grid, and the arm gate. Nothing is driven. */
	function arm(r: RandomRun, ack: string | undefined) {
		if (r.config.feasibility) {
			if (options.feasibilityExceeds && ack === undefined) {
				r.feasibility = { verdict: 'exceeded', message: 'The profile exceeds the shaker ratings given; arming needs an acknowledgement.' };
				addLog('feasibility', r.feasibility.message);
				setPhase(r, 'blocked', 'feasibility_blocked');
				stateChanged();
				refuse('refused', 'feasibility_exceeded', 'Feasibility exceeded: the profile asks more of the shaker than its ratings. Review it, or arm with an acknowledgement.', {
					feasibility: r.feasibility
				});
			}
			r.feasibility = options.feasibilityExceeds
				? { verdict: 'exceeded', message: 'Armed past a feasibility exceedance on the operator acknowledgement.' }
				: { verdict: 'advisory', message: 'The mock service does not evaluate shaker ratings.' };
		} else {
			r.feasibility = { verdict: 'not_evaluated', message: 'No shaker was given, so the profile was not checked against shaker ratings.' };
		}
		if (ack !== undefined) addLog('override', `operator acknowledgement: ${ack}`);
		if (r.station.kind === 'ni') {
			const report = tableReport();
			if (report.errors.length > 0) {
				refuse('bad_request', 'table_invalid', `The channel table has ${report.errors.length} error(s) on this station, so the run cannot arm.`, { ...report });
			}
		}
		const fs = r.station.kind === 'ni' ? coerceRate(r.config.acquisition.fs_hz) : r.config.acquisition.fs_hz;
		const grid = gridFor(r.config, r.derived.band_hz, fs, true);
		if (r.derived.band_hz[1] > fs / 2.56) {
			refuse(
				'refused',
				'arm_refused',
				`arm refused: the control band reaches ${r.derived.band_hz[1]} Hz but the station's ${grid.fs_hz} Hz at nfft ${grid.n_pts} resolves only to ${(fs / 2.56).toFixed(1)} Hz. Lower the profile's top breakpoint or raise the rate, and arm again.`
			);
		}
		if (grid.lines < MIN_LINES) {
			refuse('refused', 'arm_refused', `arm refused: the station configured ${grid.fs_hz} Hz, which leaves ${grid.lines} control lines in the band (minimum ${MIN_LINES}).`);
		}
		r.grid = grid;
		r.clock = 'shared';
		r.lines = buildLines(r);
		addLog('station', `armed on ${r.station.class} at ${grid.fs_hz} Hz, ${grid.df_hz} Hz lines, ${grid.lines} in the band, shared clock`);
		setPhase(r, 'armed', 'armed');
	}

	function start(r: RandomRun) {
		r.t = 0;
		r.level = r.ctl.sysid_level_db;
		r.target = r.level;
		setPhase(r, 'running', 'sysid');
		r.timer = setInterval(() => tick(r), FRAME_MS);
	}

	function command(r: RandomRun, db: number) {
		const step = Math.abs(db - r.level);
		r.target = db;
		r.rate = r.ctl.level_ramp_s > 0 ? Math.max(step / r.ctl.level_ramp_s, 1e-6) : Infinity;
		r.atLevelT = 0;
		addLog('level', `level ${fmt(db)} dB`);
	}

	function engage(r: RandomRun) {
		const levels = r.config.schedule.levels;
		r.level = levels[0].db - r.ctl.soft_start_db;
		r.levelIndex = 0;
		r.engagedT = 0;
		setPhase(r, 'running', levels.length === 1 ? 'full_level' : 'engaged');
		command(r, levels[0].db);
		if (r.ctl.soft_start_db > 0) r.rate = r.ctl.soft_start_db / (r.ctl.level_ramp_s > 0 ? r.ctl.level_ramp_s : SOFT_START_DEFAULT_S);
	}

	function requestStop(r: RandomRun, outcome: RandomOutcome) {
		if (DRIVING.includes(r.phase) && r.ctl.shutdown_ramp_s > 0) {
			r.stopOutcome = outcome;
			r.target = r.level - 40;
			r.rate = 40 / r.ctl.shutdown_ramp_s;
			addLog('note', `controlled stop: ramping down over ${r.ctl.shutdown_ramp_s} s`);
			setPhase(r, 'stopping', 'ramping_down');
			stateChanged();
			return;
		}
		finish(r, outcome, 'stopped');
	}

	function endMessage(r: RandomRun, outcome: RandomOutcome, trip: Trip | null): string {
		const at = `${fmt(r.level)} dB after ${(r.t ?? 0).toFixed(1)} s`;
		switch (outcome) {
			case 'completed':
				return `Test complete: ${r.config.schedule.full_level_s} s held at full level.`;
			case 'operator_stop':
				return `Stopped by ${r.endedBy ?? 'the operator'} at ${at}.`;
			case 'operator_abort':
				return `Aborted by ${r.endedBy ?? 'the operator'} at ${at}.`;
			case 'safe_stop':
				return `Nobody held control for ${timeoutS} s, so the run was stopped with its shutdown ramp, from ${at}.`;
			case 'held':
				return `Coherence gate held: band coherence ${r.coherence.toFixed(2)} is below the floor ${r.ctl.coherence_floor.toFixed(2)}; the run never drove at test level.`;
			case 'fault':
				return 'The control loop stopped on a fault.';
			case 'aborted':
				if (!trip) return 'Aborted.';
				switch (trip.kind) {
					case 'envelope':
						return `Envelope abort: ${trip.abort_lines} lines outside the abort band, worst ${fmt(trip.worst_db)} dB, at ${fmt(trip.at_level_db)} dB.`;
					case 'kinematic':
						return `Kinematic abort on channel ${trip.channel}: ${trip.value.toFixed(1)} g peak against a ${trip.limit} g limit, at ${fmt(trip.at_level_db)} dB.`;
					case 'drive_vrms':
						return `Drive abort: ${trip.realized_vrms.toFixed(3)} V rms at or above the ${trip.limit_vrms} V limit, at ${fmt(trip.at_level_db)} dB.`;
					case 'divergence':
						return `Divergence abort: ${trip.divergent_bins} lines diverged for ${trip.consecutive_loops} loops in a row.`;
				}
		}
		return 'Ended.';
	}

	function finish(r: RandomRun, outcome: RandomOutcome, phase: RandomPhase, trip: Trip | null = null) {
		const atLevel = DRIVING.includes(r.phase) || r.phase === 'ramping_down';
		r.end = { outcome, message: endMessage(r, outcome, trip), at_s: r.t === null ? null : round(r.t, 2), level_db: atLevel ? round(r.level, 2) : null, trip };
		if (trip) addLog('trip', r.end.message);
		const streaming = r.timer !== null;
		setPhase(r, 'finished', phase);
		addLog('note', r.end.message);
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		if (streaming && r.lines) broadcast('random.live', live(r) as unknown as Record<string, unknown>);
		if (seat?.conn) disarmSeatClock();
		stateChanged();
	}

	/** One control loop: the measured spectra, the protections, and the alarms. */
	function controlLoop(r: RandomRun) {
		const L = r.lines;
		if (!L) return;
		r.loops += 1;
		const next = rng(hashSeed(r.id, r.loops));
		const sysid = r.phase === 'sysid';
		const driving = DRIVING.includes(r.phase);
		const level = sysid ? r.ctl.sysid_level_db : r.level;
		const k = 10 ** (level / 10);
		const scatter = sysid ? 2.0 : 1.1 / Math.sqrt(1 + r.loops / 3) + 0.3;
		const growth = options.abortDemoAtS !== undefined && driving ? Math.max(0, r.engagedT - options.abortDemoAtS) * 1.5 : 0;
		const relax = driving && r.level < r.config.schedule.levels[0].db - 1e-9 ? r.ctl.soft_start_relax_db : 0;
		const n = L.f.length;
		const control = new Float64Array(n);
		const drive = new Float64Array(n);
		let alarmLines = 0;
		let abortLines = 0;
		let worstAlarm = 0;
		let worstAbort = 0;
		for (let i = 0; i < n; i++) {
			const fi = L.f[i];
			const ripple = (sysid ? 1.5 : 0.3) * Math.sin(Math.log(fi) * 6.1 + r.loops * 0.05);
			const bump = growth * Math.exp(-(((Math.log(fi) - Math.log(420)) / 0.1) ** 2));
			const dev = ripple + bump + scatter * gauss(next);
			control[i] = L.ref[i] * k * 10 ** (dev / 10);
			drive[i] = (control[i] / plantT2(fi)) * 4.6e-4;
			if (dev > L.alarm[i][1] + relax || dev < -(L.alarm[i][0] + relax)) {
				alarmLines++;
				if (Math.abs(dev) > Math.abs(worstAlarm)) worstAlarm = dev;
			}
			if (dev > L.abort[i][1] + relax || dev < -(L.abort[i][0] + relax)) {
				abortLines++;
				if (Math.abs(dev) > Math.abs(worstAbort)) worstAbort = dev;
			}
		}
		const df = r.grid.df_hz;
		let driveRms = Math.sqrt(drive.reduce((s, x) => s + x, 0) * df);
		if (driveRms > r.ctl.drive_ceiling_vrms) {
			const g = (r.ctl.drive_ceiling_vrms / driveRms) ** 2;
			for (let i = 0; i < n; i++) drive[i] *= g;
			driveRms = r.ctl.drive_ceiling_vrms;
		}
		const controlRms = Math.sqrt(control.reduce((s, x) => s + x, 0) * df);
		r.controlPsd = control;
		r.drivePsd = drive;
		r.controlRms = controlRms;
		r.driveRms = driveRms;
		const wasAlarm = r.alarmLines > 0;
		r.alarmLines = sysid ? 0 : alarmLines;
		r.abortLines = sysid ? 0 : abortLines;
		r.worstAlarmDb = worstAlarm;
		if (!wasAlarm && r.alarmLines > 0) addLog('alarm', `${r.alarmLines} lines outside the alarm band, worst ${fmt(worstAlarm)} dB`);

		for (const row of rows.filter((x) => x.enabled && x.role !== 'drive')) {
			const gain = row.role === 'control' ? 1 + 0.01 * gauss(next) : 1.21 + 0.02 * gauss(next);
			const value = controlRms * gain;
			r.channelRms.set(row.index, value);
			if (!r.overload.has(row.index) && value * 3 >= 0.9 * row.range_max) {
				r.overload.add(row.index);
				addLog('overload', `channel ${row.index} reached 90 % of its range`);
			}
			if (driving && row.abort.accel_g !== null && value * 3 > row.abort.accel_g) {
				finish(r, 'aborted', 'aborted', { kind: 'kinematic', channel: row.index, axis: 'accel', value: round(value * 3, 2), limit: row.abort.accel_g, at_level_db: round(r.level, 2), at_loop: r.loops });
				return;
			}
		}
		if (driving && r.ctl.drive_abort_vrms !== null && driveRms >= r.ctl.drive_abort_vrms) {
			finish(r, 'aborted', 'aborted', { kind: 'drive_vrms', realized_vrms: round(driveRms, 4), limit_vrms: r.ctl.drive_abort_vrms, at_level_db: round(r.level, 2), at_loop: r.loops });
			return;
		}
		if (driving && r.abortLines > r.ctl.max_lines_out) {
			finish(r, 'aborted', 'aborted', { kind: 'envelope', abort_lines: r.abortLines, worst_db: round(worstAbort, 2), at_level_db: round(r.level, 2), at_loop: r.loops });
		}
	}

	function walk(r: RandomRun, dt: number) {
		if (r.level === r.target) return;
		const stepDb = r.rate === Infinity ? Math.abs(r.target - r.level) : r.rate * dt;
		r.level = r.level < r.target ? Math.min(r.target, r.level + stepDb) : Math.max(r.target, r.level - stepDb);
	}

	/** The run is over, or another has replaced it (a call, not a comparison, so it is read fresh). */
	function ended(r: RandomRun): boolean {
		return random !== r || r.state === 'finished';
	}

	function tick(r: RandomRun) {
		if (ended(r)) return;
		const dt = (FRAME_MS / 1000) * speed;
		const loopS = r.grid.loop_s;
		const levels = r.config.schedule.levels;
		r.t = (r.t ?? 0) + dt;
		r.phaseT += dt;
		switch (r.phase) {
			case 'sysid':
				if (r.phaseT >= Math.max(2, r.ctl.pretest.averages * loopS)) {
					addLog('gate', `system identification done; band coherence ${r.coherence.toFixed(2)}`);
					setPhase(r, 'running', 'coherence_gate');
				}
				break;
			case 'coherence_gate':
				if (r.phaseT >= GATE_S) {
					if (r.coherence < r.ctl.coherence_floor) finish(r, 'held', 'coherence_hold');
					else if (r.ctl.review_after_sysid) setPhase(r, 'review', 'awaiting_review');
					else engage(r);
				}
				break;
			case 'engaged':
			case 'full_level': {
				r.engagedT += dt;
				r.atLevelT += dt;
				walk(r, dt);
				const last = levels.length - 1;
				if (r.mode === 'auto') {
					if (r.phase === 'engaged' && r.levelIndex < last && r.atLevelT >= (levels[r.levelIndex].dwell_s ?? 0)) {
						r.levelIndex += 1;
						command(r, levels[r.levelIndex].db);
						if (r.levelIndex === last) setPhase(r, 'running', 'full_level');
					} else if (r.phase === 'full_level' && r.level === r.target && r.atLevelT >= r.config.schedule.full_level_s) {
						finish(r, 'completed', 'stopped');
					}
				}
				break;
			}
			case 'ramping_down':
				walk(r, dt);
				if (r.level === r.target) finish(r, r.stopOutcome, 'stopped');
				break;
			default:
				break;
		}
		if (ended(r)) return;
		if (r.phase === 'sysid' || DRIVING.includes(r.phase) || r.phase === 'ramping_down') {
			r.loopAcc += dt;
			let loops = 0;
			while (r.loopAcc >= loopS && loops < MAX_LOOPS_PER_FRAME) {
				r.loopAcc -= loopS;
				loops += 1;
				controlLoop(r);
				if (ended(r)) return;
			}
			if (loops === MAX_LOOPS_PER_FRAME) r.loopAcc = 0;
		}
		broadcast('random.live', live(r) as unknown as Record<string, unknown>);
		stateChanged();
	}

	/** The `random.live` payload: three views, every series stepped on the run's lines. */
	function live(r: RandomRun): RandomLive {
		const L = r.lines as Lines;
		const driving = r.state === 'running' || r.state === 'stopping';
		const level = r.phase === 'sysid' || r.phase === 'coherence_gate' ? r.ctl.sysid_level_db : r.level;
		const k = 10 ** (level / 10);
		const at = (band: (i: number) => number) => Float64Array.from(L.ref, (ref, i) => ref * k * 10 ** (band(i) / 10));
		const stepped = (name: string, role: SpectrumView['series'][number]['role'], y: Float64Array) => ({
			name,
			role,
			x: L.f.slice(),
			y,
			shape: 'steps' as const,
			edges: L.edges.slice()
		});
		const controlSeries = [
			...(r.controlPsd ? [stepped('Control', 'measured', r.controlPsd.slice())] : []),
			stepped('Reference', 'reference', at(() => 0)),
			stepped('Alarm high', 'tolerance', at((i) => L.alarm[i][1])),
			stepped('Alarm low', 'tolerance', at((i) => -L.alarm[i][0])),
			stepped('Abort high', 'abort', at((i) => L.abort[i][1])),
			stepped('Abort low', 'abort', at((i) => -L.abort[i][0]))
		];
		const freq = { label: 'Frequency', unit: 'Hz', scale: 'log' as const };
		const referenceRms = Math.sqrt(L.ref.reduce((s, x) => s + x, 0) * r.grid.df_hz * k);
		const channels: LiveChannel[] = rows
			.filter((x) => x.enabled && x.role !== 'drive')
			.map((x) => ({
				index: x.index,
				description: x.description,
				role: x.role as LiveChannel['role'],
				rms: r.channelRms.has(x.index) ? round(r.channelRms.get(x.index) as number, 3) : null,
				unit: x.units,
				overload: r.overload.has(x.index)
			}));
		const alarms: Alarm[] = [
			...(r.alarmLines > 0 ? [{ kind: 'envelope' as const, lines: r.alarmLines, worst_db: round(r.worstAlarmDb, 2) }] : []),
			...[...r.overload].map((channel) => ({ kind: 'overload' as const, channel }))
		];
		const timing = timingOf(r);
		const controlErrDb = r.controlRms === null ? null : 20 * Math.log10(r.controlRms / referenceRms);
		const readings: ReadingRow[] = [
			controlErrDb === null
				? { label: 'Control rms', value: 'pending', unit: null, status: 'info' }
				: { label: 'Control rms', value: round(r.controlRms as number, 3), unit: 'g', status: Math.abs(controlErrDb) <= 1 ? 'pass' : 'warn' },
			{ label: 'Reference rms', value: round(referenceRms, 3), unit: 'g', status: 'info' },
			r.driveRms === null ? { label: 'Drive rms', value: 'pending', unit: null, status: 'info' } : { label: 'Drive rms', value: round(r.driveRms, 3), unit: 'V', status: 'info' },
			{ label: 'Level', value: `${fmt(level)} dB`, unit: null, status: 'info' },
			{ label: 'Lines in alarm', value: `${r.alarmLines} of ${L.f.length}`, unit: null, status: r.alarmLines === 0 ? 'pass' : 'warn' },
			{ label: 'Elapsed', value: round(r.t ?? 0, 1), unit: 's', status: 'info' }
		];
		return {
			run_id: r.id,
			source: r.source,
			class: r.stationClass,
			state: r.state as RandomLive['state'],
			phase: r.phase,
			mode: r.mode,
			elapsed_s: round(r.t ?? 0, 2),
			level_db: driving ? round(level, 2) : null,
			at_level_s: timing.at,
			remaining_at_level_s: timing.remaining,
			loop: r.loops,
			control_rms_g: r.controlRms === null ? null : round(r.controlRms, 3),
			reference_rms_g: round(referenceRms, 3),
			drive_rms_v: r.driveRms === null ? null : round(r.driveRms, 4),
			lines: { in_band: L.f.length, alarm: r.alarmLines, abort: r.abortLines },
			channels,
			alarms,
			views: [
				{ view: 'spectrum', title: 'Control', x: freq, y: { label: 'PSD', unit: 'g²/Hz', scale: 'log' }, series: controlSeries, link: 'frequency' },
				{
					view: 'spectrum',
					title: 'Drive',
					x: freq,
					y: { label: 'PSD', unit: 'V²/Hz', scale: 'log' },
					series: r.drivePsd ? [stepped('Drive', 'drive', r.drivePsd.slice())] : [],
					link: 'frequency'
				},
				{ view: 'reading', title: 'Run', rows: readings }
			]
		};
	}

	function needRun(): RandomRun {
		if (!random) refuse('refused', 'not_configured', 'Random is not configured; configure a test first.');
		return random;
	}

	function notWhileLive(what: string) {
		if (random && isLive()) refuse('refused', 'run_active', `Run ${random.id} is ${random.state}; stop it before ${what}.`, { run_id: random.id });
	}

	function notWhileSim() {
		if (simRun) refuse('refused', 'run_active', `Simulation ${simRun.id} is running; stop it first.`, { run_id: simRun.id });
	}

	function wrongState(r: RandomRun, verb: string): never {
		refuse('refused', 'wrong_state', `${verb} does not apply while the run is ${r.state} (${r.phase.replaceAll('_', ' ')}).`, { state: r.state, phase: r.phase });
	}

	const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
	const busy = (r: RandomRun) => r.state === 'running' || r.state === 'review' || r.state === 'stopping';

	return {
		hello: { type: 'hello', protocol: 'sds/1', service: 'control', version: '0.1.0-mock' },
		latencyMs: () => 25,
		attach: (c) => {
			connections.add(c);
			names.set(c, `client ${++clients}`);
		},
		detach: (c) => {
			connections.delete(c);
			if (seat?.conn === c) {
				// The mock cannot tell a clean close from a drop, so every close is treated as a
				// drop: the run goes on, and the seat is kept (absent) for the timeout. A claim
				// without the lease still takes it, as it does from a vanished holder.
				seat.conn = null;
				armSeatClock();
				sessionChanged();
			}
		},
		methods: {
			// ---- sim.* (superseded for running tests; kept for the Simulate workspace) ----------
			'sim.profiles': () => ({ profiles: PROFILES.map(({ id, title, summary }) => ({ id, title, summary })) }),
			'sim.state': () => ({ state: simState, run_id: simRun?.id ?? lastSimId }),
			'sim.start': (params) => {
				if (simRun) throw new MockError('refused', 'A simulation is already running; stop it before starting another.');
				if (random && isLive()) refuse('refused', 'run_active', `Run ${random.id} holds the station; stop it before starting a simulation.`, { run_id: random.id });
				const profile = PROFILES.find((p) => p.id === params.profile);
				if (!profile) throw new MockError('not_found', `No profile named "${String(params.profile)}"; sim.profiles lists them.`);
				const level = Number(params.level_db);
				if (!Number.isFinite(level) || level < -24 || level > 6) {
					throw new MockError('bad_request', 'Level must be between -24 and +6 dB.');
				}
				const runSpeed = params.speed === 0 ? 0 : 1;
				const id = `sim-${String(++simCount).padStart(4, '0')}`;
				lastSimId = id;
				simState = 'running';
				simRun = { id, profile, targetDb: level, speed: runSpeed, elapsed: 0, frames: 0, timer: setInterval(simTick, FRAME_MS) };
				return { run_id: id };
			},
			'sim.stop': () => {
				if (simRun) simFinish(simRun);
				return { state: simState };
			},

			// ---- controller and observers -------------------------------------------------------
			'session.state': (_params, conn): SessionState => ({
				role: seat?.conn === conn ? 'controller' : 'observer',
				controller: seatView(),
				orphan_s: orphanS(),
				heartbeat_s: HEARTBEAT_S,
				timeout_s: timeoutS
			}),
			'session.claim': (params, conn) => {
				if (params.name !== undefined && (typeof params.name !== 'string' || !params.name)) refuse('bad_request', 'bad_params', '"name" must be a non-empty string.', { param: 'name' });
				if (seat?.conn && seat.conn !== conn) {
					refuse('refused', 'controller_held', `Another client controls this service (${seat.name}). Close that client, or wait for it to release control.`, { holder: seat.name });
				}
				if (seat?.conn === conn) return { role: 'controller', lease: seat.lease, resumed: false, heartbeat_s: HEARTBEAT_S, timeout_s: timeoutS };
				const resumed = seat !== null && seat.conn === null && params.lease === seat.lease;
				const lease = resumed && seat ? seat.lease : `L-${++leases}`;
				seat = { conn, name: typeof params.name === 'string' ? params.name : (names.get(conn) ?? 'client'), lease };
				disarmSeatClock();
				sessionChanged();
				return { role: 'controller', lease, resumed, heartbeat_s: HEARTBEAT_S, timeout_s: timeoutS };
			},
			'session.release': (_params, conn) => {
				if (seat?.conn === conn) {
					seat = null;
					disarmSeatClock();
					if (isLive()) armSeatClock();
					sessionChanged();
				}
				return { role: 'observer', orphan_s: orphanS() };
			},

			// ---- stations -----------------------------------------------------------------------
			'station.list': () => stationList(),
			'station.select': (params, conn) => {
				own(conn);
				notWhileLive('changing the station');
				const kind = params.kind;
				if (kind === 'sim') {
					selection = { kind: 'sim', devices: [] };
				} else if (kind === 'ni') {
					const devices = params.devices;
					if (!Array.isArray(devices) || devices.length === 0 || !devices.every((d) => typeof d === 'string')) {
						refuse('bad_request', 'bad_params', 'Selecting NI hardware needs at least one device.', { param: 'devices' });
					}
					if (!niInstalled) refuse('refused', 'ni_driver_unavailable', niStation().unavailable?.message ?? 'NI-DAQmx is not installed.');
					const found = scan.map((d) => d.id);
					const missing = (devices as string[]).filter((d) => !found.includes(d));
					if (missing.length > 0) {
						refuse('refused', 'devices_absent', `Selected device not present: ${missing.join(', ')}. The scan reports ${found.join(', ')}.`, { missing, found });
					}
					selection = { kind: 'ni', devices: [...(devices as string[])] };
				} else {
					refuse('bad_request', 'bad_params', `"kind" is "sim" or "ni", not ${JSON.stringify(kind)}.`, { param: 'kind' });
				}
				broadcast('station.changed', stationList() as unknown as Record<string, unknown>);
				broadcast('channels.changed', channelsResult());
				return { selected: selected() };
			},

			// ---- the channel table --------------------------------------------------------------
			'channels.get': () => channelsResult(),
			'channels.set': (params, conn) => {
				const p = params as unknown as ChannelsSetParams;
				if (!Array.isArray(p.channels) || typeof p.revision !== 'number') {
					refuse('bad_request', 'bad_params', 'A table edit names the revision it was made from and carries its rows.', { param: Array.isArray(p.channels) ? 'revision' : 'channels' });
				}
				const report = validateTable(p.channels, selection, scan);
				if (p.dry_run) return { revision, report };
				own(conn);
				notWhileLive('changing the channel table');
				if (p.revision !== revision) {
					refuse('refused', 'stale_revision', `The table changed since revision ${p.revision} (it is now ${revision}); read it again and review the change before sending.`, {
						current: revision,
						provided: p.revision
					});
				}
				if (report.errors.length > 0) {
					refuse('bad_request', 'table_invalid', `The channel table has ${report.errors.length} error(s), so it was not written.`, { ...report });
				}
				rows = structuredClone(p.channels);
				revision += 1;
				broadcast('channels.changed', channelsResult());
				return { revision, report };
			},

			// ---- Random -------------------------------------------------------------------------
			'random.configure': (params, conn) => {
				if (!isObject(params.config)) refuse('bad_request', 'bad_params', 'Configuring needs a test configuration.', { param: 'config' });
				const config = params.config as unknown as RandomConfig;
				for (const key of ['profile', 'acquisition', 'schedule'] as const) {
					if (!isObject(config[key])) refuse('bad_request', 'bad_params', `the configuration has no "${key}".`, { param: `config.${key}` });
				}
				const table = tableReport();
				const checked = checkConfig(config, rows);
				if (params.dry_run === true) {
					return {
						errors: [...table.errors, ...checked.errors],
						warnings: [...table.warnings, ...checked.warnings],
						derived: table.errors.length > 0 ? null : checked.derived
					};
				}
				own(conn);
				notWhileLive('configuring');
				if (table.errors.length > 0) refuse('bad_request', 'table_invalid', `The channel table has ${table.errors.length} error(s) on this station.`, { ...table });
				if (checked.errors.length > 0 || !checked.derived) {
					refuse('bad_request', 'config_invalid', `The configuration has ${checked.errors.length} error(s).`, { errors: checked.errors, warnings: checked.warnings });
				}
				random = newRun(config, checked.derived);
				log = [];
				addLog('state', `configured "${config.title}" on ${random.station.class}`);
				stateChanged(true);
				return { state: stateOf(random), derived: checked.derived, warnings: checked.warnings };
			},
			'random.arm': (params, conn) => {
				own(conn);
				const r = needRun();
				const ack = params.override_ack;
				if (ack !== undefined && (typeof ack !== 'string' || ack.trim() === '')) {
					refuse('bad_request', 'bad_params', "An override acknowledgement is the operator's own words; it cannot be empty.", { param: 'override_ack' });
				}
				if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.', { outcome: r.end?.outcome });
				notWhileLive('arming again');
				notWhileSim();
				if (ack !== undefined && r.state !== 'blocked') refuse('refused', 'override_not_needed', 'The run is not blocked by feasibility; arm without an acknowledgement.');
				arm(r, ack as string | undefined);
				stateChanged();
				return stateOf(r);
			},
			'random.start': (_params, conn) => {
				own(conn);
				const r = needRun();
				if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.', { outcome: r.end?.outcome });
				if (busy(r)) notWhileLive('starting again');
				notWhileSim();
				if (r.state === 'configured' || r.state === 'blocked') arm(r, undefined);
				start(r);
				stateChanged();
				return stateOf(r);
			},
			'random.proceed': (_params, conn) => {
				own(conn);
				const r = needRun();
				if (r.state !== 'review') wrongState(r, 'Proceeding');
				addLog('note', 'the operator reviewed system identification and proceeded');
				engage(r);
				stateChanged();
				return stateOf(r);
			},
			'random.level': (params, conn) => {
				own(conn);
				const r = needRun();
				const hasDelta = typeof params.delta_db === 'number' && Number.isFinite(params.delta_db);
				const hasTo = typeof params.to_db === 'number' && Number.isFinite(params.to_db);
				if (hasDelta === hasTo) refuse('bad_request', 'bad_params', 'A level change gives either a step in dB or a level of the schedule.', { param: 'delta_db' });
				if (r.mode !== 'manual') refuse('refused', 'manual_mode_required', 'Level changes need the manual ladder; switch the ladder to manual first.', { mode: r.mode });
				if (!DRIVING.includes(r.phase)) wrongState(r, 'A level change');
				const levels = r.config.schedule.levels.map((l) => l.db);
				let target: number;
				if (hasTo) {
					const i = levels.findIndex((l) => Math.abs(l - (params.to_db as number)) < 1e-9);
					if (i < 0) refuse('bad_request', 'not_a_rung', `${params.to_db} dB is not a level of the schedule (${levels.join(', ')} dB).`, { levels_db: levels });
					r.levelIndex = i;
					target = levels[i];
				} else {
					target = Math.min(levels[levels.length - 1], Math.max(levels[0], r.target + (params.delta_db as number)));
				}
				command(r, target);
				setPhase(r, 'running', target >= levels[levels.length - 1] - 1e-9 ? 'full_level' : 'engaged');
				stateChanged();
				return { level_db: round(target, 2) };
			},
			'random.mode': (params, conn) => {
				own(conn);
				const r = needRun();
				const mode = params.mode;
				if (mode !== 'auto' && mode !== 'manual') refuse('bad_request', 'bad_params', '"mode" is "auto" or "manual".', { param: 'mode' });
				if (mode === r.mode) return { mode };
				r.mode = mode;
				addLog('note', `ladder mode ${mode}`);
				if (mode === 'auto' && DRIVING.includes(r.phase)) {
					// resume from the present level: on a level of the schedule, hold it out; between
					// two, hold the lower one's dwell and then move to the one above
					const levels = r.config.schedule.levels.map((l) => l.db);
					const on = levels.findIndex((l) => Math.abs(l - r.target) < 1e-9);
					const above = levels.findIndex((l) => l > r.target + 1e-9);
					r.levelIndex = on >= 0 ? on : above > 0 ? above - 1 : above === 0 ? 0 : levels.length - 1;
					r.atLevelT = 0;
					setPhase(r, 'running', on === levels.length - 1 || above < 0 ? 'full_level' : 'engaged');
				}
				stateChanged(true);
				return { mode };
			},
			// Stop and abort are open to every connection, controller or observer: they only ever
			// end a run, and the service listens on loopback only (PROTOCOL.md, "Controller and
			// Observers"). The log and the end message name who sent them.
			'random.stop': (_params, conn) => {
				if (random && isLive()) {
					random.endedBy = clientName(conn);
					addLog('note', `stop requested by ${random.endedBy}`);
					requestStop(random, 'operator_stop');
				}
				return stateOf(random);
			},
			'random.abort': (_params, conn) => {
				if (random && isLive()) {
					random.endedBy = clientName(conn);
					addLog('note', `abort requested by ${random.endedBy}`);
					finish(random, 'operator_abort', 'stopped');
				}
				return stateOf(random);
			},
			'random.reset': (_params, conn) => {
				own(conn);
				const r = needRun();
				if (busy(r)) notWhileLive('resetting');
				if (r.timer) clearInterval(r.timer);
				random = newRun(r.config, checkConfig(r.config, rows).derived ?? r.derived);
				log = [];
				addLog('state', `reset; configured "${r.config.title}" again`);
				stateChanged(true);
				return stateOf(random);
			},
			'random.clear': (_params, conn) => {
				own(conn);
				if (random && busy(random)) notWhileLive('clearing');
				random = null;
				log = [];
				stateChanged(true);
				return stateOf(null);
			},
			'random.state': () => stateOf(random),
			'random.log': (params) => {
				const since = typeof params.since === 'number' ? Math.max(0, Math.floor(params.since)) : 0;
				return { entries: log.slice(since), next: log.length };
			}
		}
	};
}
