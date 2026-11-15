/**
 * The Recorder on the wire (schema/PROTOCOL.md, "Recorder"): it records the inputs and drives
 * nothing. A run opens the station with no outputs, shows every enabled input row live, and saves
 * recordings from the run's first frame, when the operator asks, or each time a row crosses a
 * level. Every frame number is a station frame, counted from the run's first frame at the rate the
 * station programmed. Where a part of the wire is Random's, the type is theirs.
 */
import type {
	ChannelRole,
	ClockDomain,
	DataSource,
	ItemReason,
	LogEntry,
	LogKind,
	ReadingView,
	RefusalReason,
	ReportItem,
	StationClass,
	StationRef,
	View
} from './types';

/** Event topics of the Recorder. */
export const RECORDER_TOPICS = {
	state: 'recorder.state',
	log: 'recorder.log',
	live: 'recorder.live',
	data: 'recorder.data'
} as const;

/** Refusals of the `recorder.*` methods: the shared ones and the Recorder's own. */
export type RecorderRefusalReason = RefusalReason | 'wrong_mode' | 'not_enough_space';

/** `not_enough_space`'s detail. */
export interface RecorderSpaceDetail {
	free_bytes: number;
	floor_bytes: number;
}

// --- configuration -------------------------------------------------------------------------

export interface RecorderAcquisition {
	/** The rate asked of the station, 1000 to 204800 Hz; default 12800. */
	fs_hz?: number;
}

/** What starts a recording. */
export type RecorderTriggerMode = 'start' | 'manual' | 'level';

export type RecorderSlope = 'rising' | 'falling';

/** Every field optional but, in `level` mode, `row`, `level` and `record_s`. */
export interface RecorderTrigger {
	/** Default `manual`. */
	mode?: RecorderTriggerMode;
	/** The table `index` of a recorded row (`level` mode). */
	row?: number;
	/** In the row's unit, within its `range_max` either side of zero (`level` mode). */
	level?: number;
	/** Default `rising`. */
	slope?: RecorderSlope;
	/** In the row's unit, above 0; null is 1% of the row's `range_max`. */
	hysteresis?: number | null;
	/** The pre-window, seconds, 0 to 10; default 1.0. */
	pre_s?: number;
	/** Each recording's length after its trigger, seconds, above 0; null until cut. */
	record_s?: number | null;
	/** `level` mode: recordings saved after an arm, at least 1; null until the run ends. Default 1. */
	count?: number | null;
	/** `level` mode: the least station time from one trigger to the next, seconds; default 0. */
	holdoff_s?: number;
}

export type RecorderWindow = 'hann' | 'flattop' | 'rectangular' | 'blackman_harris';

export type RecorderAveraging = 'linear' | 'exponential' | 'peak_hold';

/** The live FRF's pair, two different recorded rows by table `index`. */
export interface RecorderFrfPair {
	reference: number;
	response: number;
}

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface RecorderAnalysis {
	/** A power of two from 256 to 65536; default 2048. */
	block?: number;
	/** Default `hann`. */
	window?: RecorderWindow;
	/** 0 to 95; null is the window's own (Hann 50, flat top 75, rectangular 0, Blackman-Harris 66.7). */
	overlap_pct?: number | null;
	/** Default `linear`. */
	averaging?: RecorderAveraging;
	/** At least 1; default 8. */
	averages?: number;
	/** A power of two from 1 to 256; default 1. Recordings are always at the station's rate. */
	decimation?: number;
	/** Default null: no live FRF. */
	frf?: RecorderFrfPair | null;
}

export interface RecorderCapture {
	/** The run folder's name under the local data root; default `recorder`. */
	run_name?: string;
	/** The free space the run leaves on the data volume, GiB, at least 1; default 1. */
	keep_free_gb?: number;
}

export interface RecorderConfig {
	title?: string;
	acquisition?: RecorderAcquisition;
	trigger?: RecorderTrigger;
	analysis?: RecorderAnalysis;
	capture?: RecorderCapture;
	/** Random's fields; `label` is the key when absent. */
	metadata?: { key: string; label?: string; value: string }[];
	/** The operator's words for the run's record, as Random's. */
	operator?: string;
	article?: string;
	notes?: string;
	specification?: { document?: string; clause?: string } | null;
}

