/**
 * Modal Test on the wire (schema/PROTOCOL.md, "Modal Test"): hammer impact testing and open-loop
 * shaker testing with one or more shakers, FRFs labeled by node and direction, every frame
 * recorded, and the finished test handed to the modal fit on the analysis side.
 *
 * The control service computes every number shown: the checks on every hit, the counts, the
 * windows in samples, the exponential window's time constant and the damping it adds, the FRFs,
 * the coherence. The shell sends the operator's choices and draws what comes back.
 *
 * The live frame names every trace's row, role and unit, carries the latest hit's own block with
 * the windows it was taken with and its force spectrum (impact), and the reference autospectra
 * as the average builds (shaker).
 */
import type { DataSource, ReportItem, StationClass } from './types';

/** Event topics of Modal Test. */
export const MODAL_TEST_TOPICS = {
	state: 'modal_test.state',
	log: 'modal_test.log',
	live: 'modal_test.live'
} as const;

// --- configuration -------------------------------------------------------------------------

export type ModalMode = 'impact' | 'shaker';
/** The estimator, as the service spells it. Several references always use the matrix H1. */
export type ModalEstimator = 'H1' | 'H2' | 'Hv';
export const MODAL_ESTIMATORS: readonly ModalEstimator[] = ['H1', 'H2', 'Hv'];
export type ModalAcceptMode = 'auto' | 'manual';
export type ModalSource = 'random' | 'burst_random' | 'pseudo_random' | 'periodic_random' | 'periodic_chirp' | 'burst_chirp';
export const MODAL_SOURCES: readonly ModalSource[] = ['random', 'burst_random', 'pseudo_random', 'periodic_random', 'periodic_chirp', 'burst_chirp'];

/** A roving point: the node every roving channel takes, and a direction when one channel roves. */
export interface ModalPoint {
	node: number;
	direction?: string;
}

/** The points as a list, or filled by a start, a step and a count. */
export type ModalPoints = ModalPoint[] | { start?: number; step?: number; count: number };

/** The hammer's sign: the trigger watches the force rising through the level in it. */
export type ModalPolarity = 'positive' | 'negative';

export interface ModalImpactConfig {
	/** The trigger level, percent of the hammer row's range. */
	trigger_level_pct?: number;
	polarity?: ModalPolarity;
	/** Percent of the block before the trigger. */
	pretrigger_pct?: number;
	holdoff_blocks?: number;
	/** A later force over this percent of the main peak is a double hit. */
	double_hit_pct?: number;
	/** The tip check warns above this drop of the force spectrum across the band, dB. */
	tip_warn_db?: number;
	accept?: ModalAcceptMode;
	force_window?: { on?: boolean; width_pct?: number; fall_pct?: number };
	exponential_window?: { on?: boolean; end_pct?: number };
	auto_advance?: boolean;
	/** A target peak force in the hammer row's unit, and a tolerance in percent: a signal only. */
	target?: { peak: number; tolerance_pct?: number } | null;
}

export interface ModalShakerConfig {
	drives: { row: number; level_vrms?: number }[];
	source?: ModalSource;
	burst_pct?: number;
	ramp_s?: number;
	ceiling_v?: number;
	delay_frames?: number;
	overlap_pct?: number;
	seed?: number;
}

export interface ModalTestConfig {
	title?: string;
	operator?: string;
	article?: string;
	notes?: string;
	specification?: { document?: string; clause?: string } | null;
	capture?: { run_name?: string };
	record?: { streams?: boolean };
	metadata?: { key: string; label?: string; value: string }[];
	mode: ModalMode;
	fs_hz: number;
	/** A power of two, 256 to 65536 (default 2048). */
	block?: number;
	band?: { lo_hz?: number; hi_hz?: number };
	estimator?: ModalEstimator;
	averages?: number;
	roving?: { channels: number[]; points: ModalPoints };
	impact?: ModalImpactConfig;
	shaker?: ModalShakerConfig;
}

// --- the check a configure returns -----------------------------------------------------------

/** A row as the check and the state name it: its table index, unit and DOF. */
export interface ModalRowDof {
	index: number;
	node: number | null;
	direction: string | null;
	unit: string;
	/** The row's range, in its unit (the check's rows only). */
	range?: number;
	/** The row roves (the check's rows only). */
	roves?: boolean;
}

export interface ModalCheckImpact {
	drives: string;
	/** `level` is signed, in the hammer's polarity; `watches` says what the trigger watches, in words. */
	trigger: { level: number; unit: string; polarity: ModalPolarity; watches: string; pretrigger_frames: number };
	holdoff_blocks: number;
	accept: ModalAcceptMode;
	auto_advance: boolean;
	checks: {
		overload: string;
		double_hit: string;
		tip_warn_db: number;
		target: { peak: number; tolerance_pct: number; unit: string; rejects: false } | null;
	};
	force_window: { pre_frames: number; width_frames: number; fall_frames: number } | null;
	exponential_window: { tau_s: number; inv_tau_per_s: number; end: number; added_damping: string } | null;
}

