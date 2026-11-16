/**
 * Playback on the wire (schema/PROTOCOL.md, "Playback"): the control service plays a native drive
 * file, volts at each drive output, open loop, scaled by the level, and records every enabled input
 * and every drive as played. It reads one format, the drive folder (`docs/drive-file.md`); every
 * other format is read and converted on the analysis side (`drive.inspect`, `drive.convert`,
 * `drive.fix_ends`), whose messages are here too. Where a part of the wire is Random's, the type is
 * theirs.
 */
import type { DataSource, ReportItem, RunRecordRef, StationClass } from './types';

/** Event topics of Playback. */
export const PLAYBACK_TOPICS = {
	state: 'playback.state',
	log: 'playback.log',
	live: 'playback.live'
} as const;

// --- configuration -------------------------------------------------------------------------

export interface PlaybackConfig {
	title?: string;
	/** The drive folder by its full path on the computer that runs the test, and the SHA-256 of its `drive.bin`. */
	drive: { folder: string; sha256: string };
	/** The drive row (table `index`) each file channel plays on, in file order. */
	map: number[];
	/** -40 to 0 dB (default -12). */
	level_db?: number;
	/** Volts, above 0, at most 10 and each mapped row's range. */
	ceiling_v: number;
	pre_roll_s?: number;
	post_roll_s?: number;
	capture?: { run_name?: string };
	metadata?: { key: string; label?: string; value: string }[];
	operator?: string;
	article?: string;
	notes?: string;
	specification?: { document?: string; clause?: string } | null;
	record?: { streams?: boolean } | null;
}

export interface PlaybackConfigureParams {
	config: PlaybackConfig;
	dry_run?: boolean;
}

export interface PlaybackDerived {
	fs_hz: number;
	block_frames: number;
	samples: number;
	channels: number;
	duration_s: number;
	pre_roll_frames: number;
	post_roll_frames: number;
	total_s: number;
	gain: number;
	stop: { depth_db: number; ramp_s: number };
	read_ahead_s: number;
}

// --- the drive file's check ----------------------------------------------------------------

export type PlaybackFaultReason = 'start_not_zero' | 'end_not_zero' | 'velocity_not_zero' | 'displacement_not_zero';

export interface PlaybackFault {
	reason: PlaybackFaultReason;
	value: number;
	allowed: number;
	text: string;
}

export interface PlaybackCheckChannel {
	channel: number;
	name: string;
	peak_v: number;
	start_v: number;
	end_v: number;
	/** The drive taken as an acceleration and integrated once: V s. */
	velocity_v_s: { end: number; peak: number };
	/** Integrated twice: V s². */
	displacement_v_s2: { end: number; peak: number };
	pass: boolean;
	faults: PlaybackFault[];
	note: string | null;
	drive_row: number | null;
	played_peak_v: number;
}

export interface PlaybackRefusal {
	reason: string;
	message: string;
	detail: Record<string, unknown> | null;
}

/** Each bucket's start time and its minimum and maximum, at most 200 buckets. */
export interface PlaybackEnvelope {
	t_s: number[];
	min: number[];
	max: number[];
}

export interface PlaybackPreview {
	channel: number;
	drive_v: PlaybackEnvelope;
	velocity_v_s: PlaybackEnvelope;
	displacement_v_s2: PlaybackEnvelope;
}

export interface PlaybackCheckReport {
	pass: boolean;
	sha256: string;
	rate_hz: number;
	samples: number;
	duration_s: number;
	level_db: number;
	ceiling_v: number;
	tolerances: { ends_fraction_of_peak: number; velocity_fraction_of_peak: number; displacement_fraction_of_peak: number; integrator: string };
	channels: PlaybackCheckChannel[];
	refusals: PlaybackRefusal[];
	limits: string;
	previews?: PlaybackPreview[];
}

/** A drive file that could not be read: no numbers, only the refusal. */
export interface PlaybackCheckUnread {
	pass: false;
	refusals: PlaybackRefusal[];
}

export type PlaybackCheck = PlaybackCheckReport | PlaybackCheckUnread;

export function isFullCheck(c: PlaybackCheck | null | undefined): c is PlaybackCheckReport {
	return !!c && 'channels' in c;
}

// --- results -------------------------------------------------------------------------------

export type PlaybackRunState = 'unconfigured' | 'configured' | 'starting' | 'running' | 'stopping' | 'finished';
export type PlaybackPhase = 'idle' | 'reading_ahead' | 'pre_roll' | 'playing' | 'post_roll' | 'done' | 'stopping' | 'stopped' | 'zeroed' | 'refused';
export type PlaybackOutcome = 'completed' | 'stopped' | 'aborted' | 'response_abort' | 'drive_ceiling' | 'input_overload' | 'drive_underrun' | 'reader_failed' | 'loop_faulted' | 'loop_panicked' | 'arm_refused';

export interface PlaybackTrip {
	channel: number;
	axis: string;
	value: number;
	limit: number;
}