export interface RecorderConfigureParams {
	config: RecorderConfig;
	dry_run?: boolean;
}

export type RecorderItemReason =
	| ItemReason
	| 'not_power_of_two'
	| 'row_not_recorded'
	| 'frf_same_row'
	| 'no_input_channel';

export type RecorderReportItem = Omit<ReportItem, 'reason'> & { reason: RecorderItemReason };

/** The station's working figures at the requested rate; the station's own once started. */
export interface RecorderDerivedGrid {
	fs_hz: number;
	/** The power of two nearest 20 ms of the rate. */
	block_frames: number;
	/** Two blocks: the most the service reads between two verbs. */
	batch_frames: number;
	/** The station's stated input group delay, frames, to 0.01; 0 on the simulator. */
	group_delay_frames: number;
	/** True once the run has started and the figures are the station's. */
	actual: boolean;
}

export interface RecorderDerivedAnalysis {
	/** The station's rate over the decimation. */
	fs_hz: number;
	df_hz: number;
	/** The block over 2.56. */
	lines: number;
	/** The analysis rate over 2.56. */
	span_hz: number;
	block_s: number;
	overlap_pct: number;
	/** How long after start the first full average is complete. */
	averages_s: number;
}

/** Frames at the rate, each `round(seconds x fs_hz)`. */
export interface RecorderDerivedTrigger {
	pre_frames: number;
	/** Null until cut. */
	record_frames: number | null;
	holdoff_frames: number;
}

export interface RecorderDerivedDisk {
	/** 4 bytes a row a frame. */
	bytes_per_s: number;
	floor_bytes: number;
	/** Null when the free space cannot be read. */
	free_bytes: number | null;
	/** Whole seconds of recording that fit above the floor now. */
	seconds_above_floor: number | null;
}

export interface RecorderDerived {
	grid: RecorderDerivedGrid;
	/** A Recorder run opens no output. */
	outputs: 'none';
	/** The recorded rows' table indexes, in column order. */
	rows: number[];
	analysis: RecorderDerivedAnalysis;
	trigger: RecorderDerivedTrigger;
	disk: RecorderDerivedDisk;
}

export interface RecorderConfigureResult {
	state: RecorderState;
	derived: RecorderDerived;
	warnings: RecorderReportItem[];
}

export interface RecorderDryRunResult {
	errors: RecorderReportItem[];
	warnings: RecorderReportItem[];
	/** Null with errors. */
	derived: RecorderDerived | null;
}

/** `recorder.config`: what the present run was configured with; all null when unconfigured. */
export interface RecorderConfigResult {
	run_id: string | null;
	config: RecorderConfig | null;
	derived: RecorderDerived | null;
}

// --- verbs ---------------------------------------------------------------------------------

/** `recorder.arm`: the pre-window asked and the frames it holds now. */
export interface RecorderArmResult {
	pre_frames: number;
	held_frames: number;
}

/** `recorder.record`: the new recording and the station frame of its trigger. */
export interface RecorderRecordResult {
	name: string;
	station_frame: number;
	/** The pre-window it begins with. */
	pre_frames: number;
}

/** `recorder.cut`: the recording ended and its last station frame. */
export interface RecorderCutResult {
	name: string;
	last_station_frame: number;
}

// --- run state -----------------------------------------------------------------------------

export type RecorderRunState = 'unconfigured' | 'configured' | 'starting' | 'running' | 'stopping' | 'finished';

export type RecorderPhase =
	| 'idle'
	| 'opening'
	| 'monitoring'
	| 'armed'
	| 'recording'
	| 'closing'
	| 'stopped'
	| 'aborted'
	| 'faulted';

export type RecorderOutcome =
	| 'operator_stop'
	| 'safe_stop'
	| 'operator_abort'
	| 'write_failed'
	| 'write_behind'
	| 'fault'
	| 'arm_refused';

/** Why a stop began. */
export type RecorderStopCause = 'operator' | 'safe_stop' | 'shutdown';

export interface RecorderEnd {
	outcome: RecorderOutcome;
	message: string;
	/** Station time at which acquisition ended; null when it never began. */
	at_s: number | null;
	/** Recordings saved. */
	recordings: number;
}

export interface RecorderStateGrid extends RecorderDerivedGrid {
	requested_fs_hz: number;
}

