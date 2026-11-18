/**
 * MIMO Sine in mock mode (PROTOCOL.md, "MIMO Sine"): a made-up run that walks every state the
 * screens show. Configure and its dry run check what the protocol names; Start identifies, finds
 * a resonance in the band and holds for review; the tone then climbs the level schedule in a
 * sweep, a dwell or a stepped sine. Every number is made up so that every panel can be seen: it
 * is not a measurement and nothing here controls anything.
 *
 * The run's clock is compressed: a one-way sweep takes a few seconds of the mock's time whatever
 * its rate, a dwell's level a few seconds, a stepped sine's step half a second. Where the run has
 * a limit channel, the channel nears its bound partway up the band and the drive vector is scaled
 * down there (the notch), then released. The simulator's structure picks the story:
 *
 * - any structure but the two below: a clean run to completion;
 * - `sine-softening-4x4-moderate`: at full level one control's error grows through a resonance
 *   until its abort band ends the run;
 * - `sine-near-singular-4x4`: the opposition reading climbs near one frequency, and the law asks
 *   for the sweep to slow there (granted only with the request switch on).
 */

import type { ChannelRow, DataSource, LogEntry, LogKind, RefusalReason, RunRecordRef, Series, SpectrumView, StationRef, View } from '../types';
import type { LawEntry } from '../mimo';
import type {
	MimoSineConfig,
	MimoSineDerived,
	MimoSineEnd,
	MimoSineIdentificationFacts,
	MimoSineLawEntry,
	MimoSineLiveHead,
	MimoSineOutcome,
	MimoSinePhase,
	MimoSineQuantity,
	MimoSineReport,
	MimoSineReportItem,
	MimoSineReview,
	MimoSineRunState,
	MimoSineState,
	MimoSineSubscribePlot,
	MimoSineTestType,
	MimoSineTrip
} from '../mimoSine';
import { MockError, type MockConnection, type MockMethod } from './socket';

export interface MimoSineMockHost {
	speed: number;
	timeoutS: number;
	own(conn: MockConnection): void;
	hasController(): boolean;
	otherRun(): string | null;
	broadcast(topic: string, data: Record<string, unknown>): void;
	rows(): ChannelRow[];
	station(): { source: DataSource; ref: StationRef };
	clientName(conn: MockConnection): string;
	/** The run's record: refused at arm or start (`unwritable`). */
	record?: 'unwritable' | 'problem';
	recordFolder?(runId: string): string;
}

export interface MimoSineMock {
	methods: Record<string, MockMethod>;
	isLive(): boolean;
	liveId(): string | null;
	detach(conn: MockConnection): void;
	seatExpired(): void;
	laws(): MimoSineLawEntry[];
}

const TICK_MS = 50;
const LIVE_MS = 200;
const SYSID_S = 1.5;
/** Mock seconds a one-way sweep takes, whatever its rate. */
const PASS_S = 6;
/** Mock seconds a dwell holds a level below full. */
const DWELL_LEVEL_S = 2;
const STEP_S = 0.5;
const GRID_POINTS = 256;
const LIVE: MimoSineRunState[] = ['armed', 'running', 'review', 'stopping'];
export const MOCK_STRUCTURES = [
	'sine-linear-4x4',
	'sine-softening-4x4-mild',
	'sine-softening-4x4-moderate',
	'sine-hardening-4x4',
	'sine-tall-6x3',
	'sine-wide-3x6',
	'sine-base-article-6x4',
	'sine-near-singular-4x4'
];
const SIZE: Record<string, [number, number]> = {
	'sine-tall-6x3': [6, 3],
	'sine-wide-3x6': [3, 6],
	'sine-base-article-6x4': [6, 4]
};

function refuse(code: 'refused' | 'bad_request', reason: string, message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason as RefusalReason, detail);
}

const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const wrap = (deg: number) => {
	let p = ((deg + 180) % 360 + 360) % 360 - 180;
	if (p === -180) p = 180;
	return p;
};

// ---------------------------------------------------------------------------------------------
// The law catalog of the sine_mimo world, as the service lists it

const GAIN_RULE = { key: 'gain_rule', label: 'Gain rule', help: 'The loop-speed rule sets the gain every update from the tracking filter\'s delay and the identification\'s group delay near the tone; fixed uses the printed gain.', kind: 'choice' as const, options: ['loop_speed', 'fixed'], default: 'loop_speed' };

function entry(id: string, summary: string, tests: MimoSineTestType[], amplitudeOnly: boolean, params: LawEntry['params'], diagnostics: LawEntry['diagnostics']): MimoSineLawEntry {
	return {
		id,
		world: 'sine_mimo',
		builtin: true,
		name: id,
		version: '0.1.0',
		author: 'Hertzforge',
		summary,
		sha256: null,
		check: { status: 'built_in', date: null, tool_version: null, cases: [], hardware: true, message: 'Built in: cleared for hardware.' },
		params,
		diagnostics,
		shapes: ['square', 'more_controls_than_drives', 'fewer_controls_than_drives'],
		needs: null,
		test_types: tests,
		amplitude_only: amplitudeOnly,
		default: id === 'matrix',
		limits: null
	} as MimoSineLawEntry;
}

const diag = (key: string, label: string, unit: string, help: string) => ({ key, label, unit, help, layout: null });

const LAWS: MimoSineLawEntry[] = [
	entry(
		'matrix',
		'Corrects the drive through the regularized inverse of the identification every update, and carries the drive along the model as the sweep moves.',
		['sweep', 'dwell', 'stepped'],
		true,
		[
			{ key: 'carry', label: 'Carry weight', help: 'How much of the drive the model predicts for the next frequency is added as the sweep moves, 0 to 1.', kind: 'number', unit: '', min: 0, max: 1, step: 0.01, default: 0.67 },
			GAIN_RULE,
			{ key: 'free_phase', label: 'Phase with amplitudes only', help: 'With an amplitude-only reference: take every control in phase, or take each control\'s phase from the measurement.', kind: 'choice', options: ['in_phase', 'measured'], default: 'in_phase' }
		],
		[
			diag('gain', 'Gain', '', 'The correction gain on the last update.'),
			diag('group_delay_s', 'Group delay', 's', "The identification's group delay near the tone."),
			diag('condition', 'Condition', '', 'The condition number of the response at the tone.'),
			diag('carried_share', 'Carried share', '', 'The share of the last drive change that was carried along the model.'),
			diag('rotation_deg', 'Rotation asked for', 'deg', "The largest turn of a drive's phase the law asked for."),
			diag('unseen_share', 'Unseen share', '', 'The share of the drive the controls cannot see, where there are spare shakers.'),
			diag('modes_kept', 'Modes kept', '', 'The modes the carried model holds.'),
			diag('candidates_left_out', 'Candidates left out', '', 'Resonance candidates the model fit left out.')
		]
	),
	entry('paired', 'Pairs each control with one drive and moves that drive by the ratio of wanted to measured amplitude. No model of the test article.', ['dwell', 'stepped'], true, [GAIN_RULE], [diag('gain', 'Gain', '', 'The step gain on the last update.')]),
	entry('steepest_descent', 'Moves the drive down the gradient of the squared control error through the identification. No inverse is formed.', ['dwell'], false, [GAIN_RULE], [diag('gain', 'Gain', '', 'The step gain on the last update.')]),
	entry(
		'secant',
		'Finds a complex gain per control from two successive drives and their responses, and uses it for the next step.',
		['dwell'],
		false,
		[{ key: 'carry', label: 'Carry', help: 'How the gain is carried to the next frequency.', kind: 'choice', options: ['factor', 'plain'], default: 'factor' }],
		[diag('gain', 'Gain', '', 'The secant gain on the last update.')]
	),
	entry(
		'ratio',
		"Multiplies each drive's amplitude by a compressed ratio of wanted to measured amplitude on its paired control.",
		['dwell', 'stepped'],
		true,
		[{ key: 'pairing', label: 'Pairing', help: 'Pair each drive with a control by the reference or by the drive.', kind: 'choice', options: ['reference', 'drive'], default: 'reference' }],
		[diag('gain', 'Gain', '', 'The ratio step on the last update.')]
	)
];

// ---------------------------------------------------------------------------------------------
// The run

interface Point {
	f: number;
	t: number;
	ctl: number[];
	ref: number[];
	ph: number[];
	refPh: number[];
	err: number[];
	depth: number[];
	lim: number[];
	bound: number[];
	drv: number[];
	drvPh: number[];
	opp: number;
	share: number;
}

interface Run {
	id: string;
	config: MimoSineConfig;
	derived: MimoSineDerived;
	state: MimoSineRunState;
	phase: MimoSinePhase;
	source: DataSource;
	station: StationRef & { structure: string | null };
	structure: string;
	controls: number[];
	drives: number[];
	limits: number[];
	levels: number[];
	rung: number;
	levelDb: number | null;
	t: number;
	phaseT: number;
	atLevel: number;
	/** Progress through the present pass, 0 to 1 (a sweep), or the step index (stepped). */
	progress: number;
	direction: 'up' | 'down';
	passes: number;
	passesAtLevel: number;
	steps: number[];
	held: boolean;
	requestHeld: boolean;
	alarm: boolean;
	notchScale: number;
	notchedS: number[];
	identification: MimoSineIdentificationFacts | null;
	review: MimoSineReview | null;
	end: MimoSineEnd | null;
	params: Record<string, unknown>;
	law: MimoSineLawEntry;
	misses: number;
	cappedTicks: number;
	overCeiling: number;
	rotationMax: number;
	requests: { asked: number; granted: number; heldS: number };
	pass: Point[];
	/** The last pass ended, still drawn once the run has finished. */
	lastPass: Point[];
	history: Point[];
	last: Point | null;
	worst: { err: number[]; errHz: (number | null)[]; ph: (number | null)[]; alarmS: number[]; depth: number[]; margin: (number | null)[]; marginHz: (number | null)[]; vpk: number[]; vpkHz: (number | null)[]; opp: number | null; share: number | null };
	stopping: number | null;
	timer: ReturnType<typeof setInterval> | null;
	record: RunRecordRef | null;
}

