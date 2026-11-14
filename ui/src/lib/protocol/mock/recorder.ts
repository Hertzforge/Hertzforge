/**
 * Mock Recorder (schema/PROTOCOL.md, "Recorder"), on the mock control service's run slot and
 * controller seat.
 *
 * A run opens the station with no outputs and acquires every enabled input row of the table, in
 * table order; drive rows are not opened and not recorded. The inputs carry the simulator's stated
 * source (PROTOCOL, "The Recorder on the Simulator"): row 1 the markers, a 0.5 peak half-sine of
 * 10 ms at 0.5 s in every 2.000 s; row 2 250 Hz at 0.2; row 3 433 Hz at 0.1; row 4 1000 Hz at
 * 0.15; a fifth row and after repeat the four. The source is laid on a small noise, in each row's
 * unit, at the rate the station programmed: the rate asked on the simulator, the input module's
 * ladder (51 200 Hz over 1 to 31) on the NI station.
 *
 * Time is the station's: the run reads in batches of two blocks, every frame number is a station
 * frame from the run's first frame, and a verb takes effect between two batches. The three trigger
 * modes run as PROTOCOL states them: `start` records from station frame 0; `manual` records on
 * `recorder.record` (its trigger the first frame of the next batch), with the pre-window it holds
 * when armed; `level` fires on the first sample of the trigger row at or over the level (rising) or
 * at or under it (falling) once the signal has been past the hysteresis band, takes the pre-window
 * before it, holds exactly `record_s` after it, then re-arms until `count` recordings are saved,
 * waiting out the holdoff from the last trigger in station time and a fresh full pre-window. A
 * row's acceleration abort limit is a mark, marked again only after one second within the limit;
 * the overload indicator latches at 0.9 of the row's range. The free space falls as recordings
 * are written; under the floor at start the run is refused `not_enough_space`.
 *
 * It refuses what the service refuses, by the same reasons and on the same parameters: shape by
 * `bad_params` naming the parameter (a missing required field of a level trigger among them),
 * content by item inside `config_invalid` naming the field, a verb in a state it does not apply
 * to by `wrong_state` with the state and phase, a verb in a trigger mode it does not apply to by
 * `wrong_mode` with the mode, an observer by `not_controller` for everything but stop, abort and
 * the reads, another live run by `run_active`, and a plot key a quantity does not take. It sends
 * `recorder.live` to every connection without a Recorder subscription and `recorder.data` in its
 * place to a subscribed one. Every value is synthetic, made only so the shell has something to
 * draw; nothing here is the service's analysis.
 */

import type { ChannelRow, DataSource, LogEntry, ReadingRow, RefusalReason, Series, SpectrumView, StationRef, View } from '../types';
import type {
	RecorderConfig,
	RecorderDerived,
	RecorderEnd,
	RecorderGap,
	RecorderHealth,
	RecorderItemReason,
	RecorderLiveHead,
	RecorderLiveQuantity,
	RecorderLiveRow,
	RecorderLogKind,
	RecorderMark,
	RecorderOutcome,
	RecorderPhase,
	RecorderRecording,
	RecorderRecordingNow,
	RecorderReportItem,
	RecorderRunState,
	RecorderSlope,
	RecorderState,
	RecorderStopCause,
	RecorderSubscribePlot,
	RecorderTriggerMode,
	RecorderTriggerState,
	RecorderWindow
} from '../recorder';
import type { ShockMockHost } from './shock';
import { MockError, type MockConnection, type MockMethod } from './socket';

/** One pass of the mock's loop, wall-clock ms (times the host's speed in station time). */
const TICK_MS = 40;
/** `recorder.live` ten times a second. */
const LIVE_MS = 100;
const FS_DEFAULT = 12800;
const NOISE = 2e-4;
const GIB = 1073741824;
/** The data volume's free space when the host states none: 100 GiB. */
export const RECORDER_FREE_DEFAULT = 100 * GIB;

const QUANTITIES: RecorderLiveQuantity[] = ['time', 'spectrum', 'frf', 'readings'];
const LIVE: RecorderRunState[] = ['starting', 'running', 'stopping'];
const MODES: RecorderTriggerMode[] = ['start', 'manual', 'level'];
const WINDOWS: RecorderWindow[] = ['hann', 'flattop', 'rectangular', 'blackman_harris'];
const AVERAGINGS = ['linear', 'exponential', 'peak_hold'];
const CONFIG_KEYS = ['title', 'acquisition', 'trigger', 'analysis', 'capture', 'metadata'];
const NESTED: Record<string, string[]> = {
	acquisition: ['fs_hz'],
	trigger: ['mode', 'row', 'level', 'slope', 'hysteresis', 'pre_s', 'record_s', 'count', 'holdoff_s'],
	analysis: ['block', 'window', 'overlap_pct', 'averaging', 'averages', 'decimation', 'frf'],
	capture: ['run_name', 'keep_free_gb']
};
/** Each window's own overlap, percent. */
export const RECORDER_WINDOW_OVERLAP: Record<RecorderWindow, number> = { hann: 50, flattop: 75, rectangular: 0, blackman_harris: 66.7 };

function refuse(code: 'refused' | 'bad_request', reason: RefusalReason | 'wrong_mode' | 'not_enough_space', message: string, detail?: Record<string, unknown>): never {
	throw new MockError(code, message, reason as RefusalReason, detail);
}

function bad(param: string, message: string): never {
	refuse('bad_request', 'bad_params', message, { param });
}

const round = (v: number, d = 3) => Math.round(v * 10 ** d) / 10 ** d;
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const isNum = (v: unknown): v is number => typeof v === 'number' && Number.isFinite(v);
const isInt = (v: unknown): v is number => isNum(v) && Number.isInteger(v);
const isPow2 = (v: number) => Number.isInteger(v) && v > 0 && (v & (v - 1)) === 0;
const f64 = (a: number[]) => Float64Array.from(a);

/** The power of two nearest 20 ms of the rate: the station's block. */
function blockFramesAt(fs: number): number {
	const want = fs * 0.02;
	let p = 1;
	while (p * 2 <= want) p *= 2;
	return want - p <= p * 2 - want ? p : p * 2;
}

/** The NI input module's rate ladder: 51 200 Hz divided by 1 to 31, the nearest to the rate asked. */
function ladderRate(fs: number): number {
	let best = 51200;
	for (let n = 1; n <= 31; n++) {
		const r = 51200 / n;
		if (Math.abs(r - fs) < Math.abs(best - fs)) best = r;
	}
	return best;
}

/** The writer's chunk: a tenth of a second rounded up to a power of two of frames. */
function chunkFramesAt(fs: number): number {
	let p = 1;
	while (p < fs * 0.1) p *= 2;
	return p;
}

/**
 * The simulator's stated source on the k-th recorded row (from 0) at station time t, in the row's
 * unit: the markers, then 250, 433 and 1000 Hz, the fifth row and after repeating the four.
 */
export function recorderSourceAt(k: number, t: number): number {
	switch (k % 4) {
		case 0: {
			const u = t - 0.5 - 2 * Math.floor((t - 0.5) / 2);
			return t >= 0.5 && u < 0.01 ? 0.5 * Math.sin((Math.PI * u) / 0.01) : 0;
		}
		case 1:
			return 0.2 * Math.sin(2 * Math.PI * 250 * t);
		case 2:
			return 0.1 * Math.sin(2 * Math.PI * 433 * t);
		default:
			return 0.15 * Math.sin(2 * Math.PI * 1000 * t);
	}
}

/** Each row's stated tone, or null for the markers. */
const TONES: (null | { hz: number; amp: number })[] = [null, { hz: 250, amp: 0.2 }, { hz: 433, amp: 0.1 }, { hz: 1000, amp: 0.15 }];

/** A small deterministic noise. */
const noiseAt = (n: number, k: number) => NOISE * Math.sin(n * 12.9898 + k * 78.233) * Math.cos(n * 4.1414 + k);

type Item = RecorderReportItem;
const item = (reason: RecorderItemReason, message: string, field?: string, channel?: number): Item => {
	const out: Item = { reason, message };
	if (field) out.field = field;
	if (channel !== undefined) out.channel = channel;
	return out;
};