export interface ModalCheckShaker {
	source: ModalSource;
	crest: number;
	drives: { row: number; level_vrms: number; peak_v: number; range_v: number }[];
	ceiling_v: number;
	burst_pct: number | null;
	ramp_s: number;
	delay_frames: number;
	overlap_pct: number | null;
	seed: number;
	window: 'hann' | 'uniform';
	stop: { depth_db: number; ramp_s: number; then: string };
	seconds_per_point: number;
}

/** The check of a setup that passes: what the service made of it, before anything is armed. */
export interface ModalTestCheck {
	source: DataSource;
	mode: ModalMode;
	fs_hz: number;
	block: number;
	frame_s: number;
	df_hz: number;
	band: { lo_hz: number; hi_hz: number; first_line: number; last_line: number; lines: number };
	/** `H1`, `H2`, `Hv`, or `matrix H1` with several references. */
	estimator: string;
	averages: number;
	references: ModalRowDof[];
	responses: ModalRowDof[];
	/** Each measurement's DOFs, in plan order. */
	points: { index: number; references: ModalRowDof[]; responses: ModalRowDof[] }[];
	impact?: ModalCheckImpact;
	shaker?: ModalCheckShaker;
}

export interface ModalTestDryRunResult {
	errors: ReportItem[];
	warnings: ReportItem[];
	/** Null when the setup is refused. */
	check: ModalTestCheck | null;
}

export interface ModalTestConfigureResult {
	state: ModalTestRun;
	check: ModalTestCheck;
	warnings: ReportItem[];
}

export interface ModalTestConfigResult {
	run_id: string | null;
	config: ModalTestConfig | null;
	check: ModalTestCheck | null;
}

/** `config_invalid`'s detail. */
export interface ModalTestInvalidDetail {
	errors?: ReportItem[];
	warnings?: ReportItem[];
	check?: ModalTestCheck | null;
}

// --- the state -------------------------------------------------------------------------------

export type ModalRunState = 'none' | 'configured' | 'starting' | 'idle' | 'arming' | 'armed' | 'captured' | 'measuring' | 'stopping' | 'point_done' | 'finished';
export type ModalDrivePhase = 'idle' | 'waiting' | 'ramping' | 'driving' | 'stopping' | 'zeroed';
export type ModalOutcome = 'ended' | 'aborted' | 'response_abort' | 'drive_underrun' | 'drive_ceiling' | 'source_failed' | 'input_overload' | 'loop_faulted' | 'loop_panicked' | 'run_panicked' | 'run_not_started' | 'arm_refused';
export type ModalTileStatus = 'pending' | 'accepted' | 'rejected' | 'undone';
export type ModalRejectReason = 'overload' | 'double_hit' | 'operator';
export type ModalTargetVerdict = 'high' | 'low' | 'on_target';

/**
 * The decay check: the response row (its table index) whose block ends loudest, that ratio of its
 * last 5% to its loudest 5% (a fraction), and whether it is over the configured threshold.
 */
export interface ModalDecay {
	row: number;
	ratio: number;
	warning: boolean;
}

/** The counts, kept by the service after every take, accept, reject and undo. */
export interface ModalCounts {
	/** `hits` in impact mode, `frames` in shaker mode, which counts frames in the same fields. */
	unit: 'hits' | 'frames';
	/** `decay` is the largest decay check over the point's accepted hits or frames. */
	point: { index: number; accepted: number; rejected: number; asked: number; decay?: ModalDecay | null };
	points: { index: number; accepted: number; rejected: number; taken: number; measured: boolean }[];
	totals: { accepted: number; taken: number; points_measured: number; points_planned: number };
}

/** One hit (or frame) taken at the selected point: its tile. */
export interface ModalTile {
	n: number;
	seq: number;
	status: ModalTileStatus;
	reason: ModalRejectReason | null;
	overload: boolean;
	double_hit: boolean;
	start_frame: number;
	/** Null when not checked (a shaker source other than the bursts). */
	decay?: ModalDecay | null;
	/** Impact only. */
	peak?: number;
	tip_warning?: boolean;
	force_window_cut?: boolean;
	/** With a target set only. */
	target_verdict?: ModalTargetVerdict;
	target_diff_pct?: number;
}

/** The hit's whole report, as the service checked it. */
export interface ModalHitReport {
	peak: number;
	peak_frame: number;
	/** The overloaded channels: 0 the reference, 1 + k response k. */
	overloaded: number[];
	/** The second impact: its top's frame from the block's start, its value and its ratio to the peak. */
	double_hit: { frame: number; peak: number; ratio: number } | null;
	/** Null when the spectrum at the band's top is zero (an unbounded drop). */
	tip_drop_db: number | null;
	tip_warning: boolean;
	/** Per response: true when it showed no response, null when not judged. */
	no_response: (boolean | null)[];
	force_window_cut: { pulse_end_frame: number; window_end_frame: number } | null;
	decay?: ModalDecay | null;
	/** The first reason the automatic rule rejects the hit for, or null. */
	rejected: ModalRejectReason | null;
	reasons: string[];
	warnings: string[];
	target?: number;
	target_tolerance_pct?: number;
	target_diff_pct?: number;
	target_verdict?: ModalTargetVerdict;
}