function structureOf(c: MimoSineConfig): string {
	const nc = c.channels.controls.length;
	const nd = c.channels.drives.length;
	if (c.simulator?.structure) return c.simulator.structure;
	if (nc === 6 && nd === 3) return 'sine-tall-6x3';
	if (nc === 3 && nd === 6) return 'sine-wide-3x6';
	return 'sine-linear-4x4';
}

function bandOf(c: MimoSineConfig): [number, number] {
	const t = c.test;
	if (t.kind === 'dwell') return [t.freq_hz, t.freq_hz];
	if (t.kind === 'stepped' && 'frequencies_hz' in t) return [Math.min(...t.frequencies_hz), Math.max(...t.frequencies_hz)];
	return [t.band_hz[0], t.band_hz[1]];
}

/** A profile's value at a frequency, joined in log-log between rows and held beyond them. */
function profileAt(rows: { freq_hz: number; value: number }[], f: number): number {
	if (!rows.length) return 0;
	if (f <= rows[0].freq_hz) return rows[0].value;
	for (let i = 1; i < rows.length; i++) {
		const a = rows[i - 1];
		const b = rows[i];
		if (f <= b.freq_hz) return a.value * (b.value / a.value) ** (Math.log(f / a.freq_hz) / Math.log(b.freq_hz / a.freq_hz));
	}
	return rows[rows.length - 1].value;
}

function phaseAt(rows: { freq_hz: number; phase_deg: number }[] | undefined, f: number): number {
	if (!rows?.length) return 0;
	if (f <= rows[0].freq_hz) return rows[0].phase_deg;
	for (let i = 1; i < rows.length; i++) {
		const a = rows[i - 1];
		const b = rows[i];
		if (f <= b.freq_hz) return a.phase_deg + (b.phase_deg - a.phase_deg) * (Math.log(f / a.freq_hz) / Math.log(b.freq_hz / a.freq_hz));
	}
	return rows[rows.length - 1].phase_deg;
}

function stepsOf(c: MimoSineConfig): number[] {
	const t = c.test;
	if (t.kind !== 'stepped') return [];
	if ('frequencies_hz' in t) return [...t.frequencies_hz];
	const [lo, hi] = t.band_hz;
	const out: number[] = [];
	if ((t.spacing ?? 'log') === 'log') for (let f = lo, k = 0; f <= hi * (1 + 1e-9) && k < 400; k++, f = lo * 2 ** (k / t.steps_per_octave)) out.push(round(f, 3));
	else {
		const n = Math.max(1, Math.round(Math.log2(hi / lo) * t.steps_per_octave));
		for (let k = 0; k <= n; k++) out.push(round(lo + ((hi - lo) * k) / n, 3));
	}
	return out;
}