/** The configuration with every default filled in. */
interface Norm {
	title: string;
	fs: number;
	mode: RecorderTriggerMode;
	row: number | null;
	level: number | null;
	slope: RecorderSlope;
	hysteresis: number | null;
	pre_s: number;
	record_s: number | null;
	count: number | null;
	holdoff_s: number;
	block: number;
	window: RecorderWindow;
	overlap: number;
	averaging: string;
	averages: number;
	decimation: number;
	frf: { reference: number; response: number } | null;
	run_name: string;
	keep_free_gb: number;
}

function normOf(c: RecorderConfig): Norm {
	const t = c.trigger ?? {};
	const a = c.analysis ?? {};
	const window = a.window ?? 'hann';
	return {
		title: c.title ?? '',
		fs: c.acquisition?.fs_hz ?? FS_DEFAULT,
		mode: t.mode ?? 'manual',
		row: t.row ?? null,
		level: t.level ?? null,
		slope: t.slope ?? 'rising',
		hysteresis: t.hysteresis ?? null,
		pre_s: t.pre_s ?? 1.0,
		record_s: t.record_s ?? null,
		count: t.count === undefined ? 1 : t.count,
		holdoff_s: t.holdoff_s ?? 0,
		block: a.block ?? 2048,
		window,
		overlap: a.overlap_pct ?? RECORDER_WINDOW_OVERLAP[window],
		averaging: a.averaging ?? 'linear',
		averages: a.averages ?? 8,
		decimation: a.decimation ?? 1,
		frf: a.frf ?? null,
		run_name: c.capture?.run_name ?? 'recorder',
		keep_free_gb: c.capture?.keep_free_gb ?? 1
	};
}

/** A recording, in progress or saved. */
interface Rec {
	name: string;
	trigger: RecorderTriggerMode;
	first: number;
	trig: number;
	pre: number;
	preAsked: number;
	/** The frames it holds when done: the pre-window and the length; null until cut. */
	target: number | null;
	fed: number;
	marks: number;
	gaps: RecorderGap[];
	complete: boolean;
	fault: string | null;
	requestedFs: number;
	fs: number;
	groupDelay: number;
	rows: number[];
}

interface Run {
	id: string;
	config: RecorderConfig;
	norm: Norm;
	derived: RecorderDerived;
	state: RecorderRunState;
	phase: RecorderPhase;
	station: StationRef;
	source: DataSource;
	requestedFs: number;
	fs: number;
	block: number;
	batch: number;
	groupDelay: number;
	chunk: number;
	/** The recorded rows at start, in table order. */
	rows: ChannelRow[];
	folder: string | null;
	/** Station frames acquired: station time is `frames / fs`. */
	frames: number;
	// the trigger
	armed: boolean;
	held: number;
	/** The level detector has seen the signal past the hysteresis band since it (re)armed. */
	detectorReady: boolean;
	/** A re-armed trigger waits for a full fresh pre-window. */
	needFullPre: boolean;
	fires: number;
	lastFire: number | null;
	saved: number;
	begun: number;
	// the recordings
	rec: Rec | null;
	recs: Rec[];
	marks: RecorderMark[];
	// per row
	peak: number[];
	overload: boolean[];
	rowMarks: number[];
	/** The last frame each row was past its acceleration limit, or null. */
	lastOver: (number | null)[];
	/** Sums of squares of the last second, a batch at a time. */
	ms: { sum: number; n: number }[][];
	stop: RecorderStopCause | null;
	stopBy: string | undefined;
	stopOutcome: RecorderOutcome | null;
	end: RecorderEnd | null;
	health: RecorderHealth;
	timer: ReturnType<typeof setInterval> | null;
}

export interface RecorderMock {
	methods: Record<string, MockMethod>;
	isLive(): boolean;
	/** The run's id while it is live, for another run's `run_active` refusal. */
	liveId(): string | null;
	detach(conn: MockConnection): void;
	/** Nobody held control for the timeout: the run stops, every recording saved. */
	seatExpired(): void;
}

/** What the Recorder mock needs of the control service around it: classical shock's, and the free space. */
export interface RecorderMockHost extends ShockMockHost {
	/** The data volume's free bytes before any recording; null when it cannot be read. */
	freeBytes?: number | null;
}