/** The trigger's state; null in `start` mode. */
export interface RecorderTriggerState {
	armed: boolean;
	pre_frames: number;
	held_frames: number;
	/** Triggers since start. */
	fires: number;
	/** The station frame of the last trigger; null before one. */
	last_station_frame: number | null;
	/** Recordings saved since the last arm. */
	saved: number;
	/** Recordings of `count` not yet begun; null when `count` is null or in `manual` mode. */
	left: number | null;
	/** Seconds of station time; 0 when none. */
	holdoff_left_s: number;
}

/** The recording in progress. */
export interface RecorderRecordingNow {
	name: string;
	trigger: RecorderTriggerMode;
	first_station_frame: number;
	trigger_station_frame: number;
	pre_frames: number;
	pre_frames_asked: number;
	/** Frames handed to the record thread. */
	frames_fed: number;
	frames_on_disk: number;
	/** The pre-window plus `record_frames`; null until cut. */
	target_frames: number | null;
	/** Seconds recorded after the trigger. */
	seconds: number;
	/** `record_s`; null until cut. */
	length_s: number | null;
	/** Frames fed and not yet on disk, in seconds. */
	backlog_s: number;
}

export interface RecorderDisk {
	/** Read once a second; null when the free space cannot be read. */
	free_bytes: number | null;
	floor_bytes: number;
	seconds_above_floor: number | null;
	/** The record thread's fault in its own words. */
	fault: string | null;
}

export interface RecorderPassMs {
	last: number | null;
	worst: number | null;
}

export interface RecorderHealth {
	batches: number;
	blocks_dropped: number;
	driver_warnings: number;
	station_errors: number;
	pass_ms: RecorderPassMs;
	/** The longest gap between two batches; null until two have run. */
	batch_gap_ms: number | null;
}

/** The `recorder.state` result and event. */
export interface RecorderState {
	run_id: string | null;
	state: RecorderRunState;
	phase: RecorderPhase;
	title: string | null;
	source: DataSource | null;
	station: StationRef | null;
	grid: RecorderStateGrid | null;
	clock: ClockDomain | null;
	outputs: 'none' | null;
	rows: number[] | null;
	mode: RecorderTriggerMode | null;
	/** Station time since start. */
	t_s: number | null;
	station_frames: number | null;
	trigger: RecorderTriggerState | null;
	recording: RecorderRecordingNow | null;
	/** Recordings saved. */
	recordings: number;
	/** Limit marks so far. */
	marks: number;
	disk: RecorderDisk | null;
	health: RecorderHealth | null;
	stop: RecorderStopCause | null;
	end: RecorderEnd | null;
	/** The run's record from start on: its folder, and the latest problem writing it. */
	record: RecorderRecordRef | null;
}

/** Where a Recorder run's record is (PROTOCOL.md, "Recorder Run State"). */
export interface RecorderRecordRef {
	folder: string;
	problem: string | null;
}

/** The Recorder's log kinds: Random's and its own. */
/** `record`: a problem writing the run folder's own files. */
export type RecorderLogKind = LogKind | 'recording' | 'mark' | 'record';

export type RecorderLogEntry = Omit<LogEntry, 'kind'> & { kind: RecorderLogKind };

export interface RecorderLogResult {
	entries: RecorderLogEntry[];
	next: number;
}

export interface RecorderLogEvent {
	run_id: string;
	entries: RecorderLogEntry[];
}

// --- recordings and marks ------------------------------------------------------------------

/** Frames missing from a recording: from file frame `at_frame`, `dropped_frames` of them. */
export interface RecorderGap {
	at_frame: number;
	dropped_frames: number;
}

/** One recording, as `recorder.recordings` lists it. */
export interface RecorderRecording {
	name: string;
	/** Relative to the run folder. */
	folder: string;
	trigger: RecorderTriggerMode;
	requested_fs_hz: number;
	/** The rate the file is at. */
	fs_hz: number;
	group_delay_frames: number;
	rows: number[];
	first_station_frame: number;
	trigger_station_frame: number;
	pre_frames: number;
	pre_frames_asked: number;
	frames: number;
	seconds: number;
	bytes: number;
	gap_free: boolean;
	gaps: RecorderGap[];
	/** Limit marks inside it. */
	marks: number;
	/** False while it is being written. */
	complete: boolean;
	fault: string | null;
}