export function createMimoSineMock(host: MimoSineMockHost): MimoSineMock {
	let run: Run | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastKey = '';
	const subs = new Map<MockConnection, { rate: number; plots: MimoSineSubscribePlot[] }>();
	let liveTimer: ReturnType<typeof setInterval> | null = null;
	const isLive = () => run !== null && LIVE.includes(run.state);

	function addLog(kind: LogKind, text: string) {
		const e: LogEntry = { seq: log.length, at_unix: Date.now() / 1000, kind, text };
		log.push(e);
		if (run) host.broadcast('mimo_sine.log', { run_id: run.id, entries: [e] });
	}

	// ---- the state ------------------------------------------------------------------------------

	function remaining(r: Run): number | null {
		const t = r.config.test;
		const left = r.levels.length - 1 - r.rung;
		if (t.kind === 'sweep') {
			const full = (r.config.schedule?.full_level && 'sweeps' in r.config.schedule.full_level ? r.config.schedule.full_level.sweeps : 2) * PASS_S;
			return round(Math.max(0, left * PASS_S + full - r.atLevel), 1);
		}
		return round(Math.max(0, left * DWELL_LEVEL_S + DWELL_LEVEL_S * 2 - r.atLevel), 1);
	}

	function stateOf(r: Run | null): MimoSineState {
		if (!r) {
			return {
				run_id: null, state: 'unconfigured', phase: 'idle', title: null, source: null, station: null, grid: null, clock: null, test: null, shape: null,
				phase_specified: null, mode: null, levels_db: null, level_db: null, rung: null, frequency_hz: null, direction: null, sweeps: null, steps: null,
				held: false, request_held: false, elapsed_s: null, at_level_s: null, remaining_s: null, alarm: false, notch: false, notch_scale: null,
				identification: null, review: null, law: null, feasibility: null, end: null
			};
		}
		const started = r.state !== 'configured';
		const t = r.config.test;
		const driving = r.phase === 'engaged' || r.phase === 'full_level';
		const fullSweeps = r.config.schedule?.full_level && 'sweeps' in r.config.schedule.full_level ? r.config.schedule.full_level.sweeps : 2;
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.config.title ?? null,
			source: r.source,
			station: r.station,
			grid: { ...r.derived.grid, actual: started },
			clock: started ? 'shared' : null,
			test: t.kind,
			shape: r.derived.shape,
			phase_specified: r.config.phase_specified ?? true,
			mode: r.config.schedule?.mode ?? 'auto',
			levels_db: r.levels,
			level_db: r.levelDb,
			rung: driving ? r.rung : null,
			frequency_hz: driving ? round(freqOf(r), 2) : null,
			direction: t.kind === 'sweep' ? r.direction : null,
			sweeps: t.kind === 'sweep' ? { done: r.passes, of: (r.levels.length - 1) + fullSweeps } : null,
			steps: t.kind === 'stepped' ? { done: Math.floor(r.progress), of: r.steps.length } : null,
			held: r.held,
			request_held: r.requestHeld,
			elapsed_s: started ? round(r.t, 2) : null,
			at_level_s: driving ? round(r.atLevel, 2) : null,
			remaining_s: driving ? remaining(r) : null,
			alarm: r.alarm,
			notch: r.notchScale < 1,
			notch_scale: round(r.notchScale, 3),
			identification: r.identification,
			review: r.review,
			law: { id: r.law.id, name: r.law.name, version: r.law.version, sha256: r.law.sha256, params: r.params, misses: r.misses, capped_ticks: r.cappedTicks, over_ceiling: r.overCeiling, rotation_max_deg: round(r.rotationMax, 2), requests: { asked: r.requests.asked, granted: r.requests.granted } },
			feasibility: r.derived.feasibility,
			end: r.end,
			...(r.record ? { record: { ...r.record } } : {})
		};
	}

	function stateChanged(force = false) {
		const s = stateOf(run);
		const key = JSON.stringify({ ...s, elapsed_s: null, at_level_s: null, remaining_s: null, frequency_hz: null, notch_scale: null, law: null, steps: null, sweeps: null });
		if (!force && key === lastKey) return;
		lastKey = key;
		host.broadcast('mimo_sine.state', s as unknown as Record<string, unknown>);
	}

	function claimRecord(r: Run) {
		if (r.record) return;
		const folder = host.recordFolder?.(r.id) ?? `~/Hertzforge_Data/${r.id}`;
		if (host.record === 'unwritable') {
			const reason = 'Permission denied';
			refuse('refused', 'record_unwritable', `The run's record could not be written, so the run did not arm: ${folder} (${reason}). Choose a data folder that can be written.`, { path: folder, reason });
		}
		r.record = { folder, streams: r.config.record?.streams === false ? 'off' : 'on', problem: null };
	}

	// ---- checking a configuration -----------------------------------------------------------------

	function check(c: MimoSineConfig): { errors: MimoSineReportItem[]; warnings: MimoSineReportItem[]; derived: MimoSineDerived | null; views: SpectrumView[] } {
		const errors: MimoSineReportItem[] = [];
		const warnings: MimoSineReportItem[] = [];
		const item = (reason: MimoSineReportItem['reason'] | string, message: string, field: string): MimoSineReportItem => ({ reason: reason as MimoSineReportItem['reason'], message, field });
		const rows = host.rows();
		const ctl = c.channels?.controls ?? [];
		const drv = c.channels?.drives ?? [];
		const lims = c.limits?.channels ?? [];
		const nc = ctl.length;
		const nd = drv.length;
		if (nc < 1 || nc > 16) errors.push(item('controls_out_of_range', 'From 1 to 16 controls.', 'channels.controls'));
		if (nd < 1 || nd > 8) errors.push(item('drives_out_of_range', 'From 1 to 8 drives.', 'channels.drives'));
		if (lims.length > 8) errors.push(item('limits_out_of_range', 'At most 8 limit channels.', 'limits.channels'));
		const named = [...ctl.map((x) => x.channel), ...drv.map((x) => x.channel), ...lims.map((x) => x.channel)];
		if (new Set(named).size !== named.length) errors.push(item('duplicate_channel', 'A channel is named twice among the controls, drives and limit channels.', 'channels'));
		const phaseOn = c.phase_specified ?? true;
		const band = bandOf(c);
		ctl.forEach((x, i) => {
			if (!rows.some((r) => r.index === x.channel && r.enabled && r.role === 'control')) errors.push(item('control_unknown_channel', `Channel ${x.channel} is not an enabled control row.`, `channels.controls[${i}].channel`));
			const pr = x.profile?.rows ?? [];
			if (pr.length < 2) errors.push(item('profile_too_short', `Control ${x.channel}: give at least two rows.`, `channels.controls[${i}].profile.rows`));
			else if (pr[0].freq_hz > band[0] + 1e-9 || pr[pr.length - 1].freq_hz < band[1] - 1e-9)
				errors.push(item('profile_not_covering', `Control ${x.channel}: the reference runs from ${pr[0].freq_hz} to ${pr[pr.length - 1].freq_hz} Hz; it covers the test's band, ${band[0]} to ${band[1]} Hz.`, `channels.controls[${i}].profile.rows`));
			if (pr.some((r) => !(r.value > 0))) errors.push(item('value_not_positive', `Control ${x.channel}: every value is above 0.`, `channels.controls[${i}].profile.rows`));
			if (phaseOn) {
				if (!x.phase?.length) errors.push(item('phase_missing', `Control ${x.channel} has no reference phase; give one, or run on amplitudes only.`, `channels.controls[${i}].phase`));
				else if (x.phase.some((p, k) => !(p.freq_hz > 0) || p.phase_deg <= -180 || p.phase_deg > 180 || (k > 0 && p.freq_hz <= x.phase![k - 1].freq_hz)))
					errors.push(item('phase_invalid', `Control ${x.channel}: phase rows ascend in frequency, each above -180 and at most 180 degrees.`, `channels.controls[${i}].phase`));
			}
		});
		const driveRows = rows.filter((r) => r.enabled && r.role === 'drive').map((r) => r.index);
		drv.forEach((x, i) => {
			if (!driveRows.includes(x.channel)) errors.push(item('drive_unknown_channel', `Channel ${x.channel} is not an enabled drive row.`, `channels.drives[${i}].channel`));
			if (!(x.ceiling_vpk > 0 && x.ceiling_vpk <= 10)) errors.push(item('out_of_range', 'The ceiling is above 0 and at most 10 V pk.', `channels.drives[${i}].ceiling_vpk`));
			if (!(x.abort_vpk > x.ceiling_vpk && x.abort_vpk <= 10)) errors.push(item('abort_below_ceiling', 'The abort is above the ceiling and at most 10 V pk.', `channels.drives[${i}].abort_vpk`));
		});
		if (driveRows.length !== nd || driveRows.some((ch, i) => drv[i]?.channel !== ch)) errors.push(item('drive_unknown_channel', "Every enabled drive row is a drive of the run, in the table's order.", 'channels.drives'));
		lims.forEach((x, i) => {
			if (ctl.some((k) => k.channel === x.channel)) errors.push(item('limit_on_control_channel', `Channel ${x.channel} is a control; a limit channel is another input.`, `limits.channels[${i}].channel`));
			else if (!rows.some((r) => r.index === x.channel && r.enabled && r.role !== 'drive')) errors.push(item('limit_unknown_channel', `Channel ${x.channel} is not an enabled input.`, `limits.channels[${i}].channel`));
		});
		const t = c.test;
		if (t.kind === 'stepped' && stepsOf(c).length === 0) errors.push(item('steps_empty', 'The stepped sine has no steps.', 'test'));
		const s = c.schedule ?? {};
		const levels = s.levels_db ?? [-12, -9, -6, -3, 0];
		if (!levels.length) errors.push(item('levels_empty', 'The schedule has no levels.', 'schedule.levels_db'));
		for (let i = 1; i < levels.length; i++) if (levels[i] <= levels[i - 1]) errors.push(item('levels_not_ascending', 'The levels rise from one to the next.', `schedule.levels_db[${i}]`));
		if (s.start_db !== undefined && !levels.includes(s.start_db)) errors.push(item('start_not_a_level', `${s.start_db} dB is not a level of the schedule.`, 'schedule.start_db'));
		const structure = structureOf(c);
		if (!MOCK_STRUCTURES.includes(structure)) errors.push(item('structure_unknown', `No simulator structure "${structure}".`, 'simulator.structure'));
		else if (c.simulator?.structure) {
			// A structure named must be the run's size; with none named the mock plays its linear story at any size.
			const [sc, sd] = SIZE[structure] ?? [4, 4];
			if (host.station().ref.kind === 'sim' && (sc !== nc || sd !== nd)) errors.push(item('structure_wrong_size', `The structure "${structure}" has ${sc} controls and ${sd} drives; the run has ${nc} and ${nd}.`, 'simulator.structure'));
		}
		const lawId = c.control?.law?.id ?? 'matrix';
		const law = LAWS.find((l) => l.id === lawId);
		const shape = nc === nd ? 'square' : nc > nd ? 'more_controls_than_drives' : 'fewer_controls_than_drives';
		if (!law) errors.push(item('law_unknown', `No control law "${lawId}" in the catalog.`, 'control.law.id'));
		else {
			if (!law.test_types.includes(t.kind)) errors.push(item('law_test_type_not_accepted', `${law.name} does not accept a ${t.kind === 'stepped' ? 'stepped sine' : t.kind}.`, 'control.law.id'));
			if (!phaseOn && !law.amplitude_only) errors.push(item('law_amplitude_only_not_accepted', `${law.name} needs a phase on every reference.`, 'control.law.id'));
			if (!law.shapes.includes(shape)) errors.push(item('law_shape_not_accepted', `${law.name} does not accept this shape.`, 'control.law.id'));
			for (const [k, v] of Object.entries(c.control?.law?.params ?? {})) {
				const decl = law.params.find((x) => x.key === k);
				if (!decl) errors.push(item('law_param_unknown', `The law declares no parameter "${k}".`, `control.law.params.${k}`));
				else if (decl.kind === 'number' && (typeof v !== 'number' || v < decl.min || v > decl.max)) errors.push(item('law_param_invalid', `${decl.label} is from ${decl.min} to ${decl.max}.`, `control.law.params.${k}`));
				else if (decl.kind === 'choice' && !decl.options.includes(String(v))) errors.push(item('law_param_invalid', `${decl.label} is one of ${decl.options.join(', ')}.`, `control.law.params.${k}`));
			}
		}
		const rate = t.kind === 'sweep' && t.rate.law === 'log' ? t.rate.oct_per_min : null;
		if (rate !== null && rate > 6) warnings.push(item('tracking_lag', `At ${rate} oct/min the tracking filter lags the tone near the band's low edge; expect the error there to read high.`, 'control.tracking'));
		if (errors.length) return { errors, warnings, derived: null, views: [] };
		const fs = c.acquisition?.fs_hz ?? 16384;
		const tickFrames = Math.max(2, Math.round(((c.acquisition?.tick_ms ?? 10) / 1000) * fs));
		const derived: MimoSineDerived = {
			band_hz: band,
			shape,
			controls: nc,
			drives: nd,
			limits: lims.length,
			grid: { fs_hz: fs, tick_frames: tickFrames, tick_s: round(tickFrames / fs, 7), actual: false },
			profiles: ctl.map((x) => {
				const peak = Math.max(...x.profile.rows.filter((r) => r.quantity === 'accel_g').map((r) => r.value), 0);
				return { channel: x.channel, crossovers: [], peak_accel_g: peak, peak_velocity_ips: round(peak * 6.15, 2), peak_disp_pp_in: round(peak * 0.39, 4) } as MimoSineDerived['profiles'][number];
			}),
			test_s: t.kind === 'sweep' && rate ? round(((Math.log2(band[1] / band[0]) / rate) * 60) * (levels.length + 1), 1) : null,
			identification_s: (2 * (1 + (c.identification?.frames ?? Math.max(24, 6 * nd)))) / (c.identification?.resolution_hz ?? 0.25),
			call_budget_s: round((c.control?.law_budget ?? 0.25) * (tickFrames / fs), 5),
			stop_ramp_db_per_s: 120,
			safe_stop_s: 0.85,
			tracking: rate !== null && rate > 6 ? { verdict: 'advisory', ratio: round(rate / 12, 2), at_hz: band[0], message: warnings[warnings.length - 1].message } : { verdict: 'ok', ratio: 0.04, at_hz: null, message: null },
			law: { id: law!.id, name: law!.name, version: law!.version, sha256: null, shapes: law!.shapes, test_types: law!.test_types, amplitude_only: law!.amplitude_only },
			feasibility: { verdict: 'not_evaluated', message: c.feasibility ? 'The shaker records were read.' : 'No shaker records were given: feasibility is not evaluated and the kinematic monitor is off.' }
		};
		return { errors, warnings, derived, views: configViews(c) };
	}

	// ---- views ------------------------------------------------------------------------------------

	const fAxis = { label: 'Frequency', unit: 'Hz', scale: 'log' as const };
	const tAxis = { label: 'Time', unit: 's', scale: 'linear' as const };

	function gridOf(band: [number, number], n = GRID_POINTS): Float64Array {
		const x = new Float64Array(n);
		for (let i = 0; i < n; i++) x[i] = band[0] * (band[1] / band[0]) ** (i / (n - 1));
		return x;
	}

	function configViews(c: MimoSineConfig): SpectrumView[] {
		const band = bandOf(c);
		const b: [number, number] = band[1] > band[0] ? band : [band[0] / 1.5, band[0] * 1.5];
		const x = gridOf(b);
		const out: SpectrumView[] = [];
		const line = (name: string, role: Series['role'], f: (fr: number) => number): Series => ({ name, role, x, y: Float64Array.from(x, f) });
		for (const k of c.channels.controls) {
			const ref = (fr: number) => profileAt(k.profile.rows, fr);
			const a = k.profile.alarm_db ?? [1.5, 1.5];
			const ab = k.profile.abort_db ?? [3, 3];
			out.push({ view: 'spectrum', title: `Profile ${k.channel}`, x: fAxis, y: { label: 'Amplitude', unit: 'g', scale: 'log' }, link: 'frequency', series: [
				line('Reference', 'reference', ref), line('Alarm high', 'tolerance', (f) => ref(f) * 10 ** (a[1] / 20)), line('Alarm low', 'tolerance', (f) => ref(f) / 10 ** (a[0] / 20)), line('Abort high', 'abort', (f) => ref(f) * 10 ** (ab[1] / 20)), line('Abort low', 'abort', (f) => ref(f) / 10 ** (ab[0] / 20))
			] });
			if (c.phase_specified ?? true) out.push({ view: 'spectrum', title: `Phase ${k.channel}`, x: fAxis, y: { label: 'Phase', unit: 'deg', scale: 'linear' }, link: 'frequency', series: [line('Reference phase', 'reference', (f) => phaseAt(k.phase, f))] });
		}
		for (const l of c.limits?.channels ?? []) out.push({ view: 'spectrum', title: `Limit ${l.channel}`, x: fAxis, y: { label: 'Amplitude', unit: 'g', scale: 'log' }, link: 'frequency', series: [line('Bound', 'limit', (f) => profileAt(l.profile.rows, f))] });
		return out;
	}

	const unitOf = (ch: number) => host.rows().find((r) => r.index === ch)?.units || 'g';
	const nameOf = (ch: number, word: string) => host.rows().find((r) => r.index === ch)?.description || `${word} ${ch}`;

	function series(points: Point[], name: string, role: Series['role'], y: (p: Point) => number, dwell: boolean): Series {
		return { name, role, x: Float64Array.from(points, (p) => (dwell ? p.t : p.f)), y: Float64Array.from(points, y) };
	}

	/**
	 * The points a view is drawn on: the pass in a sweep or a stepped sine (the last one ended, once
	 * the run has finished), the last 60 s in a dwell.
	 */
	function pointsOf(r: Run): { pts: Point[]; dwell: boolean } {
		const dwell = r.config.test.kind === 'dwell';
		if (dwell) return { pts: r.history.filter((p) => p.t >= r.t - 60).slice(-1200), dwell };
		return { pts: r.state === 'finished' && !r.pass.length ? r.lastPass : r.pass, dwell };
	}

	function xAxis(dwell: boolean) {
		return dwell ? tAxis : fAxis;
	}

	function controlView(r: Run, i: number, pts: Point[], dwell: boolean): SpectrumView {
		const ch = r.controls[i];
		const prof = r.config.channels.controls[i].profile;
		const a = prof.alarm_db ?? [1.5, 1.5];
		const ab = prof.abort_db ?? [3, 3];
		return {
			view: 'spectrum', title: `Control ${ch}`, x: xAxis(dwell), y: { label: 'Amplitude', unit: unitOf(ch), scale: 'log' }, link: dwell ? 'time' : 'frequency',
			series: [
				series(pts, 'Measured', 'measured', (p) => p.ctl[i], dwell),
				series(pts, 'Reference', 'reference', (p) => p.ref[i], dwell),
				series(pts, 'Alarm high', 'tolerance', (p) => p.ref[i] * 10 ** (a[1] / 20), dwell),
				series(pts, 'Alarm low', 'tolerance', (p) => p.ref[i] / 10 ** (a[0] / 20), dwell),
				series(pts, 'Abort high', 'abort', (p) => p.ref[i] * 10 ** (ab[1] / 20), dwell),
				series(pts, 'Abort low', 'abort', (p) => p.ref[i] / 10 ** (ab[0] / 20), dwell)
			]
		};
	}

	function phaseView(r: Run, i: number, pts: Point[], dwell: boolean): SpectrumView {
		return { view: 'spectrum', title: `Phase ${r.controls[i]}`, x: xAxis(dwell), y: { label: 'Phase', unit: 'deg', scale: 'linear' }, link: dwell ? 'time' : 'frequency', series: [series(pts, 'Measured', 'measured', (p) => p.ph[i], dwell), series(pts, 'Reference', 'reference', (p) => p.refPh[i], dwell)] };
	}

	function errorView(r: Run, pts: Point[], dwell: boolean): SpectrumView {
		const s = r.controls.map((ch, i) => series(pts, `Control ${ch}`, 'measured', (p) => p.err[i], dwell));
		if (pts.some((p) => p.depth.some((d) => d > 0))) r.controls.forEach((ch, i) => s.push(series(pts, `Notch depth ${ch}`, 'other', (p) => -p.depth[i], dwell)));
		return { view: 'spectrum', title: 'Error', x: xAxis(dwell), y: { label: 'Error', unit: 'dB', scale: 'linear' }, link: dwell ? 'time' : 'frequency', series: s };
	}

	function limitView(r: Run, j: number, pts: Point[], dwell: boolean): SpectrumView {
		const ch = r.limits[j];
		return { view: 'spectrum', title: `Limit ${ch}`, x: xAxis(dwell), y: { label: 'Amplitude', unit: unitOf(ch), scale: 'log' }, link: dwell ? 'time' : 'frequency', series: [series(pts, 'Measured', 'measured', (p) => p.lim[j], dwell), series(pts, 'Bound', 'limit', (p) => p.bound[j], dwell)] };
	}

	function drivesView(r: Run, pts: Point[], dwell: boolean, phase = false): SpectrumView {
		return {
			view: 'spectrum', title: phase ? 'Drive phase' : 'Drives', x: xAxis(dwell), y: phase ? { label: 'Phase', unit: 'deg', scale: 'linear' } : { label: 'Drive', unit: 'V', scale: 'log' }, link: dwell ? 'time' : 'frequency',
			series: r.drives.map((ch, k) => series(pts, `Drive ${ch}`, 'drive', (p) => (phase ? p.drvPh[k] : p.drv[k]), dwell))
		};
	}

	function readingsView(r: Run, pts: Point[], dwell: boolean): SpectrumView {
		return { view: 'spectrum', title: 'Readings', x: xAxis(dwell), y: { label: 'Reading', unit: null, scale: 'linear' }, link: dwell ? 'time' : 'frequency', series: [series(pts, 'Opposition', 'measured', (p) => p.opp, dwell), series(pts, 'Largest share', 'other', (p) => p.share, dwell)] };
	}

	function runReading(r: Run): View {
		const s = stateOf(r);
		const p = r.last;
		const rows: { label: string; value: number | string; unit: string | null; status: 'info' | 'warn' | 'fail' }[] = [
			{ label: 'Frequency', value: round(freqOf(r), 2), unit: 'Hz', status: 'info' },
			...(r.config.test.kind === 'sweep' ? [{ label: 'Direction', value: r.direction, unit: null, status: 'info' as const }] : []),
			{ label: 'Level', value: r.levelDb ?? 0, unit: 'dB', status: 'info' },
			{ label: 'Notch', value: round(r.notchScale, 3), unit: null, status: r.notchScale < 1 ? 'warn' : 'info' },
			{ label: 'Drive', value: p ? round(Math.max(...p.drv), 3) : 0, unit: 'V', status: 'info' },
			{ label: 'Opposition', value: p ? round(p.opp, 3) : 0, unit: null, status: 'info' },
			{ label: 'Largest share', value: p ? round(p.share, 3) : 0, unit: null, status: 'info' },
			s.sweeps ? { label: 'Sweeps', value: `${s.sweeps.done} of ${s.sweeps.of}`, unit: null, status: 'info' } : { label: 'Steps', value: s.steps ? `${s.steps.done} of ${s.steps.of}` : '-', unit: null, status: 'info' },
			{ label: 'Elapsed', value: round(r.t, 1), unit: 's', status: 'info' },
			{ label: 'Remaining', value: s.remaining_s ?? 0, unit: 's', status: 'info' }
		];
		return { view: 'reading', title: 'Run', rows } as View;
	}

	function identificationViews(r: Run, ch: number): View[] {
		const band = r.derived.band_hz[1] > r.derived.band_hz[0] ? r.derived.band_hz : ([r.derived.band_hz[0] / 2, r.derived.band_hz[0] * 2] as [number, number]);
		const x = gridOf(band, 200);
		const fr = resonanceOf(r);
		const isLimit = r.limits.includes(ch);
		const row = isLimit ? r.limits.indexOf(ch) + r.controls.length : Math.max(0, r.controls.indexOf(ch));
		const gain: Series[] = r.drives.map((d, k) => ({ name: `Drive ${d}`, role: 'measured', x, y: Float64Array.from(x, (f) => (row === k ? 2 : 0.6) / Math.hypot(1 - (f / fr) ** 2, 0.06 * (f / fr)) * (1 + 0.1 * k)) }));
		const phase: Series[] = r.drives.map((d, k) => ({ name: `Drive ${d}`, role: 'measured', x, y: Float64Array.from(x, (f) => wrap((-Math.atan2(0.06 * (f / fr), 1 - (f / fr) ** 2) * 180) / Math.PI - 4 * k)) }));
		const coh: Series[] = [
			...r.controls.map((c, i) => ({ name: `Control ${c}`, role: 'measured' as const, x, y: Float64Array.from(x, (f) => Math.min(0.999, 0.97 - 0.002 * i + 0.02 * Math.cos(f / 37))) })),
			...r.limits.map((c) => ({ name: `Limit ${c}`, role: 'measured' as const, x, y: Float64Array.from(x, () => 0.95) })),
			{ name: 'Trust coherence', role: 'tolerance' as const, x, y: Float64Array.from(x, () => r.config.identification?.trust_coherence ?? 0.5) }
		];
		return [
			{ view: 'spectrum', title: `Identification Gain ${ch}`, x: fAxis, y: { label: 'Gain', unit: `${unitOf(ch)}/V`, scale: 'log' }, link: 'frequency', series: gain },
			{ view: 'spectrum', title: `Identification Phase ${ch}`, x: fAxis, y: { label: 'Phase', unit: 'deg', scale: 'linear' }, link: 'frequency', series: phase },
			{ view: 'spectrum', title: 'Coherence', x: fAxis, y: { label: 'Coherence', unit: null, scale: 'linear' }, link: 'frequency', series: coh }
		] as View[];
	}

	function lawReading(r: Run): View {
		const p = r.last;
		const values = lawValues(r, p);
		const rows = [
			...r.law.diagnostics.filter((d) => d.key in values).map((d) => ({ label: d.label, value: values[d.key], unit: d.unit || null, status: 'info' })),
			{ label: 'Unusable calls in a row', value: r.misses, unit: null, status: 'info' },
			{ label: 'Ticks a cap bound', value: r.cappedTicks, unit: null, status: 'info' },
			{ label: 'Largest rotation', value: round(r.rotationMax, 2), unit: 'deg', status: 'info' },
			{ label: 'Requests', value: `${r.requests.asked} made, ${r.requests.granted} granted`, unit: null, status: 'info' }
		];
		return { view: 'reading', title: 'Law', rows } as View;
	}

	function lawValues(r: Run, p: Point | null): Record<string, number> {
		if (!p) return {};
		const out: Record<string, number> = { gain: round(0.12 + 0.03 * Math.sin(r.t), 3) };
		if (r.law.id === 'matrix') {
			Object.assign(out, { group_delay_s: 0.0042, condition: round(r.structure === 'sine-near-singular-4x4' ? 2 + p.opp * 8 : 3.1, 2), carried_share: round(r.config.test.kind === 'sweep' ? 0.6 : 0, 2), rotation_deg: round(Math.abs(Math.sin(r.t * 3)) * 2, 2), unseen_share: r.drives.length > r.controls.length ? 0.04 : 0, modes_kept: 5, candidates_left_out: 0 });
		}
		return out;
	}

	function liveViews(r: Run): [SpectrumView, SpectrumView, View] {
		const { pts, dwell } = pointsOf(r);
		const controls: SpectrumView = {
			view: 'spectrum', title: 'Controls', x: xAxis(dwell), y: { label: 'Amplitude', unit: unitOf(r.controls[0]), scale: 'log' }, link: dwell ? 'time' : 'frequency',
			series: r.controls.flatMap((ch, i) => [series(pts, `Control ${ch}`, 'measured', (p) => p.ctl[i], dwell), series(pts, `Reference ${ch}`, 'reference', (p) => p.ref[i], dwell)])
		};
		return [controls, drivesView(r, pts, dwell), runReading(r)];
	}

	function plotViews(q: MimoSineQuantity, p: MimoSineSubscribePlot): View[] {
		const r = run;
		if (!r) return [];
		if (q === 'identification') {
			if (!r.identification) return [];
			const ch = Array.isArray(p.channels) && p.channels.length ? p.channels[0] : r.controls[0];
			return identificationViews(r, ch);
		}
		if (!r.last) return [];
		const { pts, dwell } = pointsOf(r);
		const pick = (list: number[]) => (Array.isArray(p.channels) ? list.flatMap((ch, i) => ((p.channels as number[]).includes(ch) ? [i] : [])) : list.map((_, i) => i));
		switch (q) {
			case 'control':
				return pick(r.controls).map((i) => controlView(r, i, pts, dwell));
			case 'phase':
				return (r.config.phase_specified ?? true) ? pick(r.controls).map((i) => phaseView(r, i, pts, dwell)) : [];
			case 'error':
				return [errorView(r, pts, dwell)];
			case 'limit':
				return pick(r.limits).map((j) => limitView(r, j, pts, dwell));
			case 'drive':
				return [drivesView(r, pts, dwell)];
			case 'drive_phase':
				return [drivesView(r, pts, dwell, true)];
			case 'readings':
				return [readingsView(r, pts, dwell), runReading(r)];
			case 'time': {
				const n = p.points ?? 1024;
				const f = freqOf(r);
				const t = Float64Array.from({ length: n }, (_, k) => k / r.derived.grid.fs_hz);
				const chans = Array.isArray(p.channels) ? p.channels : [...r.controls, ...r.limits];
				return chans.map((ch) => {
					const i = r.controls.indexOf(ch);
					const a = i >= 0 ? (r.last?.ctl[i] ?? 0) : (r.last?.lim[r.limits.indexOf(ch)] ?? 0);
					return { view: 'spectrum', title: nameOf(ch, 'Channel'), x: tAxis, y: { label: 'Amplitude', unit: unitOf(ch), scale: 'linear' }, link: 'time', series: [{ name: 'Measured', role: 'measured', x: t, y: Float64Array.from(t, (s) => a * Math.sin(2 * Math.PI * f * s)) }] } as View;
				});
			}
			case 'law':
				return [lawReading(r)];
		}
		return [];
	}

	// ---- the tone --------------------------------------------------------------------------------

	function resonanceOf(r: Run): number {
		const [lo, hi] = r.derived.band_hz;
		return hi > lo ? lo * (hi / lo) ** 0.45 : lo * 1.05;
	}

	function freqOf(r: Run): number {
		const t = r.config.test;
		if (t.kind === 'dwell') return t.freq_hz;
		if (t.kind === 'stepped') return r.steps[Math.min(r.steps.length - 1, Math.floor(r.progress))] ?? r.derived.band_hz[0];
		const [lo, hi] = r.derived.band_hz;
		const u = r.direction === 'up' ? r.progress : 1 - r.progress;
		return t.rate.law === 'log' ? lo * (hi / lo) ** u : lo + (hi - lo) * u;
	}

	/** The run's numbers at this tick, made up: a small error through the resonance, a limit nearing its bound. */
	function pointOf(r: Run): Point {
		const f = freqOf(r);
		const lin = 10 ** ((r.levelDb ?? 0) / 20);
		const fr = resonanceOf(r);
		const near = Math.exp(-((Math.log(f / fr) / 0.06) ** 2));
		const full = r.levelDb !== null && Math.abs(r.levelDb) < 1e-9;
		const grow = r.structure === 'sine-softening-4x4-moderate' && full ? Math.min(5, r.atLevel * 1.2) : 0;
		// The limit channels: the predicted response passes the bound near 1.4 times the resonance.
		const fl = fr * 1.4;
		const limNear = Math.exp(-((Math.log(f / fl) / 0.08) ** 2));
		const bound = r.config.limits?.channels?.map((l) => profileAt(l.profile.rows, f)) ?? [];
		const wanted = bound.map((b) => b * lin * 10 ** ((-8 + 11 * limNear) / 20));
		const scale = Math.min(1, ...wanted.map((w, j) => (w > bound[j] * lin ? (bound[j] * lin) / w : 1)));
		const target = r.notchScale + (scale - r.notchScale) * 0.5;
		r.notchScale = target;
		const depthDb = r.notchScale < 1 ? -20 * Math.log10(r.notchScale) : 0;
		const ref = r.config.channels.controls.map((c) => profileAt(c.profile.rows, f) * lin);
		const refPh = r.config.channels.controls.map((c) => ((r.config.phase_specified ?? true) ? phaseAt(c.phase, f) : 0));
		const err = ref.map((_, i) => round(0.4 * near * Math.sin(1 + i) + 0.05 * Math.sin(r.t * 2.3 + i) + (i === 1 ? grow * near * 3 + grow * 0.2 : 0) - depthDb, 3));
		const ctl = ref.map((v, i) => v * 10 ** (err[i] / 20));
		const ph = refPh.map((p, i) => wrap(p + 2 * near * Math.cos(i) + 0.3 * Math.sin(r.t + i)));
		const lim = wanted.map((w) => w * r.notchScale);
		const singular = r.structure === 'sine-near-singular-4x4' ? Math.exp(-((Math.log(f / (fr * 0.8)) / 0.05) ** 2)) : 0;
		const drv = r.drives.map((_, k) => round(lin * r.notchScale * (0.18 + 0.05 * k + 0.4 * near) * (1 + 2 * singular), 4));
		const drvPh = r.drives.map((_, k) => wrap(-20 * k - 70 * near + (singular ? 150 * singular * (k % 2) : 0)));
		const norm = Math.hypot(...drv);
		const share = drv.length ? Math.max(...drv.map((v) => (v * v) / (norm * norm || 1))) : 0;
		return { f, t: r.t, ctl, ref, ph, refPh, err, depth: ref.map(() => round(depthDb, 3)), lim, bound: bound.map((b) => b * lin), drv, drvPh, opp: round(1.02 + 0.15 * near + 3 * singular, 3), share: round(share, 3) };
	}

	function liveOf(r: Run): MimoSineLiveHead & { views: View[] } {
		const s = stateOf(r);
		const p = r.last;
		const t = r.config.test;
		const live: MimoSineLiveHead = {
			run_id: r.id,
			source: r.source,
			class: r.station.class,
			state: r.state as MimoSineLiveHead['state'],
			phase: r.phase,
			test: t.kind,
			mode: s.mode ?? 'auto',
			t_s: round(r.t, 3),
			elapsed_s: s.elapsed_s,
			frequency_hz: s.frequency_hz,
			direction: s.direction,
			...(t.kind === 'sweep' ? (t.rate.law === 'log' ? { sweep_rate_oct_per_min: (r.direction === 'up' ? 1 : -1) * t.rate.oct_per_min * (r.requestHeld ? 0.5 : 1) } : { sweep_rate_hz_per_s: (r.direction === 'up' ? 1 : -1) * t.rate.hz_per_s }) : {}),
			sweeps: s.sweeps,
			steps: s.steps,
			held: r.held,
			request_held: r.requestHeld,
			level_db: r.levelDb,
			commanded_db: r.levelDb,
			notch: r.notchScale < 0.999,
			notch_scale: round(r.notchScale, 3),
			alarm: r.alarm,
			controls: r.controls.map((ch, i) => ({
				channel: ch,
				description: nameOf(ch, 'Control'),
				unit: unitOf(ch),
				amplitude: p ? round(p.ctl[i], 4) : null,
				phase_deg: p ? round(p.ph[i], 2) : null,
				reference: p ? round(p.ref[i], 4) : null,
				reference_phase_deg: p && (r.config.phase_specified ?? true) ? round(p.refPh[i], 2) : null,
				error_db: p ? p.err[i] : null,
				phase_error_deg: p && (r.config.phase_specified ?? true) ? round(wrap(p.ph[i] - p.refPh[i]), 2) : null,
				notch_depth_db: p ? p.depth[i] : 0,
				overload: false
			})),
			limits: r.limits.map((ch, j) => ({ channel: ch, description: nameOf(ch, 'Limit'), unit: unitOf(ch), amplitude: p ? round(p.lim[j], 4) : null, phase_deg: p ? 171 : null, bound: p ? round(p.bound[j], 4) : null, margin_db: p && p.bound[j] > 0 ? round(20 * Math.log10(p.lim[j] / p.bound[j]), 2) : null })),
			drives: r.drives.map((ch, k) => ({ channel: ch, vpk: p ? p.drv[k] : 0, phase_deg: p ? round(p.drvPh[k], 2) : 0, ceiling_vpk: r.config.channels.drives[k].ceiling_vpk })),
			readings: { opposition_ratio: p ? p.opp : null, largest_share: p ? p.share : null, rotation_deg: p ? round(Math.abs(Math.sin(r.t * 3)) * 2, 2) : null },
			request: r.requestHeld ? { kind: 'slow', factor: 0.5, granted: r.config.requests?.honour === true } : null,
			law: p ? { values: lawValues(r, p), misses: r.misses, capped_ticks: r.cappedTicks, faulted: false } : null
		};
		return { ...live, views: liveViews(r) };
	}

	function broadcastLive(r: Run) {
		host.broadcast('mimo_sine.live', liveOf(r) as unknown as Record<string, unknown>);
	}

	function dataFor(conn: MockConnection) {
		const sub = subs.get(conn);
		if (!sub || !run) return;
		const { views: _views, ...head } = liveOf(run);
		void _views;
		conn.emit('mimo_sine.data', { ...head, plots: sub.plots.map((p) => ({ id: p.id, quantity: p.quantity, channels: Array.isArray(p.channels) ? p.channels : null, views: plotViews(p.quantity, p) })) } as unknown as Record<string, unknown>);
	}

	function sendData() {
		for (const c of subs.keys()) dataFor(c);
	}

	function passViews(r: Run): View[] {
		const pts = r.pass;
		const out: View[] = [];
		r.controls.forEach((_, i) => out.push(controlView(r, i, pts, false)));
		if (r.config.phase_specified ?? true) r.controls.forEach((_, i) => out.push(phaseView(r, i, pts, false)));
		out.push(errorView(r, pts, false));
		r.limits.forEach((_, j) => out.push(limitView(r, j, pts, false)));
		out.push(drivesView(r, pts, false), drivesView(r, pts, false, true), readingsView(r, pts, false));
		return out;
	}

	function endPass(r: Run) {
		if (!r.pass.length) return;
		host.broadcast('mimo_sine.sweep', { run_id: r.id, pass: r.passes + 1, level_db: r.levelDb ?? 0, direction: r.config.test.kind === 'sweep' ? r.direction : null, views: passViews(r) } as unknown as Record<string, unknown>);
		r.lastPass = r.pass;
		r.pass = [];
	}

	function trackWorst(r: Run, p: Point, dt: number) {
		const w = r.worst;
		p.err.forEach((e, i) => {
			if (Math.abs(e) > Math.abs(w.err[i])) {
				w.err[i] = e;
				w.errHz[i] = round(p.f, 2);
			}
			const pe = wrap(p.ph[i] - p.refPh[i]);
			if ((r.config.phase_specified ?? true) && Math.abs(pe) > Math.abs(w.ph[i] ?? 0)) w.ph[i] = round(pe, 2);
			const a = r.config.channels.controls[i].profile.alarm_db ?? [1.5, 1.5];
			if (e > a[1] || e < -a[0]) w.alarmS[i] += dt;
			w.depth[i] = Math.max(w.depth[i], p.depth[i]);
		});
		p.lim.forEach((v, j) => {
			const m = p.bound[j] > 0 ? 20 * Math.log10(v / p.bound[j]) : null;
			if (m !== null && (w.margin[j] === null || m > (w.margin[j] as number))) {
				w.margin[j] = round(m, 2);
				w.marginHz[j] = round(p.f, 2);
			}
			if (r.notchScale < 0.999) r.notchedS[j] += dt;
		});
		p.drv.forEach((v, k) => {
			if (v > w.vpk[k]) {
				w.vpk[k] = v;
				w.vpkHz[k] = round(p.f, 2);
			}
		});
		w.opp = Math.max(w.opp ?? 0, p.opp);
		w.share = Math.max(w.share ?? 0, p.share);
	}

	// ---- the schedule ----------------------------------------------------------------------------

	function trip(r: Run, t: MimoSineTrip, message: string) {
		end(r, 'aborted', message, t);
	}

	function end(r: Run, outcome: MimoSineOutcome, message: string, t: MimoSineTrip | null = null) {
		if (r.config.test.kind !== 'dwell') endPass(r);
		r.state = 'finished';
		r.phase = outcome === 'operator_abort' || outcome === 'aborted' ? 'aborted' : outcome === 'held' ? 'coherence_hold' : 'stopped';
		r.end = { outcome, message, at_s: round(r.t, 2), level_db: r.levelDb, at_hz: r.last ? round(r.last.f, 2) : null, ramp_s: outcome === 'operator_abort' || outcome === 'aborted' ? null : 0.4, trip: t };
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		addLog('state', `The run ended: ${message}`);
		stateChanged(true);
		broadcastLive(r);
		host.broadcast('mimo_sine.report', { report: reportOf(r) } as unknown as Record<string, unknown>);
		stopLive();
	}

	function reportOf(r: Run): MimoSineReport | null {
		if (!r.end) return null;
		const w = r.worst;
		return {
			run_id: r.id,
			title: r.config.title ?? null,
			source: r.source,
			grid: { ...r.derived.grid, actual: true },
			test: r.config.test.kind,
			shape: r.derived.shape,
			phase_specified: r.config.phase_specified ?? true,
			law: stateOf(r).law!,
			levels_db: r.levels,
			controls: r.controls.map((ch, i) => ({ channel: ch, worst_error_db: round(w.err[i], 2), worst_error_hz: w.errHz[i], worst_phase_error_deg: (r.config.phase_specified ?? true) ? w.ph[i] : null, alarm_s: round(w.alarmS[i], 2), notch_depth_max_db: round(w.depth[i], 2) })),
			limits: r.limits.map((ch, j) => ({ channel: ch, worst_margin_db: w.margin[j], worst_margin_hz: w.marginHz[j], notched_s: round(r.notchedS[j], 2) })),
			drives: r.drives.map((ch, k) => ({ channel: ch, vpk_max: w.vpk[k], vpk_max_hz: w.vpkHz[k], ceiling_vpk: r.config.channels.drives[k].ceiling_vpk, over_ceiling_ticks: 0 })),
			readings: { opposition_ratio_max: w.opp, largest_share_max: w.share },
			requests: { asked: r.requests.asked, granted: r.requests.granted, held_s: round(r.requests.heldS, 2) },
			identification: r.identification,
			end: r.end
		};
	}

	function identify(r: Run) {
		const nc = r.controls.length;
		const nd = r.drives.length;
		const id = r.config.identification ?? {};
		const gate = id.gate_coherence ?? 0.5;
		r.identification = {
			level_db: id.level_db ?? -20,
			measured_level_db: round((id.level_db ?? -20) + 2.4, 1),
			realizations: id.frames ?? Math.max(24, 6 * nd),
			line_spacing_hz: id.resolution_hz ?? 0.25,
			duration_s: (2 * (1 + (id.frames ?? Math.max(24, 6 * nd)))) / (id.resolution_hz ?? 0.25),
			drive_vrms: r.drives.map(() => 0.05),
			coherence_mean: r.controls.map((_, i) => round(0.99 - 0.004 * i, 3)),
			limit_coherence_mean: r.limits.map(() => 0.96),
			trusted_frac: 0.97,
			condition_max: r.structure === 'sine-near-singular-4x4' ? 140 : nc === nd ? 12.4 : 6.1,
			lag_frames: 0,
			gate_coherence: gate,
			passed: true
		};
		const t = r.config.test;
		const fr = resonanceOf(r);
		const rate = t.kind === 'sweep' && t.rate.law === 'log' ? t.rate.oct_per_min : t.kind === 'sweep' ? 1 : null;
		const tc = rate === null ? null : round(2.6 / rate, 2);
		r.review = {
			resonances: t.kind === 'sweep' && tc !== null ? [{ freq_hz: round(fr, 1), bandwidth_hz: round(fr * 0.02, 2), time_constants: tc, flagged: tc < 2 }, { freq_hz: round(fr * 1.9, 1), bandwidth_hz: round(fr * 0.06, 2), time_constants: round(tc * 4, 2), flagged: tc * 4 < 2 }] : [],
			acknowledged: null
		};
		addLog('state', 'Identification done; the coherence check passed.');
		if (id.review ?? true) {
			r.state = 'review';
			r.phase = 'awaiting_review';
		} else engage(r);
		sendData();
		stateChanged(true);
	}

	function engage(r: Run) {
		r.state = 'running';
		const start = r.config.schedule?.start_db;
		r.rung = Math.max(0, start === undefined ? 0 : r.levels.indexOf(start));
		r.levelDb = r.levels[r.rung];
		r.phase = r.rung === r.levels.length - 1 ? 'full_level' : 'engaged';
		r.atLevel = 0;
		r.progress = 0;
		r.direction = r.config.test.kind === 'sweep' ? (r.config.test.direction ?? 'up') : 'up';
		addLog('law', `The law ${r.law.name} was built from the identification; engaged at ${r.levelDb} dB.`);
	}

	function nextLevel(r: Run): boolean {
		if (r.rung >= r.levels.length - 1) return false;
		if ((r.config.schedule?.mode ?? 'auto') === 'manual') return true;
		r.rung += 1;
		r.levelDb = r.levels[r.rung];
		r.atLevel = 0;
		r.passesAtLevel = 0;
		r.phase = r.rung === r.levels.length - 1 ? 'full_level' : 'engaged';
		addLog('level', `Level ${r.levelDb} dB.`);
		stateChanged(true);
		return true;
	}

	function fullCount(r: Run): number {
		const fl = r.config.schedule?.full_level;
		return fl && 'sweeps' in fl ? fl.sweeps : 2;
	}

	function drive(r: Run, dt: number) {
		if (r.held) return;
		const t = r.config.test;
		const slow = r.requestHeld && r.config.requests?.honour === true ? 0.5 : 1;
		r.atLevel += dt;
		if (t.kind === 'sweep') {
			r.progress += (dt / PASS_S) * slow;
			if (r.progress >= 1) {
				endPass(r);
				r.passes += 1;
				r.passesAtLevel += 1;
				r.progress = 0;
				r.direction = r.direction === 'up' ? 'down' : 'up';
				const full = r.rung === r.levels.length - 1;
				if (full && r.passesAtLevel >= fullCount(r)) return end(r, 'completed', 'The schedule finished.');
				if (!full) nextLevel(r);
			}
		} else if (t.kind === 'stepped') {
			const before = Math.floor(r.progress);
			r.progress += dt / STEP_S;
			if (Math.floor(r.progress) !== before) stateChanged(true);
			if (r.progress >= r.steps.length) {
				endPass(r);
				r.passes += 1;
				r.progress = 0;
				if (r.rung === r.levels.length - 1) return end(r, 'completed', 'The schedule finished.');
				nextLevel(r);
			}
		} else {
			const full = r.rung === r.levels.length - 1;
			const hold = full ? Math.min(DWELL_LEVEL_S * 2, 'seconds' in t.end ? t.end.seconds : DWELL_LEVEL_S * 2) : DWELL_LEVEL_S;
			if (r.atLevel >= hold) {
				if (full) return end(r, 'completed', 'The dwell finished.');
				nextLevel(r);
			}
		}
		// The near-singular structure: the law asks to slow the sweep near the hard frequency.
		const p = r.last;
		const asking = r.structure === 'sine-near-singular-4x4' && t.kind === 'sweep' && !!p && p.opp > 1.6;
		if (asking && !r.requestHeld) {
			r.requests.asked += 1;
			if (r.config.requests?.honour) r.requests.granted += 1;
			addLog('law', `The law asked to slow the sweep to half its rate${r.config.requests?.honour ? '; granted' : '; recorded, not acted on (the request switch is off)'}.`);
		}
		if (asking && r.config.requests?.honour) r.requests.heldS += dt;
		if (asking !== r.requestHeld) {
			r.requestHeld = asking;
			stateChanged(true);
		}
	}

	/** One timer tick: the run's clock moves in steps of at most 50 ms of its own time, however fast the mock runs. */
	function tick(r: Run) {
		const total = (TICK_MS / 1000) * host.speed;
		const n = Math.max(1, Math.ceil(total / 0.05));
		for (let k = 0; k < n && run === r && r.state !== 'finished'; k++) step(r, total / n);
	}

	function step(r: Run, dt: number) {
		r.t += dt;
		r.phaseT += dt;
		if (r.state === 'running' && r.phase === 'sysid' && r.phaseT >= SYSID_S) {
			r.phase = 'coherence_gate';
			stateChanged(true);
			identify(r);
			return;
		}
		if (r.state === 'stopping') {
			if (r.stopping !== null && r.t >= r.stopping) end(r, 'operator_stop', 'Stopped by the operator.');
			return;
		}
		if (r.state !== 'running' || (r.phase !== 'engaged' && r.phase !== 'full_level')) return;
		const wasNotch = r.notchScale < 0.999;
		const p = pointOf(r);
		r.last = p;
		if (r.config.test.kind === 'dwell') {
			r.history.push(p);
			if (r.history.length > 1400) r.history.splice(0, r.history.length - 1200);
		} else r.pass.push(p);
		trackWorst(r, p, dt);
		const alarm = p.err.some((e, i) => {
			const a = r.config.channels.controls[i].profile.alarm_db ?? [1.5, 1.5];
			return e > a[1] || e < -a[0];
		});
		if (alarm !== r.alarm) {
			r.alarm = alarm;
			addLog('alarm', alarm ? 'A control is outside its alarm band.' : 'Every control is back inside its alarm band.');
			stateChanged(true);
		}
		if (wasNotch !== (r.notchScale < 0.999)) {
			addLog('note', r.notchScale < 0.999 ? `A limit channel holds the drive vector down (notch in at ${round(p.f, 1)} Hz).` : `The notch released at ${round(p.f, 1)} Hz.`);
			stateChanged(true);
		}
		// The softening structure: control 2 passes its abort band at full level.
		const i = 1;
		if (r.controls.length > 1) {
			const ab = r.config.channels.controls[i].profile.abort_db ?? [3, 3];
			if (p.err[i] > ab[1]) return trip(r, { kind: 'abort_band', channel: r.controls[i], side: 'high', measured: round(p.ctl[i], 4), limit: round(p.ref[i] * 10 ** (ab[1] / 20), 4), at_hz: round(p.f, 2), at_level_db: r.levelDb ?? 0, at_tick: Math.round(r.t / r.derived.grid.tick_s) }, `Control ${r.controls[i]} passed its abort band at ${round(p.f, 1)} Hz.`);
		}
		drive(r, dt);
	}

	function startLive() {
		if (liveTimer) return;
		liveTimer = setInterval(() => {
			if (run && isLive()) {
				broadcastLive(run);
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

	const needRun = (): Run => run ?? refuse('refused', 'wrong_state', 'No MIMO Sine run is configured.');
	const wrong = (r: Run, verb: string): never => refuse('refused', 'wrong_state', `${verb} does not apply while the run is ${r.state}.`, { state: r.state, phase: r.phase });
	const needRunning = (verb: string): Run => {
		const r = needRun();
		if (r.state !== 'running' || (r.phase !== 'engaged' && r.phase !== 'full_level')) wrong(r, verb);
		return r;
	};
	const needSweep = (verb: string): Run => {
		const r = needRunning(verb);
		if (r.config.test.kind !== 'sweep') refuse('refused', 'not_a_sweep', `${verb} applies to a sweep only.`);
		return r;
	};

	function newRun(config: MimoSineConfig, derived: MimoSineDerived): Run {
		runCount += 1;
		const st = host.station();
		const structure = structureOf(config);
		const law = LAWS.find((l) => l.id === (config.control?.law?.id ?? 'matrix')) ?? LAWS[0];
		const params: Record<string, unknown> = {};
		for (const p of law.params) params[p.key] = config.control?.law?.params?.[p.key] ?? (p as { default: unknown }).default;
		const nc = config.channels.controls.length;
		const nl = config.limits?.channels?.length ?? 0;
		const nd = config.channels.drives.length;
		const levels = [...(config.schedule?.levels_db ?? [-12, -9, -6, -3, 0])];
		return {
			id: `msine-${String(runCount).padStart(4, '0')}`,
			config,
			derived,
			state: 'configured',
			phase: 'idle',
			source: st.source,
			station: { ...st.ref, structure: st.ref.kind === 'sim' ? structure : null },
			structure,
			controls: config.channels.controls.map((c) => c.channel),
			drives: config.channels.drives.map((d) => d.channel),
			limits: (config.limits?.channels ?? []).map((l) => l.channel),
			levels,
			rung: 0,
			levelDb: null,
			t: 0,
			phaseT: 0,
			atLevel: 0,
			progress: 0,
			direction: 'up',
			passes: 0,
			passesAtLevel: 0,
			steps: stepsOf(config),
			held: false,
			requestHeld: false,
			alarm: false,
			notchScale: 1,
			notchedS: Array(nl).fill(0),
			identification: null,
			review: null,
			end: null,
			params,
			law,
			misses: 0,
			cappedTicks: 0,
			overCeiling: 0,
			rotationMax: 4.2,
			requests: { asked: 0, granted: 0, heldS: 0 },
			pass: [],
			lastPass: [],
			history: [],
			last: null,
			worst: { err: Array(nc).fill(0), errHz: Array(nc).fill(null), ph: Array(nc).fill(null), alarmS: Array(nc).fill(0), depth: Array(nc).fill(0), margin: Array(nl).fill(null), marginHz: Array(nl).fill(null), vpk: Array(nd).fill(0), vpkHz: Array(nd).fill(null), opp: null, share: null },
			stopping: null,
			timer: null,
			record: null
		};
	}

	function armRun(r: Run) {
		const other = host.otherRun();
		if (other) refuse('refused', 'run_active', `Run ${other} holds the station.`, { run_id: other });
		claimRecord(r);
	}

	const DEFAULT_PLOTS: MimoSineSubscribePlot[] = [];
	const QUANTITIES: MimoSineQuantity[] = ['control', 'phase', 'error', 'limit', 'drive', 'drive_phase', 'readings', 'time', 'identification', 'law'];

	const methods: Record<string, MockMethod> = {
		'mimo_sine.configure': (params, conn) => {
			if (!isObject(params.config)) refuse('bad_request', 'bad_params', 'Give the configuration.', { param: 'config' });
			const raw = params.config as Record<string, unknown>;
			if (raw.capture != null) refuse('bad_request', 'bad_params', 'MIMO Sine takes no "capture".', { param: 'config.capture' });
			if (raw.cola != null) refuse('bad_request', 'bad_params', 'MIMO Sine takes no "cola".', { param: 'config.cola' });
			if (!isObject(raw.channels) || !isObject(raw.test)) refuse('bad_request', 'bad_params', 'Give the channels and the test.', { param: !isObject(raw.channels) ? 'config.channels' : 'config.test' });
			const config = raw as unknown as MimoSineConfig;
			const dry = params.dry_run === true;
			if (!dry) host.own(conn);
			if (!dry && isLive()) wrong(run as Run, 'Configure');
			const res = check(config);
			if (dry) return res;
			if (res.errors.length) refuse('bad_request', 'config_invalid', `The configuration has ${res.errors.length} ${res.errors.length === 1 ? 'error' : 'errors'}.`, { errors: res.errors, warnings: res.warnings });
			if (run?.timer) clearInterval(run.timer);
			run = newRun(config, res.derived as MimoSineDerived);
			log = [];
			addLog('state', 'Configured.');
			stateChanged(true);
			return { state: stateOf(run), derived: res.derived, warnings: res.warnings };
		},
		'mimo_sine.config': () => ({ run_id: run?.id ?? null, config: run?.config ?? null, derived: run?.derived ?? null, views: run ? configViews(run.config) : [] }),
		'mimo_sine.state': () => stateOf(run),
		'mimo_sine.arm': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'configured' && r.state !== 'blocked') wrong(r, 'Arm');
			armRun(r);
			r.state = 'armed';
			r.phase = 'armed';
			addLog('station', 'Armed: the station is open; nothing is driven.');
			startLive();
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_sine.start': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'configured' && r.state !== 'armed') wrong(r, 'Start');
			if (r.state === 'configured') armRun(r);
			r.state = 'running';
			r.phase = 'sysid';
			r.phaseT = 0;
			r.timer = setInterval(() => run === r && tick(r), TICK_MS);
			startLive();
			addLog('state', 'Identification: independent random on every drive at once.');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_sine.proceed': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (r.state !== 'review') wrong(r, 'Proceed');
			const ack = typeof params.acknowledge === 'string' ? params.acknowledge.trim() : '';
			if (ack && r.review) {
				r.review = { ...r.review, acknowledged: ack };
				addLog('override', `Proceeded: "${ack}"`);
			}
			engage(r);
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_sine.level': (params, conn) => {
			host.own(conn);
			const r = needRunning('Level');
			if ((r.config.schedule?.mode ?? 'auto') !== 'manual') refuse('refused', 'manual_mode_required', 'The level moves by hand in the manual mode only.');
			let to: number;
			if (typeof params.to_db === 'number') {
				if (!r.levels.includes(params.to_db)) refuse('refused', 'not_a_rung', `${params.to_db} dB is not a level of the schedule.`);
				to = params.to_db;
			} else {
				const idx = r.rung + (typeof params.delta_db === 'number' && params.delta_db < 0 ? -1 : 1);
				to = r.levels[Math.max(0, Math.min(r.levels.length - 1, idx))];
			}
			r.rung = r.levels.indexOf(to);
			r.levelDb = to;
			r.atLevel = 0;
			r.phase = r.rung === r.levels.length - 1 ? 'full_level' : 'engaged';
			addLog('level', `Level ${to} dB, by hand.`);
			stateChanged(true);
			return { level_db: to };
		},
		'mimo_sine.mode': (params, conn) => {
			host.own(conn);
			const r = needRun();
			if (!['configured', 'blocked'].includes(stateOf(r).state)) refuse('refused', 'mode_fixed', "The ladder's mode is fixed once the run has armed; it is set in the configuration (schedule.mode).");
			const mode = params.mode === 'manual' ? 'manual' : 'auto';
			r.config = { ...r.config, schedule: { ...(r.config.schedule ?? {}), mode } };
			stateChanged(true);
			return { mode };
		},
		'mimo_sine.hold': (_p, conn) => {
			host.own(conn);
			const r = needRunning('Hold');
			r.held = true;
			addLog('note', 'Held: the tone stays at its frequency and level, under control.');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_sine.resume': (_p, conn) => {
			host.own(conn);
			const r = needRunning('Resume');
			if (!r.held) wrong(r, 'Resume');
			r.held = false;
			addLog('note', 'Resumed.');
			stateChanged(true);
			return stateOf(r);
		},
		'mimo_sine.reverse': (_p, conn) => {
			host.own(conn);
			needSweep('Reverse');
			return refuse('refused', 'reverse_not_available', "A MIMO Sine sweep turns only at the band's edges; go to a frequency, or stop and start a sweep in the other direction.");
		},
		'mimo_sine.jog': (params, conn) => {
			host.own(conn);
			const r = needSweep('Jog');
			return { frequency_hz: moveTo(r, freqOf(r) + (typeof params.delta_hz === 'number' ? params.delta_hz : 0), true) };
		},
		'mimo_sine.goto': (params, conn) => {
			host.own(conn);
			const r = needSweep('Go to');
			if (typeof params.hz !== 'number') refuse('bad_request', 'bad_params', 'Give the frequency.', { param: 'hz' });
			return { frequency_hz: moveTo(r, params.hz as number, false) };
		},
		'mimo_sine.stop': () => {
			const r = needRun();
			if (!isLive()) wrong(r, 'Stop');
			if (r.state === 'armed' || r.state === 'review' || r.phase === 'sysid') end(r, 'operator_stop', 'Stopped by the operator.');
			else {
				r.state = 'stopping';
				r.phase = 'ramping_down';
				r.stopping = r.t + 0.4;
				stateChanged(true);
			}
			return stateOf(r);
		},
		'mimo_sine.abort': () => {
			const r = needRun();
			if (!isLive()) wrong(r, 'Abort');
			end(r, 'operator_abort', 'Aborted by the operator.');
			return stateOf(r);
		},
		'mimo_sine.reset': (_p, conn) => {
			host.own(conn);
			const r = needRun();
			if (isLive()) wrong(r, 'New Run');
			run = newRun(r.config, r.derived);
			log = [];
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_sine.clear': (_p, conn) => {
			host.own(conn);
			if (isLive()) wrong(run as Run, 'Clear');
			run = null;
			stateChanged(true);
			return stateOf(run);
		},
		'mimo_sine.log': (params) => {
			const since = typeof params.since === 'number' ? params.since : 0;
			return { entries: log.filter((e) => e.seq >= since), next: log.length };
		},
		'mimo_sine.report': () => ({ report: run ? reportOf(run) : null }),
		'mimo_sine.subscribe': (params, conn) => {
			if (params.default === true) {
				subs.delete(conn);
				return { default: true, rate_hz: 5, plots: null, quantities: QUANTITIES };
			}
			if (!Array.isArray(params.plots)) refuse('bad_request', 'bad_params', 'Give the plots, or ask for the default.', { param: 'plots' });
			const plots = (params.plots as MimoSineSubscribePlot[]).map((p) => ({ ...p }));
			if (plots.length > 64) refuse('bad_request', 'bad_params', 'At most 64 plots.', { param: 'plots' });
			for (const p of plots) if (!QUANTITIES.includes(p.quantity)) refuse('bad_request', 'bad_params', `No quantity "${p.quantity}".`, { param: 'plots' });
			const rate = typeof params.rate_hz === 'number' ? params.rate_hz : 5;
			subs.set(conn, { rate, plots });
			queueMicrotask(() => dataFor(conn));
			void DEFAULT_PLOTS;
			return { default: false, rate_hz: rate, plots: plots.map((p) => ({ ...p, channels: p.channels ?? (['control', 'phase', 'limit', 'time', 'identification'].includes(p.quantity) ? 'all' : undefined), points: p.quantity === 'time' ? (p.points ?? 1024) : undefined })), quantities: QUANTITIES };
		}
	};

	function moveTo(r: Run, hz: number, clamp: boolean): number {
		const [lo, hi] = r.derived.band_hz;
		if (!clamp && (hz < lo || hz > hi)) refuse('refused', 'out_of_band', `${hz} Hz is outside the band, ${lo} to ${hi} Hz.`);
		const f = Math.min(hi, Math.max(lo, hz));
		const t = r.config.test;
		const u = t.kind === 'sweep' && t.rate.law === 'linear' ? (f - lo) / (hi - lo) : Math.log(f / lo) / Math.log(hi / lo);
		r.progress = r.direction === 'up' ? u : 1 - u;
		addLog('note', `The frequency moved to ${round(f, 1)} Hz.`);
		return round(f, 2);
	}

	return {
		methods,
		isLive,
		liveId: () => (isLive() && run ? run.id : null),
		detach: (conn) => {
			subs.delete(conn);
		},
		seatExpired: () => {
			if (run && isLive()) end(run, 'safe_stop', 'Nobody held control.');
		},
		laws: () => LAWS
	};
}