export function createRecorderMock(host: RecorderMockHost): RecorderMock {
	let run: Run | null = null;
	let runCount = 0;
	let log: LogEntry[] = [];
	let lastStateKey = '';
	const folders = new Set<string>();
	const freeAtStart = host.freeBytes === undefined ? RECORDER_FREE_DEFAULT : host.freeBytes;
	let written = 0;
	const subs = new Map<MockConnection, { rate: number; plots: RecorderSubscribePlot[]; acc: number }>();
	let liveTimer: ReturnType<typeof setInterval> | null = null;

	const isLive = () => run !== null && LIVE.includes(run.state);
	const enabled = () => host.rows().filter((r) => r.enabled);
	const inputs = () => enabled().filter((r) => r.role !== 'drive');
	const drives = () => enabled().filter((r) => r.role === 'drive');
	const freeNow = () => (freeAtStart === null ? null : Math.max(0, freeAtStart - written));
	const tSec = (r: Run) => r.frames / r.fs;

	function addLog(kind: RecorderLogKind, text: string) {
		const e = { seq: log.length, at_unix: Date.now() / 1000, kind, text } as LogEntry;
		log.push(e);
		if (run) host.broadcast('recorder.log', { run_id: run.id, entries: [e] });
	}

	// ---- the station's figures and the derived numbers ------------------------------------------

	const isNi = () => host.station().ref.kind !== 'sim';

	function figuresAt(fs: number) {
		const block = blockFramesAt(fs);
		const groupDelay = isNi() ? round(38.4 + 3.2e-6 * fs, 2) : 0;
		return { block, batch: 2 * block, groupDelay };
	}

	function floorOf(n: Norm, fs: number, rows: number): number {
		const keep = n.keep_free_gb * GIB;
		if (n.record_s === null) return keep;
		const frames = Math.round(n.pre_s * fs) + Math.round(n.record_s * fs);
		return Math.max(keep, 2 * frames * 4 * rows);
	}

	function derivedOf(n: Norm, fs: number, actual: boolean, rows: number[]): RecorderDerived {
		const fig = figuresAt(fs);
		const fa = fs / n.decimation;
		const hop = n.block * (1 - n.overlap / 100);
		const bytes = 4 * rows.length * fs;
		const floor = floorOf(n, fs, rows.length);
		const free = freeNow();
		return {
			grid: { fs_hz: round(fs, 6), block_frames: fig.block, batch_frames: fig.batch, group_delay_frames: fig.groupDelay, actual },
			outputs: 'none',
			rows: [...rows],
			analysis: {
				fs_hz: round(fa, 6),
				df_hz: round(fa / n.block, 6),
				lines: Math.floor(n.block / 2.56),
				span_hz: round(fa / 2.56, 6),
				block_s: round(n.block / fa, 6),
				overlap_pct: n.overlap,
				averages_s: round((n.block + (n.averages - 1) * hop) / fa, 6)
			},
			trigger: { pre_frames: Math.round(n.pre_s * fs), record_frames: n.record_s === null ? null : Math.round(n.record_s * fs), holdoff_frames: Math.round(n.holdoff_s * fs) },
			disk: { bytes_per_s: round(bytes, 3), floor_bytes: floor, free_bytes: free, seconds_above_floor: free === null || bytes <= 0 ? null : Math.max(0, Math.floor((free - floor) / bytes)) }
		};
	}

	// ---- checking a configuration ---------------------------------------------------------------

	/** Shape first, as the service reads the request: a key the configuration does not have, a wrong type, a missing field. */
	function shapeOk(c: Record<string, unknown>) {
		for (const k of Object.keys(c)) if (!CONFIG_KEYS.includes(k)) bad(`config.${k}`, `The configuration has no "${k}".`);
		for (const [k, keys] of Object.entries(NESTED)) {
			if (c[k] === undefined) continue;
			if (!isObject(c[k])) bad(`config.${k}`, `"${k}" is an object.`);
			for (const kk of Object.keys(c[k] as object)) if (!keys.includes(kk)) bad(`config.${k}.${kk}`, `"${k}" has no "${kk}".`);
		}
		if (c.title !== undefined && typeof c.title !== 'string') bad('config.title', '"title" is text.');
		const acq = (c.acquisition ?? {}) as Record<string, unknown>;
		if (acq.fs_hz !== undefined && !isNum(acq.fs_hz)) bad('config.acquisition.fs_hz', '"fs_hz" is a number of hertz.');
		const t = (c.trigger ?? {}) as Record<string, unknown>;
		if (t.mode !== undefined && !MODES.includes(t.mode as RecorderTriggerMode)) bad('config.trigger.mode', '"mode" is "start", "manual" or "level".');
		if (t.row !== undefined && t.row !== null && !isInt(t.row)) bad('config.trigger.row', '"row" is a row\'s index, a whole number.');
		if (t.level !== undefined && t.level !== null && !isNum(t.level)) bad('config.trigger.level', '"level" is a number in the row\'s unit.');
		if (t.slope !== undefined && t.slope !== 'rising' && t.slope !== 'falling') bad('config.trigger.slope', '"slope" is "rising" or "falling".');
		for (const k of ['hysteresis', 'record_s']) if (t[k] !== undefined && t[k] !== null && !isNum(t[k])) bad(`config.trigger.${k}`, `"${k}" is a number, or null.`);
		for (const k of ['pre_s', 'holdoff_s']) if (t[k] !== undefined && !isNum(t[k])) bad(`config.trigger.${k}`, `"${k}" is a number of seconds.`);
		if (t.count !== undefined && t.count !== null && !isInt(t.count)) bad('config.trigger.count', '"count" is a whole number, or null.');
		if (t.mode === 'level') {
			if (t.row === undefined || t.row === null) bad('config.trigger.row', 'A level trigger needs the row its level is read on.');
			if (t.level === undefined || t.level === null) bad('config.trigger.level', 'A level trigger needs its level.');
			if (t.record_s === undefined || t.record_s === null) bad('config.trigger.record_s', 'A level recording needs its length, so the trigger can re-arm.');
		}
		const a = (c.analysis ?? {}) as Record<string, unknown>;
		for (const k of ['block', 'averages', 'decimation']) if (a[k] !== undefined && !isInt(a[k])) bad(`config.analysis.${k}`, `"${k}" is a whole number.`);
		if (a.window !== undefined && !WINDOWS.includes(a.window as RecorderWindow)) bad('config.analysis.window', `"window" is one of ${WINDOWS.join(', ')}.`);
		if (a.averaging !== undefined && !AVERAGINGS.includes(a.averaging as string)) bad('config.analysis.averaging', `"averaging" is one of ${AVERAGINGS.join(', ')}.`);
		if (a.overlap_pct !== undefined && a.overlap_pct !== null && !isNum(a.overlap_pct)) bad('config.analysis.overlap_pct', '"overlap_pct" is a number, or null.');
		if (a.frf !== undefined && a.frf !== null) {
			if (!isObject(a.frf)) bad('config.analysis.frf', '"frf" is {reference, response}, or null.');
			const f = a.frf as Record<string, unknown>;
			for (const k of Object.keys(f)) if (k !== 'reference' && k !== 'response') bad(`config.analysis.frf.${k}`, `"frf" has no "${k}".`);
			for (const k of ['reference', 'response']) if (!isInt(f[k])) bad(`config.analysis.frf.${k}`, `"${k}" is a row's index.`);
		}
		const cap = (c.capture ?? {}) as Record<string, unknown>;
		if (cap.run_name !== undefined && typeof cap.run_name !== 'string') bad('config.capture.run_name', '"run_name" is text.');
		if (cap.keep_free_gb !== undefined && !isNum(cap.keep_free_gb)) bad('config.capture.keep_free_gb', '"keep_free_gb" is a number of GiB.');
		if (c.metadata !== undefined && !(Array.isArray(c.metadata) && c.metadata.every((m) => isObject(m) && typeof m.key === 'string' && typeof m.value === 'string'))) bad('config.metadata', '"metadata" is a list of {key, label, value}.');
	}

	/** The table's findings, at configure and again at start. */
	function tableItems(): { errors: Item[]; warnings: Item[] } {
		const errors: Item[] = [];
		const warnings: Item[] = [];
		if (!inputs().length) errors.push(item('no_input_channel', 'The channel table has no enabled input row: there is nothing to record.'));
		for (const r of drives()) warnings.push(item('drive_channel_present', `Row ${r.index} is a drive row; a Recorder run does not open or record it.`, undefined, r.index));
		return { errors, warnings };
	}

	function check(c: RecorderConfig): { errors: Item[]; warnings: Item[]; derived: RecorderDerived | null } {
		const n = normOf(c);
		const errors: Item[] = [];
		const rec = inputs();
		const byIndex = new Map(rec.map((r) => [r.index, r]));
		if (!(n.fs >= 1000 && n.fs <= 204800)) errors.push(item('out_of_range', 'The rate is from 1000 to 204800 Hz.', 'acquisition.fs_hz'));
		if (n.mode === 'level' && n.row !== null) {
			const row = byIndex.get(n.row);
			if (!row) errors.push(item('row_not_recorded', `Row ${n.row} is not an enabled input row, so it is not recorded.`, 'trigger.row'));
			else {
				if (n.level !== null && !(Math.abs(n.level) <= row.range_max)) errors.push(item('out_of_range', `The level is within row ${row.index}'s range, ${row.range_max} ${row.units} either side of zero.`, 'trigger.level'));
				if (n.hysteresis !== null && !(n.hysteresis > 0 && n.hysteresis <= row.range_max)) errors.push(item('out_of_range', `The hysteresis is above 0 and at most row ${row.index}'s range, ${row.range_max} ${row.units}.`, 'trigger.hysteresis'));
			}
		}
		if (!(n.pre_s >= 0 && n.pre_s <= 10)) errors.push(item('out_of_range', 'The pre-window is from 0 to 10 s.', 'trigger.pre_s'));
		if (n.record_s !== null && !(n.record_s > 0)) errors.push(item('out_of_range', 'A recording is longer than 0 s.', 'trigger.record_s'));
		if (n.count !== null && !(n.count >= 1)) errors.push(item('out_of_range', 'The count is at least 1.', 'trigger.count'));
		if (!(n.holdoff_s >= 0)) errors.push(item('out_of_range', 'The holdoff is at least 0 s.', 'trigger.holdoff_s'));
		if (!isPow2(n.block)) errors.push(item('not_power_of_two', 'The analysis block is a power of two.', 'analysis.block'));
		else if (!(n.block >= 256 && n.block <= 65536)) errors.push(item('out_of_range', 'The analysis block is from 256 to 65536 frames.', 'analysis.block'));
		if (!isPow2(n.decimation)) errors.push(item('not_power_of_two', 'The decimation is a power of two.', 'analysis.decimation'));
		else if (!(n.decimation >= 1 && n.decimation <= 256)) errors.push(item('out_of_range', 'The decimation is from 1 to 256.', 'analysis.decimation'));
		if (!(n.overlap >= 0 && n.overlap <= 95)) errors.push(item('out_of_range', 'The overlap is from 0 to 95%.', 'analysis.overlap_pct'));
		if (!(n.averages >= 1)) errors.push(item('out_of_range', 'At least 1 average.', 'analysis.averages'));
		if (n.frf) {
			if (n.frf.reference === n.frf.response) errors.push(item('frf_same_row', 'The FRF pair is two different rows.', 'analysis.frf'));
			else
				for (const k of ['reference', 'response'] as const)
					if (!byIndex.has(n.frf[k])) errors.push(item('row_not_recorded', `Row ${n.frf[k]} is not an enabled input row, so it is not recorded.`, `analysis.frf.${k}`));
		}
		if (!n.run_name.trim() || /[\\/]/.test(n.run_name) || n.run_name.includes('..')) errors.push(item('out_of_range', 'The run folder is a plain name, not a path.', 'capture.run_name'));
		if (!(n.keep_free_gb >= 1)) errors.push(item('out_of_range', 'The free space kept is at least 1 GiB.', 'capture.keep_free_gb'));
		const table = tableItems();
		errors.push(...table.errors);
		if (errors.length) return { errors, warnings: table.warnings, derived: null };
		return { errors, warnings: table.warnings, derived: derivedOf(n, n.fs, false, rec.map((r) => r.index)) };
	}

	// ---- the state --------------------------------------------------------------------------------

	function emptyState(): RecorderState {
		return {
			run_id: null,
			state: 'unconfigured',
			phase: 'idle',
			title: null,
			source: null,
			station: null,
			grid: null,
			clock: null,
			outputs: null,
			rows: null,
			mode: null,
			t_s: null,
			station_frames: null,
			trigger: null,
			recording: null,
			recordings: 0,
			marks: 0,
			disk: null,
			health: null,
			stop: null,
			end: null,
			record: null
		};
	}

	const started = (r: Run) => r.state !== 'configured' && r.state !== 'starting' && !(r.state === 'finished' && r.end?.outcome === 'arm_refused');

	function triggerOf(r: Run): RecorderTriggerState | null {
		const n = r.norm;
		if (n.mode === 'start') return null;
		const hold = n.mode === 'level' && r.lastFire !== null ? Math.max(0, r.lastFire + r.derived.trigger.holdoff_frames - r.frames) : 0;
		return {
			armed: r.armed,
			pre_frames: r.derived.trigger.pre_frames,
			held_frames: r.armed ? r.held : 0,
			fires: r.fires,
			last_station_frame: r.lastFire,
			saved: r.saved,
			left: n.mode === 'level' && n.count !== null ? Math.max(0, n.count - r.begun) : null,
			holdoff_left_s: round(hold / r.fs, 3)
		};
	}

	function recordingOf(r: Run): RecorderRecordingNow | null {
		const x = r.rec;
		if (!x) return null;
		const onDisk = onDiskOf(r, x);
		return {
			name: x.name,
			trigger: x.trigger,
			first_station_frame: x.first,
			trigger_station_frame: x.trig,
			pre_frames: x.pre,
			pre_frames_asked: x.preAsked,
			frames_fed: x.fed,
			frames_on_disk: onDisk,
			target_frames: x.target,
			seconds: round(Math.max(0, x.fed - x.pre) / r.fs, 3),
			length_s: r.norm.record_s,
			backlog_s: round((x.fed - onDisk) / r.fs, 3)
		};
	}

	/** On disk: the writer keeps whole chunks until the recording is finished. */
	const onDiskOf = (r: Run, x: Rec) => (x.complete ? x.fed : Math.floor(x.fed / r.chunk) * r.chunk);

	function diskOf(r: Run) {
		const free = freeNow();
		const floor = r.derived.disk.floor_bytes;
		const bytes = r.derived.disk.bytes_per_s;
		return { free_bytes: free, floor_bytes: floor, seconds_above_floor: free === null ? null : Math.max(0, Math.floor((free - floor) / bytes)), fault: null };
	}

	function stateOf(r: Run | null): RecorderState {
		if (!r) return emptyState();
		const on = started(r);
		return {
			run_id: r.id,
			state: r.state,
			phase: r.phase,
			title: r.norm.title,
			source: r.source,
			station: r.station,
			grid: { requested_fs_hz: r.requestedFs, fs_hz: round(r.fs, 6), block_frames: r.block, batch_frames: r.batch, group_delay_frames: r.groupDelay, actual: on },
			clock: on ? 'shared' : null,
			outputs: 'none',
			rows: [...r.derived.rows],
			mode: r.norm.mode,
			t_s: on ? round(tSec(r), 3) : null,
			station_frames: on ? r.frames : null,
			trigger: triggerOf(r),
			recording: recordingOf(r),
			recordings: r.recs.filter((x) => x.complete).length,
			marks: r.marks.length,
			disk: diskOf(r),
			health: on ? { ...r.health, pass_ms: { ...r.health.pass_ms } } : null,
			stop: r.stop,
			end: r.end,
			record: r.folder ? { folder: r.folder, problem: null } : null
		};
	}

	function stateChanged(force = false) {
		const s = stateOf(run);
		const key = JSON.stringify([s.run_id, s.state, s.phase, s.stop, s.end]);
		if (!force && key === lastStateKey) return;
		lastStateKey = key;
		host.broadcast('recorder.state', s as unknown as Record<string, unknown>);
	}

	function rephase(r: Run) {
		if (r.state !== 'running') return;
		const p: RecorderPhase = r.rec ? 'recording' : r.armed ? 'armed' : 'monitoring';
		if (p !== r.phase) {
			r.phase = p;
			stateChanged();
		}
	}

	// ---- the run ----------------------------------------------------------------------------------

	function newRun(config: RecorderConfig, derived: RecorderDerived): Run {
		const st = host.station();
		const n = normOf(config);
		const fig = figuresAt(n.fs);
		return {
			id: `recorder-${String(++runCount).padStart(4, '0')}`,
			config: structuredClone(config),
			norm: n,
			derived,
			state: 'configured',
			phase: 'idle',
			station: st.ref,
			source: st.source,
			requestedFs: n.fs,
			fs: n.fs,
			...fig,
			chunk: chunkFramesAt(n.fs),
			rows: [],
			folder: null,
			frames: 0,
			armed: false,
			held: 0,
			detectorReady: false,
			needFullPre: false,
			fires: 0,
			lastFire: null,
			saved: 0,
			begun: 0,
			rec: null,
			recs: [],
			marks: [],
			peak: [],
			overload: [],
			rowMarks: [],
			lastOver: [],
			ms: [],
			stop: null,
			stopBy: undefined,
			stopOutcome: null,
			end: null,
			health: { batches: 0, blocks_dropped: 0, driver_warnings: 0, station_errors: 0, pass_ms: { last: null, worst: null }, batch_gap_ms: null },
			timer: null
		};
	}

	const recName = (r: Run) => `rec_${String(r.recs.length + 1).padStart(4, '0')}`;

	function begin(r: Run, trigger: RecorderTriggerMode, trig: number, pre: number): Rec {
		const n = r.norm;
		const preAsked = trigger === 'start' ? 0 : r.derived.trigger.pre_frames;
		const x: Rec = {
			name: recName(r),
			trigger,
			first: trig - pre,
			trig,
			pre,
			preAsked,
			target: n.record_s === null ? null : pre + Math.round(n.record_s * r.fs),
			fed: pre,
			marks: 0,
			gaps: [],
			complete: false,
			fault: null,
			requestedFs: r.requestedFs,
			fs: r.fs,
			groupDelay: r.groupDelay,
			rows: r.rows.map((row) => row.index)
		};
		r.rec = x;
		r.recs.push(x);
		written += pre * 4 * x.rows.length;
		if (trigger !== 'start') {
			r.fires += 1;
			r.lastFire = trig;
			r.begun += 1;
		}
		r.armed = false;
		r.held = 0;
		addLog('recording', `${x.name} began at station frame ${trig} (pre-window ${pre} frames)`);
		return x;
	}

	/** The recording in progress is saved, holding `fed` frames. */
	function save(r: Run) {
		const x = r.rec;
		if (!x) return;
		x.complete = true;
		r.rec = null;
		r.saved += 1;
		addLog('recording', `${x.name} saved: ${x.fed} frames, ${(x.fed / r.fs).toFixed(3)} s, ${x.gaps.length ? `${x.gaps.length} gaps` : 'no gaps'}`);
		if (r.state !== 'running') return;
		// A level trigger re-arms by itself until it has saved its count; any other recording returns the run to monitoring.
		const n = r.norm;
		if (x.trigger === 'level' && (n.count === null || r.begun < n.count)) {
			r.armed = true;
			r.held = 0;
			r.detectorReady = false;
			r.needFullPre = true;
		}
	}

	/** Frames fed so far to the recording in progress, on the batch's last frame acquired. */
	function feed(r: Run, upTo: number) {
		const x = r.rec;
		if (!x) return;
		const fed = upTo - x.first;
		const was = x.fed;
		x.fed = Math.max(x.fed, x.target === null ? fed : Math.min(x.target, fed));
		// The free space falls by what the recording writes.
		written += (x.fed - was) * 4 * x.rows.length;
	}

	function mark(r: Run, m: RecorderMark, k: number) {
		r.marks.push(m);
		r.rowMarks[k] += 1;
		if (r.rec) r.rec.marks += 1;
		if (m.kind === 'limit') addLog('mark', `row ${m.row} passed its ${m.axis} limit ${m.limit} at station frame ${m.station_frame}: ${m.value}`);
		else addLog('overload', `row ${m.row} reached ${m.value}, over 0.9 of its range (${m.level}), at station frame ${m.station_frame}; the indicator holds for the run`);
	}

	/** One batch of two blocks: every sample of every recorded row, the trigger, the marks and the recordings. */
	function batch(r: Run) {
		const n = r.norm;
		const f0 = r.frames;
		const f1 = f0 + r.batch;
		const rows = r.rows;
		const trigK = n.mode === 'level' && n.row !== null ? rows.findIndex((x) => x.index === n.row) : -1;
		const trigRow = trigK >= 0 ? rows[trigK] : null;
		const level = n.level ?? 0;
		const hyst = n.hysteresis ?? (trigRow ? 0.01 * trigRow.range_max : 0);
		const rising = n.slope === 'rising';
		const preFrames = r.derived.trigger.pre_frames;
		const holdoff = r.derived.trigger.holdoff_frames;
		const sums = rows.map(() => ({ sum: 0, n: 0 }));
		for (let f = f0; f < f1; f++) {
			const t = f / r.fs;
			// A timed recording ends after exactly its frames; the rest of the batch goes on to the trigger.
			if (r.rec && r.rec.target !== null && f >= r.rec.first + r.rec.target) {
				feed(r, r.rec.first + r.rec.target);
				save(r);
			}
			for (let k = 0; k < rows.length; k++) {
				const row = rows[k];
				const v = recorderSourceAt(k, t) + noiseAt(f, k);
				const a = Math.abs(v);
				sums[k].sum += v * v;
				sums[k].n += 1;
				if (a > r.peak[k]) r.peak[k] = a;
				const over = 0.9 * row.range_max;
				if (!r.overload[k] && a >= over) {
					r.overload[k] = true;
					mark(r, { kind: 'overload', row: row.index, value: round(v, 6), level: round(over, 6), station_frame: f, recording: r.rec?.name ?? null }, k);
				}
				const lim = row.units === 'g' ? row.abort.accel_g : null;
				if (lim !== null && lim !== undefined && a >= lim) {
					const last = r.lastOver[k];
					if (last === null || f - last > r.fs) mark(r, { kind: 'limit', row: row.index, axis: 'accel', value: round(v, 6), limit: lim, station_frame: f, recording: r.rec?.name ?? null }, k);
					r.lastOver[k] = f;
				}
				if (k === trigK && r.armed && !r.rec) {
					if (!r.detectorReady) r.detectorReady = rising ? v < level - hyst : v > level + hyst;
					else if ((rising ? v >= level : v <= level) && (r.lastFire === null || f >= r.lastFire + holdoff) && (!r.needFullPre || r.held >= preFrames)) {
						begin(r, 'level', f, Math.min(r.held, preFrames));
						r.needFullPre = false;
					}
				}
			}
			// The pre-window holds the frames acquired since the arm, up to its length.
			if (r.armed && !r.rec) r.held = Math.min(preFrames, r.held + 1);
		}
		r.frames = f1;
		feed(r, f1);
		if (r.rec && r.rec.target !== null && r.rec.fed >= r.rec.target) save(r);
		const perSecond = Math.max(1, Math.ceil(r.fs / r.batch));
		for (let k = 0; k < rows.length; k++) {
			r.ms[k].push(sums[k]);
			if (r.ms[k].length > perSecond) r.ms[k].shift();
		}
		const h = r.health;
		h.batches += 1;
		const pass = round(0.6 + 0.4 * Math.abs(Math.sin(f1)), 2);
		h.pass_ms = { last: pass, worst: Math.max(h.pass_ms.worst ?? 0, pass) };
		if (h.batches >= 2) h.batch_gap_ms = Math.max(h.batch_gap_ms ?? 0, round(1.5 + Math.abs(Math.sin(f1 * 0.37)), 2));
	}

	function endMessage(r: Run, outcome: RecorderOutcome): string {
		switch (outcome) {
			case 'operator_stop':
				return `Stopped by ${r.stopBy ?? 'the operator'}: every recording is saved.`;
			case 'operator_abort':
				return `Aborted by ${r.stopBy ?? 'the operator'}: every recording is saved.`;
			case 'safe_stop':
				return 'Nobody held control: the run stopped and every recording is saved.';
			case 'arm_refused':
				return 'The station refused the start; nothing was recorded.';
			default:
				return 'The station or the service loop failed.';
		}
	}

	/** Acquisition ends at the next batch: the recording in progress is saved with every frame acquired. */
	function finish(r: Run) {
		if (r.timer) clearInterval(r.timer);
		r.timer = null;
		save(r);
		const outcome = r.stopOutcome ?? 'operator_stop';
		r.armed = false;
		r.state = 'finished';
		r.phase = outcome === 'operator_abort' ? 'aborted' : outcome === 'operator_stop' || outcome === 'safe_stop' ? 'stopped' : 'faulted';
		r.end = { outcome, message: endMessage(r, outcome), at_s: round(tSec(r), 3), recordings: r.recs.filter((x) => x.complete).length };
		stateChanged();
		addLog('state', `ended ${outcome}: ${r.end.message}`);
		publish(r);
	}

	function beginStop(r: Run, cause: RecorderStopCause, outcome: RecorderOutcome, by?: string) {
		r.stop = cause;
		r.stopBy = by;
		r.stopOutcome = outcome;
		r.state = 'stopping';
		r.phase = 'closing';
		addLog('state', `stop began: ${cause}`);
		stateChanged();
	}

	function tick(r: Run) {
		if (run !== r || r.state === 'finished') return;
		if (r.state === 'stopping') {
			finish(r);
			return;
		}
		const want = (TICK_MS / 1000) * host.speed * r.fs;
		const batches = Math.max(1, Math.round(want / r.batch));
		for (let b = 0; b < batches && r.state === 'running'; b++) batch(r);
		rephase(r);
	}

	// ---- display ----------------------------------------------------------------------------------

	const titleOf = (row: ChannelRow) => `Channel ${row.index}: ${row.description.trim() || row.role}`;

	function liveRows(r: Run): RecorderLiveRow[] {
		return r.rows.map((row, k) => {
			const sec = r.ms[k] ?? [];
			const sum = sec.reduce((s, b) => s + b.sum, 0);
			const cnt = sec.reduce((s, b) => s + b.n, 0);
			return {
				index: row.index,
				description: row.description,
				role: row.role as RecorderLiveRow['role'],
				unit: row.units,
				rms: cnt ? round(Math.sqrt(sum / cnt), 6) : null,
				peak: cnt ? round(r.peak[k], 6) : null,
				overload: r.overload[k],
				marks: r.rowMarks[k]
			};
		});
	}

	function head(r: Run): RecorderLiveHead {
		const s = stateOf(r);
		return {
			run_id: r.id,
			source: r.source,
			class: r.station.class,
			state: r.state as RecorderLiveHead['state'],
			phase: r.phase,
			t_s: s.t_s,
			station_frames: s.station_frames,
			grid: { requested_fs_hz: r.requestedFs, fs_hz: round(r.fs, 6), group_delay_frames: r.groupDelay },
			rows: liveRows(r),
			trigger: s.trigger,
			recording: s.recording,
			recordings: s.recordings,
			marks: s.marks,
			disk: s.disk,
			health: s.health
		};
	}

	function readings(r: Run): View {
		const s = stateOf(r);
		const none = (v: number | null | undefined, d = 3) => (v === null || v === undefined ? 'none' : round(v, d));
		const rows: ReadingRow[] = [
			{ label: 'Rate', value: round(r.fs, 3), unit: 'Hz', status: 'info' },
			{ label: 'Asked', value: r.requestedFs, unit: 'Hz', status: 'info' },
			{ label: 'Group delay', value: r.groupDelay, unit: 'frames', status: 'info' },
			{ label: 'Recording', value: s.recording?.name ?? 'none', unit: null, status: 'info' },
			{ label: 'Recorded', value: none(s.recording?.seconds), unit: 's', status: 'info' },
			{ label: 'Backlog', value: none(s.recording?.backlog_s), unit: 's', status: 'info' },
			{ label: 'Saved', value: s.recordings, unit: null, status: 'info' },
			{ label: 'Fires', value: s.trigger ? s.trigger.fires : 'none', unit: null, status: 'info' },
			{ label: 'Marks', value: s.marks, unit: null, status: 'info' },
			{ label: 'Free above floor', value: none(s.disk?.seconds_above_floor, 0), unit: 's', status: 'info' }
		];
		return { view: 'reading', title: 'Run', rows };
	}

	function resolve(r: Run, sel: RecorderSubscribePlot['channels']): { row: ChannelRow; k: number }[] {
		const all = r.rows.map((row, k) => ({ row, k }));
		return Array.isArray(sel) ? all.filter((x) => sel.includes(x.row.index)) : all;
	}

	/** The largest and smallest of row k's samples over frames [n0, n1). */
	function envelope(r: Run, k: number, n0: number, n1: number): [number, number] {
		const tone = TONES[k % 4];
		if (tone && (n1 - n0) / r.fs >= 1 / tone.hz) return [tone.amp + NOISE, -tone.amp - NOISE];
		let hi = -Infinity;
		let lo = Infinity;
		const steps = Math.min(n1 - n0, 32);
		for (let i = 0; i < steps; i++) {
			const f = Math.floor(n0 + ((n1 - n0) * i) / steps);
			const v = recorderSourceAt(k, f / r.fs) + noiseAt(f, k);
			hi = Math.max(hi, v);
			lo = Math.min(lo, v);
		}
		if (k % 4 === 0) {
			// The marker's own peak, when the interval holds it.
			const t0 = n0 / r.fs;
			const tk = 0.5 + 2 * Math.ceil((t0 - 0.505) / 2) + 0.005;
			if (tk >= t0 && tk < n1 / r.fs) hi = Math.max(hi, 0.5);
		}
		return [hi, lo];
	}

	function timeView(r: Run, sel: { row: ChannelRow; k: number }[], points: number, seconds: number): { view: SpectrumView; first: number } | null {
		if (!sel.length || r.frames <= 0) return null;
		const want = Math.round(seconds * r.fs);
		const first = Math.max(0, r.frames - want);
		const span = r.frames - first;
		const n = Math.min(points, span);
		const x: number[] = [];
		for (let i = 0; i < n; i++) x.push(round((first + (span * i) / n) / r.fs, 6));
		const series: Series[] = sel.map(({ row, k }) => {
			const y: number[] = [];
			const ymin: number[] = [];
			for (let i = 0; i < n; i++) {
				const [hi, lo] = envelope(r, k, Math.floor(first + (span * i) / n), Math.max(Math.floor(first + (span * (i + 1)) / n), Math.floor(first + (span * i) / n) + 1));
				y.push(round(hi, 6));
				ymin.push(round(lo, 6));
			}
			return { name: `Channel ${row.index}`, role: 'measured', x: f64(x), y: f64(y), y_min: f64(ymin) };
		});
		return { view: { view: 'spectrum', title: 'Time', x: { label: 'Time', unit: 's', scale: 'linear' }, y: { label: 'Amplitude', unit: sel[0].row.units, scale: 'linear' }, series }, first };
	}

	const averaged = (r: Run) => tSec(r) >= r.derived.analysis.averages_s;

	function spectrumViews(r: Run, sel: { row: ChannelRow; k: number }[]): View[] {
		const a = r.derived.analysis;
		const x = Array.from({ length: a.lines + 1 }, (_, i) => round(i * a.df_hz, 6));
		const floor = (NOISE * NOISE) / (a.fs_hz / 2);
		return sel.map(({ row, k }) => {
			const tone = TONES[k % 4];
			const y = x.map((f, i) => {
				if (!tone) return 1e-5 * Math.exp(-f / 400) + floor;
				const d = Math.abs(f - tone.hz) / a.df_hz;
				return d < 1 ? ((tone.amp * tone.amp) / 2 / a.df_hz) * (1 - d * 0.6) : floor * (1 + 0.3 * Math.sin(i * 1.3 + row.index));
			});
			return { view: 'spectrum', title: titleOf(row), x: { label: 'Frequency', unit: 'Hz', scale: 'linear' }, y: { label: 'PSD', unit: `${row.units}²/Hz`, scale: 'log' }, series: [{ name: `Channel ${row.index}`, role: 'measured', x: f64(x), y: f64(y) }], link: 'frequency' } as SpectrumView;
		});
	}

	function frfViews(r: Run): View[] {
		const pair = r.norm.frf;
		if (!pair) return [];
		const ref = r.rows.find((x) => x.index === pair.reference);
		const resp = r.rows.find((x) => x.index === pair.response);
		if (!ref || !resp) return [];
		const a = r.derived.analysis;
		const x = Array.from({ length: a.lines + 1 }, (_, i) => round(i * a.df_hz, 6));
		const axis = { label: 'Frequency', unit: 'Hz', scale: 'linear' as const };
		const mag = x.map((f) => 0.5 * (1 + 0.2 * Math.sin(f / 300)));
		const phase = x.map((f) => round(-360 * f * 78.125e-6, 4));
		const coh = x.map((f) => round(0.97 + 0.02 * Math.cos(f / 500), 4));
		return [
			{ view: 'spectrum', title: 'FRF', x: axis, y: { label: 'Magnitude', unit: `${resp.units}/${ref.units}`, scale: 'log' }, series: [{ name: 'H1', role: 'measured', x: f64(x), y: f64(mag) }], link: 'frequency' },
			{ view: 'spectrum', title: 'Phase', x: axis, y: { label: 'Phase', unit: 'deg', scale: 'linear' }, series: [{ name: 'H1', role: 'measured', x: f64(x), y: f64(phase) }], link: 'frequency' },
			{ view: 'spectrum', title: 'Coherence', x: axis, y: { label: 'Coherence', unit: null, scale: 'linear' }, series: [{ name: 'Coherence', role: 'measured', x: f64(x), y: f64(coh) }], link: 'frequency' }
		] as View[];
	}

	function liveOf(r: Run) {
		return { ...head(r), views: [readings(r)] };
	}

	function dataFor(r: Run, plots: RecorderSubscribePlot[]) {
		const passed = r.frames > 0;
		return {
			...head(r),
			plots: plots.map((pl) => {
				const base = { id: pl.id, quantity: pl.quantity, channels: null as number[] | null, first_frame: null as number | null, views: [] as View[] };
				switch (pl.quantity) {
					case 'time': {
						const sel = resolve(r, pl.channels ?? 'all');
						base.channels = sel.map((x) => x.row.index);
						const tv = passed ? timeView(r, sel, pl.points ?? 1024, pl.seconds ?? 2) : null;
						if (tv) {
							base.views = [tv.view];
							base.first_frame = tv.first;
						}
						break;
					}
					case 'spectrum': {
						const sel = resolve(r, pl.channels ?? 'all');
						base.channels = sel.map((x) => x.row.index);
						if (passed && averaged(r)) base.views = spectrumViews(r, sel);
						break;
					}
					case 'frf':
						if (passed && averaged(r)) base.views = frfViews(r);
						break;
					case 'readings':
						if (passed) base.views = [readings(r)];
						break;
				}
				return base;
			})
		};
	}

	/** As the service sends them: `recorder.live` to every connection without a subscription, `recorder.data` to each subscribed one at its rate. */
	function publish(r: Run) {
		const final = r.state === 'finished';
		let frame: Record<string, unknown> | null = null;
		for (const conn of host.connections()) {
			const sub = subs.get(conn);
			if (!sub) {
				frame ??= liveOf(r) as unknown as Record<string, unknown>;
				conn.emit('recorder.live', frame);
				continue;
			}
			sub.acc += LIVE_MS / 1000;
			if (final || sub.acc + 1e-9 >= 1 / sub.rate) {
				sub.acc = 0;
				conn.emit('recorder.data', dataFor(r, sub.plots) as unknown as Record<string, unknown>);
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
			if (run.state === 'starting') return;
			publish(run);
		}, LIVE_MS);
	}

	// ---- refusals -----------------------------------------------------------------------------------

	function needRun(): Run {
		if (!run) refuse('refused', 'not_configured', 'The Recorder is not configured; configure a test first.');
		return run;
	}

	const PHASE_WORD: Record<RecorderPhase, string> = {
		idle: 'idle',
		opening: 'opening',
		monitoring: 'monitoring',
		armed: 'armed',
		recording: 'recording',
		closing: 'closing',
		stopped: 'stopped',
		aborted: 'aborted',
		faulted: 'faulted'
	};

	function wrongState(r: Run, verb: string, why = ''): never {
		refuse('refused', 'wrong_state', `${verb} does not apply while the run is ${r.state} (${PHASE_WORD[r.phase]}).${why ? ` ${why}` : ''}`, { state: r.state, phase: r.phase });
	}

	function wrongMode(r: Run, verb: string, modes: string): never {
		refuse('refused', 'wrong_mode', `${verb} applies in ${modes} mode; this run's trigger is "${r.norm.mode}".`, { mode: r.norm.mode });
	}

	function notWhileOther() {
		const other = host.otherRun();
		if (other) refuse('refused', 'run_active', `Run ${other} holds the station; stop it first.`, { run_id: other });
	}

	function notWhileLive(what: string) {
		if (run && isLive()) refuse('refused', 'run_active', `Run ${run.id} is ${run.state}; stop it before ${what}.`, { run_id: run.id });
	}

	function noOtherKeys(params: Record<string, unknown>, keys: string[]) {
		const extra = Object.keys(params).find((k) => !keys.includes(k));
		if (extra) bad(extra, `"${extra}" is not a parameter here; it takes ${keys.join(', ') || 'none'}.`);
	}

	function since(params: Record<string, unknown>): number {
		noOtherKeys(params, ['since']);
		if (params.since !== undefined && !(isInt(params.since) && (params.since as number) >= 0)) bad('since', '"since" is a position, a whole number from 0.');
		return (params.since as number | undefined) ?? 0;
	}

	function parseSubscribe(params: Record<string, unknown>): { rate: number; plots: RecorderSubscribePlot[] } | null {
		if (params.default !== undefined) {
			if (params.default !== true) bad('default', '"default" can only be true.');
			if (params.plots !== undefined || params.rate_hz !== undefined) bad('default', '"default" takes no other parameter.');
			return null;
		}
		noOtherKeys(params, ['rate_hz', 'plots']);
		const rate = params.rate_hz === undefined ? 10 : params.rate_hz;
		if (!(isNum(rate) && rate >= 0.5 && rate <= 10)) bad('rate_hz', '"rate_hz" is a number from 0.5 to 10.');
		if (!Array.isArray(params.plots)) bad('plots', '"plots" is a list of plots.');
		const list = params.plots as unknown[];
		if (list.length > 64) bad('plots', 'At most 64 plots.');
		const seen = new Set<string>();
		const plots = list.map((raw, i): RecorderSubscribePlot => {
			const at = `plots[${i}]`;
			if (!isObject(raw)) bad(at, `${at} is an object.`);
			const p = raw as Record<string, unknown>;
			const extra = Object.keys(p).find((k) => !['id', 'quantity', 'channels', 'points', 'seconds'].includes(k));
			if (extra) bad(`${at}.${extra}`, `A plot has an id, a quantity, channels, points and seconds; not "${extra}".`);
			if (typeof p.id !== 'string' || p.id.length < 1 || p.id.length > 64) bad(`${at}.id`, 'A plot id is 1 to 64 characters.');
			if (seen.has(p.id as string)) bad(`${at}.id`, `The plot id "${p.id}" is used twice.`);
			seen.add(p.id as string);
			if (!QUANTITIES.includes(p.quantity as RecorderLiveQuantity)) bad(`${at}.quantity`, `"quantity" is one of ${QUANTITIES.join(', ')}.`);
			const quantity = p.quantity as RecorderLiveQuantity;
			const out: RecorderSubscribePlot = { id: p.id as string, quantity };
			if (quantity === 'time' || quantity === 'spectrum') {
				const ch = p.channels ?? 'all';
				const ok = ch === 'all' || (Array.isArray(ch) && ch.length > 0 && ch.every((x) => Number.isInteger(x) && x >= 0) && new Set(ch).size === ch.length);
				if (!ok) bad(`${at}.channels`, '"channels" is a list of recorded rows\' indexes, or "all".');
				out.channels = ch as RecorderSubscribePlot['channels'];
			} else if (p.channels !== undefined) bad(`${at}.channels`, `"channels" does not apply to ${quantity}.`);
			if (quantity === 'time') {
				const pts = p.points ?? 1024;
				if (!(Number.isInteger(pts) && (pts as number) >= 64 && (pts as number) <= 8192)) bad(`${at}.points`, '"points" is a whole number from 64 to 8192.');
				const secs = p.seconds ?? 2;
				if (!(isNum(secs) && secs > 0 && secs <= 2)) bad(`${at}.seconds`, '"seconds" is above 0 and at most 2.');
				out.points = pts as number;
				out.seconds = secs as number;
			} else {
				if (p.points !== undefined) bad(`${at}.points`, `"points" does not apply to ${quantity}.`);
				if (p.seconds !== undefined) bad(`${at}.seconds`, `"seconds" does not apply to ${quantity}.`);
			}
			return out;
		});
		return { rate: rate as number, plots };
	}

	function recordingEntry(r: Run, x: Rec): RecorderRecording {
		const frames = onDiskOf(r, x);
		return {
			name: x.name,
			folder: `streams/${x.name}`,
			trigger: x.trigger,
			requested_fs_hz: x.requestedFs,
			fs_hz: round(x.fs, 6),
			group_delay_frames: x.groupDelay,
			rows: [...x.rows],
			first_station_frame: x.first,
			trigger_station_frame: x.trig,
			pre_frames: x.pre,
			pre_frames_asked: x.preAsked,
			frames,
			seconds: round(frames / x.fs, 6),
			bytes: frames * 4 * x.rows.length,
			gap_free: x.gaps.length === 0,
			gaps: x.gaps.map((g) => ({ ...g })),
			marks: x.marks,
			complete: x.complete,
			fault: x.fault
		};
	}

	/** The list as PROTOCOL orders it: the saved recordings, then the one in progress. */
	const listed = (r: Run) => [...r.recs.filter((x) => x.complete), ...r.recs.filter((x) => !x.complete)];

	// ---- the methods --------------------------------------------------------------------------------

	return {
		isLive,
		liveId: () => (run && isLive() ? run.id : null),
		detach: (conn) => {
			subs.delete(conn);
		},
		seatExpired: () => {
			if (run && run.state === 'running') beginStop(run, 'safe_stop', 'safe_stop');
		},
		methods: {
			'recorder.configure': (params, conn) => {
				noOtherKeys(params, ['config', 'dry_run']);
				if (!isObject(params.config)) bad('config', 'Configuring needs a test configuration.');
				if (params.dry_run !== undefined && typeof params.dry_run !== 'boolean') bad('dry_run', '"dry_run" is true or false.');
				shapeOk(params.config as Record<string, unknown>);
				const config = params.config as unknown as RecorderConfig;
				const checked = check(config);
				if (params.dry_run === true) return { errors: checked.errors, warnings: checked.warnings, derived: checked.derived };
				host.own(conn);
				notWhileLive('configuring');
				notWhileOther();
				if (checked.errors.length || !checked.derived) refuse('bad_request', 'config_invalid', `The Recorder configuration has ${checked.errors.length} error(s).`, { errors: checked.errors, warnings: checked.warnings });
				if (run?.timer) clearInterval(run.timer);
				run = newRun(config, checked.derived);
				log = [];
				addLog('state', `configured "${run.norm.title}" on ${run.station.class}`);
				stateChanged(true);
				return { state: stateOf(run), derived: checked.derived, warnings: checked.warnings };
			},
			'recorder.config': () => (run ? { run_id: run.id, config: run.config, derived: run.derived } : { run_id: null, config: null, derived: null }),
			'recorder.start': (params, conn) => {
				noOtherKeys(params, []);
				host.own(conn);
				const r = needRun();
				if (r.state === 'finished') refuse('refused', 'run_finished', 'This run has finished; reset or configure again to run.', { outcome: r.end?.outcome });
				if (r.state !== 'configured') wrongState(r, 'Starting');
				notWhileOther();
				const checked = check(r.config);
				if (checked.errors.length) refuse('bad_request', 'config_invalid', `The channel table, as it is now, no longer carries the run: ${checked.errors.length} error(s).`, { errors: checked.errors, warnings: checked.warnings });
				const free = freeNow();
				const floor = r.derived.disk.floor_bytes;
				if (free !== null && free < floor) refuse('refused', 'not_enough_space', `The data volume has ${free} bytes free, under the run's floor of ${floor} bytes; nothing was made.`, { free_bytes: free, floor_bytes: floor });
				r.state = 'starting';
				r.phase = 'opening';
				stateChanged();
				// The simulated station opens at once: the simulator programs the rate asked, the NI ladder its nearest rate.
				r.rows = inputs().map((x) => ({ ...x, abort: { ...x.abort } }));
				r.fs = isNi() ? ladderRate(r.requestedFs) : r.requestedFs;
				Object.assign(r, figuresAt(r.fs));
				r.chunk = chunkFramesAt(r.fs);
				r.derived = derivedOf(r.norm, r.fs, true, r.rows.map((x) => x.index));
				let name = r.norm.run_name;
				for (let k = 2; folders.has(name); k++) name = `${r.norm.run_name}-${k}`;
				folders.add(name);
				r.folder = `~/Hertzforge_Data/${name}`;
				r.peak = r.rows.map(() => 0);
				r.overload = r.rows.map(() => false);
				r.rowMarks = r.rows.map(() => 0);
				r.lastOver = r.rows.map(() => null);
				r.ms = r.rows.map(() => []);
				addLog('station', `the station programmed ${round(r.fs, 3)} Hz (asked ${r.requestedFs} Hz)`);
				addLog('note', `records in ${r.folder}`);
				const d = drives();
				addLog('note', d.length ? `outputs: none; ${d.length === 1 ? 'drive row' : 'drive rows'} ${d.map((x) => x.index).join(', ')} not opened` : 'outputs: none');
				r.state = 'running';
				r.phase = 'monitoring';
				r.frames = 0;
				if (r.norm.mode === 'start') begin(r, 'start', 0, 0);
				// The start answers once acquisition runs: after the first batch.
				batch(r);
				r.timer = setInterval(() => tick(r), TICK_MS);
				rephase(r);
				addLog('state', `running, ${r.phase}`);
				stateChanged();
				startLive();
				return stateOf(r);
			},
			'recorder.arm': (params, conn) => {
				noOtherKeys(params, []);
				host.own(conn);
				const r = needRun();
				if (r.norm.mode === 'start') wrongMode(r, 'Arming', 'manual or level');
				if (r.state !== 'running' || r.phase !== 'monitoring') wrongState(r, 'Arming', r.phase === 'armed' ? 'The run is already armed.' : '');
				r.armed = true;
				r.held = 0;
				r.saved = 0;
				r.begun = 0;
				r.detectorReady = false;
				r.needFullPre = false;
				rephase(r);
				return { pre_frames: r.derived.trigger.pre_frames, held_frames: 0 };
			},
			'recorder.disarm': (params, conn) => {
				noOtherKeys(params, []);
				host.own(conn);
				const r = needRun();
				if (r.norm.mode === 'start') wrongMode(r, 'Disarming', 'manual or level');
				if (r.state !== 'running' || r.phase !== 'armed') wrongState(r, 'Disarming');
				r.armed = false;
				r.held = 0;
				rephase(r);
				return stateOf(r);
			},
			'recorder.record': (params, conn) => {
				noOtherKeys(params, []);
				host.own(conn);
				const r = needRun();
				if (r.norm.mode !== 'manual') wrongMode(r, 'Record', 'manual');
				if (r.state !== 'running' || (r.phase !== 'monitoring' && r.phase !== 'armed')) wrongState(r, 'Record', r.phase === 'recording' ? 'A recording is being saved; cut it first.' : '');
				const pre = r.armed ? Math.min(r.held, r.derived.trigger.pre_frames) : 0;
				const x = begin(r, 'manual', r.frames, pre);
				rephase(r);
				return { name: x.name, station_frame: x.trig, pre_frames: x.pre };
			},
			'recorder.cut': (params, conn) => {
				noOtherKeys(params, []);
				host.own(conn);
				const r = needRun();
				if (r.state !== 'running' || !r.rec) wrongState(r, 'Cut', 'No recording is being saved.');
				const x = r.rec;
				feed(r, r.frames);
				x.target = x.fed;
				save(r);
				rephase(r);
				return { name: x.name, last_station_frame: x.first + x.fed - 1 };
			},
			'recorder.stop': (params, conn) => {
				noOtherKeys(params, []);
				if (run && isLive() && run.state !== 'stopping') beginStop(run, 'operator', 'operator_stop', host.clientName(conn));
				return stateOf(run);
			},
			'recorder.abort': (params, conn) => {
				noOtherKeys(params, []);
				if (run && isLive()) {
					if (run.state === 'stopping') run.stopOutcome = 'operator_abort';
					else beginStop(run, 'operator', 'operator_abort', host.clientName(conn));
				}
				return stateOf(run);
			},
			'recorder.recordings': (params) => {
				const from = since(params);
				const list = run ? listed(run).map((x) => recordingEntry(run!, x)) : [];
				return { recordings: list.slice(from), next: list.length };
			},
			'recorder.marks': (params) => {
				const from = since(params);
				const list = run?.marks ?? [];
				return { marks: list.slice(from).map((m) => ({ ...m })), next: list.length };
			},
			'recorder.reset': (params, conn) => {
				noOtherKeys(params, []);
				host.own(conn);
				const r = needRun();
				notWhileLive('resetting');
				notWhileOther();
				const checked = check(r.config);
				if (checked.errors.length || !checked.derived) refuse('bad_request', 'config_invalid', `The configuration no longer checks on this table and station: ${checked.errors.length} error(s).`, { errors: checked.errors, warnings: checked.warnings });
				run = newRun(r.config, checked.derived);
				log = [];
				addLog('state', `reset; configured "${run.norm.title}" again`);
				stateChanged(true);
				return stateOf(run);
			},
			'recorder.clear': (params, conn) => {
				noOtherKeys(params, []);
				host.own(conn);
				notWhileLive('clearing');
				run = null;
				log = [];
				stateChanged(true);
				return stateOf(null);
			},
			'recorder.state': () => stateOf(run),
			'recorder.log': (params) => {
				const from = since(params);
				return { entries: log.slice(from), next: log.length };
			},
			'recorder.subscribe': (params, conn) => {
				const sub = parseSubscribe(params);
				if (!sub) {
					subs.delete(conn);
					return { default: true, rate_hz: 10, plots: null, quantities: QUANTITIES };
				}
				const entry = { ...sub, acc: 0 };
				subs.set(conn, entry);
				if (run && started(run)) {
					const r = run;
					setTimeout(() => {
						if (subs.get(conn) === entry && run === r) conn.emit('recorder.data', dataFor(r, sub.plots) as unknown as Record<string, unknown>);
					}, 30);
				}
				return { default: false, rate_hz: sub.rate, plots: sub.plots, quantities: QUANTITIES };
			}
		}
	};
}