/** The latest hit: its tile, its point and its report (impact; a shaker frame's has neither). */
export interface ModalLatest extends ModalTile {
	point?: number;
	report?: ModalHitReport | null;
}

export interface ModalTestEnd {
	outcome: ModalOutcome;
	message: string;
	emergency_stop: boolean;
	at_s: number | null;
}

/** A configured test, live or finished. */
export interface ModalTestRun {
	run_id: string;
	state: Exclude<ModalRunState, 'none'>;
	mode: ModalMode;
	source: DataSource;
	station_class: StationClass;
	title: string;
	drive_phase: ModalDrivePhase;
	point: number;
	point_dofs: { references: ModalRowDof[]; responses: ModalRowDof[] };
	counts: ModalCounts;
	hits: ModalTile[];
	latest: ModalLatest | null;
	record: { folder: string; streams: boolean; problem: string | null } | null;
	end: ModalTestEnd | null;
}

export type ModalTestState = { run_id: null; state: 'none' } | ModalTestRun;

export function isModalRun(s: ModalTestState | null | undefined): s is ModalTestRun {
	return !!s && s.state !== 'none' && s.run_id !== null;
}

/** `{ok, counts, state}`: the answer of every operator call. */
export interface ModalCallResult {
	ok: boolean;
	counts?: ModalCounts;
	state: ModalTestState;
}

export type ModalHitKind = 'clean' | 'double' | 'overload';
export const MODAL_HIT_KINDS: readonly ModalHitKind[] = ['clean', 'double', 'overload'];

// --- the live frame --------------------------------------------------------------------------

export type ModalTraceRole = 'reference' | 'response' | 'monitor' | 'drive';

export interface ModalTrace {
	values: Float32Array | number[];
	/** The row's table index, its role and unit. */
	index?: number;
	role?: ModalTraceRole;
	unit?: string;
}

/** `modal_test.live`, ten a second while a test runs. */
export interface ModalTestLive {
	run_id: string;
	state: ModalRunState;
	point: number;
	time: { rate_hz: number; channels: ModalTrace[] };
	frf: {
		frequency_hz: Float32Array | number[];
		averages: number;
		/** `response` and `reference` index the state's `point_dofs` lists. */
		pairs: { response: number; reference: number; magnitude: Float32Array | number[]; phase_deg: Float32Array | number[] }[];
		coherence: { response: number; values: Float32Array | number[] }[];
	};
	/** Impact: the latest hit's own block (null until a hit is taken). */
	hit?: ModalLiveHit | null;
	/** Shaker: each reference's autospectrum in the present average. */
	references?: ModalLiveReferences;
}

/** The latest hit's own block, references then responses, from the block's start. */
export interface ModalLiveHit {
	point: number;
	n: number;
	seq: number;
	start_frame: number;
	start_s: number;
	/** The block's length in samples; the values may be fewer when decimated. */
	samples: number;
	/** 1: every sample; otherwise each interval of `decimation` samples sends its two extremes. */
	decimation: number;
	/** The time between the values sent. */
	step_s: number;
	channels: ModalTrace[];
	/** The windows it was taken with, in samples from the block's start; null when off. */
	windows: {
		force: { pre_frames: number; width_frames: number; fall_frames: number } | null;
		exponential: { tau_s: number; end: number } | null;
	};
	force_spectrum: { frequency_hz: Float32Array | number[]; unit: string; values: Float32Array | number[]; tip_drop_db: number | null };
}

export interface ModalLiveReferences {
	frequency_hz: Float32Array | number[];
	averages: number;
	autospectra: { index?: number; unit?: string; values: Float32Array | number[] }[];
}

// --- the log ---------------------------------------------------------------------------------

export interface ModalTestLogEntry {
	seq: number;
	at_unix: number;
	kind: string;
	text: string;
}

export interface ModalTestLogResult {
	entries: ModalTestLogEntry[];
	next: number;
}

export interface ModalTestLogEvent {
	run_id: string;
	entries: ModalTestLogEntry[];
}

/** The reasons the calls name, as the service names them. */
export type ModalRefusalReason =
	| 'not_configured'
	| 'not_started'
	| 'not_live'
	| 'wrong_state'
	| 'run_finished'
	| 'run_active'
	| 'not_in_mode'
	| 'point_done'
	| 'no_hit'
	| 'busy'
	| 'not_simulated'
	| 'table_changed'
	| 'config_invalid'
	| 'record_unwritable'
	| 'hardware_busy'
	| 'arm_refused'
	| 'rate_mismatch'
	| 'already'
	| 'no_such_point'
	| 'nothing_to_undo'
	| 'ending';