export interface RecorderListParams {
	since?: number;
}

export interface RecorderRecordingsResult {
	recordings: RecorderRecording[];
	next: number;
}

export type RecorderLimitAxis = 'accel' | 'velocity' | 'displacement';

export type RecorderMark =
	| {
			kind: 'limit';
			row: number;
			axis: RecorderLimitAxis;
			/** In the axis's unit: g, in/s, mil. */
			value: number;
			limit: number;
			station_frame: number;
			recording: string | null;
	  }
	| {
			kind: 'overload';
			row: number;
			value: number;
			/** 0.9 of the row's `range_max`. */
			level: number;
			station_frame: number;
			recording: string | null;
	  };

export interface RecorderMarksResult {
	marks: RecorderMark[];
	next: number;
}

/** A recording's `stream.json` (PROTOCOL.md, "Recorder Records"). */
export interface RecorderStream {
	kind: 'recording';
	channels: { index: number; unit: string; description?: string }[];
	complete: boolean;
	name: string;
	trigger: RecorderTriggerMode;
	requested_fs_hz: number;
	fs_hz: number;
	group_delay_frames: number;
	rows: number[];
	units: string[];
	first_station_frame: number;
	trigger_station_frame: number;
	pre_frames: number;
	pre_frames_asked: number;
	frames: number;
	gaps: RecorderGap[];
	marks: RecorderMark[];
	fault: string | null;
}

// --- display -------------------------------------------------------------------------------

export type RecorderLiveQuantity = 'time' | 'spectrum' | 'frf' | 'readings';

/** Which rows a `time` or `spectrum` plot shows: recorded rows' indexes, or all of them. */
export type RecorderRowSelector = number[] | 'all';

export interface RecorderSubscribePlot {
	id: string;
	quantity: RecorderLiveQuantity;
	/** `time` and `spectrum` only; default `all`. */
	channels?: RecorderRowSelector;
	/** `time` only: 64 to 8192, default 1024. */
	points?: number;
	/** `time` only: above 0 and at most 2, default 2. */
	seconds?: number;
}

export type RecorderSubscribeParams = { rate_hz?: number; plots: RecorderSubscribePlot[] } | { default: true };

export interface RecorderSubscribeResult {
	default: boolean;
	rate_hz: number;
	plots: RecorderSubscribePlot[] | null;
	quantities: RecorderLiveQuantity[];
}

/** One recorded row on a live frame. */
export interface RecorderLiveRow {
	index: number;
	description: string;
	role: Exclude<ChannelRole, 'drive'>;
	unit: string;
	/** Over the last second of station time, DC included. */
	rms: number | null;
	/** The largest magnitude since start. */
	peak: number | null;
	/** The overload indicator has latched (0.9 of the row's `range_max`). */
	overload: boolean;
	/** The row's limit marks so far. */
	marks: number;
}

/** The numbers every live frame carries, `recorder.live` and `recorder.data` alike. */
export interface RecorderLiveHead {
	run_id: string;
	source: DataSource;
	class: StationClass;
	state: 'running' | 'stopping' | 'finished';
	phase: RecorderPhase;
	t_s: number | null;
	station_frames: number | null;
	grid: { requested_fs_hz: number; fs_hz: number; group_delay_frames: number };
	rows: RecorderLiveRow[];
	trigger: RecorderTriggerState | null;
	recording: RecorderRecordingNow | null;
	recordings: number;
	marks: number;
	disk: RecorderDisk | null;
	health: RecorderHealth | null;
}

/** The `recorder.live` event: the run's readings plus the numbers behind them. */
export interface RecorderLive extends RecorderLiveHead {
	/** Run readings. */
	views: [ReadingView];
}

/** One plot's data in a `recorder.data` frame. */
export interface RecorderDataPlot {
	id: string;
	quantity: RecorderLiveQuantity;
	/** The resolved rows of a `time` or `spectrum` plot, in table order; null for the rest. */
	channels: number[] | null;
	/** The station frame of a `time` plot's first sample; null for the rest. */
	first_frame: number | null;
	views: View[];
}

/** The `recorder.data` event: a subscribed connection's own frame. */
export interface RecorderData extends RecorderLiveHead {
	plots: RecorderDataPlot[];
}