export interface PlaybackEnd {
	outcome: PlaybackOutcome;
	message: string;
	at_s: number | null;
	played: number;
	emergency_stop: boolean;
	trip: PlaybackTrip | null;
}

export interface PlaybackProgress {
	played: number;
	samples: number;
	played_s: number;
	fraction: number;
	elapsed_s: number;
	buffered_s: number;
}

export interface PlaybackRow {
	index: number;
	role: 'input' | 'drive';
	unit: string;
	description: string;
}

/** A configured or later run. */
export interface PlaybackRun {
	run_id: string;
	state: Exclude<PlaybackRunState, 'unconfigured'>;
	phase: PlaybackPhase;
	title: string;
	source: DataSource;
	class: StationClass;
	fs_hz: number;
	drive: { samples: number; duration_s: number; sha256: string; map: number[] };
	level_db: number;
	ceiling_v: number;
	rolls: { pre_frames: number; post_frames: number };
	progress: PlaybackProgress;
	inputs: PlaybackRow[];
	drives: PlaybackRow[];
	overloads: { channel: number; overload: boolean }[];
	record: RunRecordRef | null;
	end: PlaybackEnd | null;
}

export type PlaybackState = { run_id: null; state: 'unconfigured' } | PlaybackRun;

export function isRun(s: PlaybackState | null | undefined): s is PlaybackRun {
	return !!s && s.state !== 'unconfigured';
}

export interface PlaybackConfigureResult {
	state: PlaybackState;
	check: PlaybackCheck;
	derived: PlaybackDerived;
	warnings: ReportItem[];
}

export interface PlaybackDryRunResult {
	errors: ReportItem[];
	warnings: ReportItem[];
	check: PlaybackCheck;
	derived: PlaybackDerived | null;
}

export interface PlaybackConfigResult {
	run_id: string | null;
	config: PlaybackConfig | null;
	check: PlaybackCheck | null;
	derived: PlaybackDerived | null;
}

/** `config_invalid`'s detail: the items, and the check report when the file was read. */
export interface PlaybackInvalidDetail {
	errors: ReportItem[];
	warnings: ReportItem[];
	check?: PlaybackCheck;
}

export type PlaybackLogKind = 'state' | 'check' | 'note' | 'record' | 'operator' | 'overload' | 'trip' | 'underrun' | 'abort' | 'outcome';

export interface PlaybackLogEntry {
	seq: number;
	at_unix: number;
	kind: PlaybackLogKind;
	text: string;
}

export interface PlaybackLogResult {
	entries: PlaybackLogEntry[];
	next: number;
}

export interface PlaybackLogEvent {
	run_id: string;
	entries: PlaybackLogEntry[];
}

/** `playback.live`: ten a second from start until the run has finished, then once more. */
export interface PlaybackLive {
	run_id: string;
	state: 'starting' | 'running' | 'stopping' | 'finished';
	phase: PlaybackPhase;
	progress: PlaybackProgress;
	/** The last display points of every input then every drive, the first at `t0_s` station seconds. */
	time: { rate_hz: number; t0_s: number; channels: { index: number; role: 'input' | 'drive'; unit: string; values: Float32Array }[] };
}

// --- the drive file, on the analysis side ---------------------------------------------------

/** One signal a file holds, as `drive.inspect` lists it. */
export interface DriveSignal {
	key: string;
	name: string;
	samples: number;
	rate_hz: number | null;
	unit: string | null;
	where: string;
	/** Why it cannot be a drive (a unit that is not a voltage, say), or null. */
	problem: string | null;
}

export interface DriveInspectResult {
	source: { path?: string; objects?: string[]; format: string };
	signals: DriveSignal[];
	rate_hz: number | null;
	notes: string[];
}

/** A drive folder's sidecar, `drive.json`. */
export interface DriveSidecar {
	format: 'hertzforge-drive';
	version: number;
	rate_hz: number;
	samples: number;
	channels: { name: string; unit: 'V'; source: unknown }[];
	sha256: string;
	source: unknown;
	fixes: unknown[];
	tool: string;
	created_utc: string;
}

/** A channel's ends: volts, volt seconds and volt seconds squared. */
export interface DriveEnds {
	start: number;
	end: number;
	peak: number;
	velocity_end: number;
	velocity_peak: number;
	displacement_end: number;
	displacement_peak: number;
}

export interface DriveFixChannel {
	name: string;
	a: number;
	b: number;
	rms_before: number;
	rms_after: number;
	before: DriveEnds;
	after: DriveEnds;
}

export interface DriveFixEndsResult {
	folder: string;
	drive: DriveSidecar;
	from: { folder: string; sha256: string };
	taper_s: number;
	taper_samples: number;
	channels: DriveFixChannel[];
}

export interface DriveConvertResult {
	folder: string;
	drive: DriveSidecar;
	notes: string[];
	fixed?: DriveFixEndsResult;
}
