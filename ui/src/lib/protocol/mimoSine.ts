/**
 * MIMO Sine on the wire (schema/PROTOCOL.md, "MIMO Sine"): one tone at a time on several drives at
 * once, each drive with its own amplitude and phase, controlled at several control channels each
 * against its own reference amplitude and phase. A sweep, a dwell or a stepped sine. The run's law
 * is chosen from the control-law catalog's `sine_mimo` world. Where a part of the wire is Sine's
 * or MIMO Shock's, the type is theirs.
 */
import type {
	ClockDomain,
	DataSource,
	FeasibilityVerdict,
	LadderMode,
	LogEntry,
	ReadingView,
	ReportItem,
	RunRecordRef,
	SpectrumView,
	StationClass,
	StationRef,
	View
} from './types';
import type { LawEntry, MimoShape } from './mimo';
import type { ShockLawChoice } from './shock';
import type {
	SineAcquisition,
	SineDirection,
	SineDwellFor,
	SineDwellTest,
	SineFeasibility,
	SineGrid,
	SineItemReason,
	SineLimitChannel,
	SinePhase,
	SineProfile,
	SineProfileFacts,
	SineRefusalReason,
	SineRunState,
	SineSchedule,
	SineStepTest,
	SineSweepTest,
	SineTracking,
	SineTrackingVerdict
} from './sine';

/** Event topics of MIMO Sine. */
export const MIMO_SINE_TOPICS = {
	state: 'mimo_sine.state',
	log: 'mimo_sine.log',
	live: 'mimo_sine.live',
	sweep: 'mimo_sine.sweep',
	data: 'mimo_sine.data',
	report: 'mimo_sine.report'
} as const;

/** The law world MIMO Sine's laws are listed under. */
export const MIMO_SINE_WORLD = 'sine_mimo';

/**
 * Refusals of the `mimo_sine.*` methods: Sine's, and `reverse_not_available` (a sweep turns only at
 * the band's edges) and `mode_fixed` (the ladder's mode is set before the run arms).
 */
export type MimoSineRefusalReason = SineRefusalReason | 'reverse_not_available' | 'mode_fixed';

export type MimoSineTestType = 'sweep' | 'dwell' | 'stepped';

/** A catalog entry of the `sine_mimo` world: the shapes, test types and references it accepts. */
export type MimoSineLawEntry = LawEntry & {
	world: 'sine_mimo';
	shapes: MimoShape[];
	test_types: MimoSineTestType[];
	amplitude_only: boolean;
	/** True on the world's default law alone. */
	default: boolean;
	/** A built-in law's limits as measured, in words; null for a law file. */
	limits: string | null;
};

// --- configuration -------------------------------------------------------------------------

/** One point of a control's reference phase against frequency. */
export interface MimoSinePhasePoint {
	/** Above 0, ascending. */
	freq_hz: number;
	/** Above -180, at most 180, relative to the tone. */
	phase_deg: number;
}

export interface MimoSineControl {
	/** A table `index`: an enabled control row. */
	channel: number;
	/** The reference amplitude at full level and its bands, Sine's form. */
	profile: SineProfile;
	/** Required on every control when `phase_specified`; ignored otherwise. */
	phase?: MimoSinePhasePoint[];
}

export interface MimoSineDrive {
	/** A table `index`: an enabled drive row. */
	channel: number;
	/** Volts peak, above 0, at most 10. */
	ceiling_vpk: number;
	/** Volts peak, above the ceiling, at most 10. */
	abort_vpk: number;
}

export interface MimoSineChannels {
	/** 1 to 24. */
	controls: MimoSineControl[];
	/** 1 to 12, in the table's order. */
	drives: MimoSineDrive[];
}

/** A stepped sine on a list of frequencies, played in the order given. */
export interface MimoSineStepListTest {
	kind: 'stepped';
	frequencies_hz: number[];
	dwell: SineDwellFor;
}

/** A stepped sine on a grid: Sine's step test under the kind `stepped`. */
export type MimoSineStepGridTest = Omit<SineStepTest, 'kind'> & { kind: 'stepped' };

export type MimoSineTest = SineSweepTest | SineDwellTest | MimoSineStepGridTest | MimoSineStepListTest;

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface MimoSineIdentification {
	level_db?: number;
	/** The identification's line spacing, Hz (0.05 to 4); each record lasts about its inverse. */
	resolution_hz?: number;
	/** Null: the larger of 24 and six times the drives. */
	frames?: number | null;
	trust_coherence?: number;
	gate_coherence?: number;
	review?: boolean;
}

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface MimoSineControlSettings {
	tracking?: SineTracking;
	/** The law by catalog id, with parameter values by key (world `sine_mimo`). */
	law?: ShockLawChoice;
	law_budget?: number;
	law_misses?: number;
	max_step_db?: number;
	max_rise_db_per_s?: number;
	/** Degrees per tick; null for no cap. */
	rotation_cap_deg?: number | null;
	divergence_s?: number;
	abort_ticks?: number;
}

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface MimoSineRequests {
	honour?: boolean;
	max_hold_s?: number;
	min_slow_factor?: number;
}

export interface MimoSineNotch {
	/** Ticks each limit channel's reading is smoothed over; default 3, at least 1. */
	smoothing_ticks?: number;
	/** How fast the notch's depth may rise back towards the whole reference, dB/s; default 10, above 0. */
	release_ramp_db_per_s?: number;
	/** How far the readings must allow the depth to rise before it begins to, dB; default 1, 0 to 6. */
	release_hysteresis_db?: number;
	/** How near its bound a limit channel must be for a law's notched reference to be taken, dB; default 3. */
	near_bound_db?: number;
}

export interface MimoSineLimits {
	/** Default true; needs `feasibility`. */
	kinematic_monitor?: boolean;
	/** 0 to 8; Sine's limit channels. */
	channels?: SineLimitChannel[];
	notch?: MimoSineNotch;
}

/** One shaker's record and moving mass, Sine's, for the drive it is on. */
export type MimoSineShaker = SineFeasibility & { drive: number };

export interface MimoSineConfig {
	title?: string;
	channels: MimoSineChannels;
	/** Default true. */
	phase_specified?: boolean;
	test: MimoSineTest;
	acquisition?: SineAcquisition;
	schedule?: SineSchedule;
	identification?: MimoSineIdentification;
	control?: MimoSineControlSettings;
	requests?: MimoSineRequests;
	limits?: MimoSineLimits;
	feasibility?: { shakers: MimoSineShaker[] } | null;
	operator?: string;
	article?: string;
	notes?: string;
	specification?: { document: string; clause: string } | null;
	record?: { streams?: boolean } | null;
	metadata?: { key: string; label?: string; value: string }[];
}

export interface MimoSineConfigureParams {
	config: MimoSineConfig;
	dry_run?: boolean;
}

export type MimoSineItemReason =
	| SineItemReason
	| 'controls_out_of_range'
	| 'drives_out_of_range'
	| 'limits_out_of_range'
	| 'duplicate_channel'
	| 'control_unknown_channel'
	| 'drive_unknown_channel'
	| 'phase_missing'
	| 'phase_invalid'
	| 'steps_empty'
	| 'structure_unknown'
	| 'structure_wrong_size'
	| 'shaker_record_invalid'
	| 'law_shape_not_accepted'
	| 'law_test_type_not_accepted'
	| 'law_amplitude_only_not_accepted';

export type MimoSineReportItem = Omit<ReportItem, 'reason'> & { reason: MimoSineItemReason };

export interface MimoSineProfileFacts extends SineProfileFacts {
	channel: number;
}

export interface MimoSineLawFacts {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	shapes: MimoShape[];
	test_types: MimoSineTestType[];
	amplitude_only: boolean;
}

export interface MimoSineDerived {
	band_hz: [number, number];
	shape: MimoShape;
	controls: number;
	drives: number;
	limits: number;
	grid: SineGrid;
	profiles: MimoSineProfileFacts[];
	test_s: number | null;
	identification_s: number;
	call_budget_s: number;
	stop_ramp_db_per_s: number;
	safe_stop_s: number;
	tracking: SineTrackingVerdict;
	law: MimoSineLawFacts;
	feasibility: FeasibilityVerdict;
}

export interface MimoSineConfigureResult {
	state: MimoSineState;
	derived: MimoSineDerived;
	warnings: MimoSineReportItem[];
}

export interface MimoSineDryRunResult {
	errors: MimoSineReportItem[];
	warnings: MimoSineReportItem[];
	derived: MimoSineDerived | null;
	/** Profile, Phase and Limit views; empty with errors. */
	views: SpectrumView[];
}

export interface MimoSineConfigResult {
	run_id: string | null;
	config: MimoSineConfig | null;
	derived: MimoSineDerived | null;
	views: SpectrumView[];
}

// --- running -------------------------------------------------------------------------------

export interface MimoSineArmParams {
	override_ack?: string;
}

export interface MimoSineProceedParams {
	acknowledge?: string;
}

export type MimoSineLevelParams = { delta_db: number } | { to_db: number };

export interface MimoSineJogParams {
	delta_hz: number;
}

export interface MimoSineGotoParams {
	hz: number;
}

// --- run state -----------------------------------------------------------------------------

export type MimoSineRunState = SineRunState;

export type MimoSinePhase = SinePhase;

export type MimoSineOutcome =
	| 'completed'
	| 'operator_stop'
	| 'operator_abort'
	| 'safe_stop'
	| 'aborted'
	| 'held'
	| 'law_fault'
	| 'fault';

/** Why a MIMO Sine law call was not used. */
export type MimoSineLawFaultReason =
	| 'error'
	| 'panicked'
	| 'over_budget'
	| 'over_memory'
	| 'wrong_drive_count'
	| 'non_finite'
	| 'too_many_values'
	| 'undeclared_value'
	| 'bad_request'
	| 'wrong_reference_count';

export type MimoSineTrip = { at_hz: number; at_level_db: number; at_tick: number } & (
	| { kind: 'abort_band'; channel: number; side: 'high' | 'low'; measured: number; limit: number }
	| { kind: 'kinematic'; drive: number; axis: 'velocity' | 'displacement'; value: number; limit: number }
	| { kind: 'limit_channel'; channel: number; measured: number; limit: number }
	| { kind: 'drive_vpk'; channel: number; vpk: number; abort_vpk: number }
	| {
			kind: 'watchdog';
			channel: number;
			axis: string;
			value: number;
			limit: number;
			measured: number;
			drive_vpk: number;
	  }
	| { kind: 'oscillator_range'; requested_hz: number; fs_hz: number }
	| { kind: 'divergence'; capped_s: number; cap: 'step' | 'rise' | 'rotation' }
	| { kind: 'law_fault'; reason: MimoSineLawFaultReason; consecutive: number }
);

export interface MimoSineEnd {
	outcome: MimoSineOutcome;
	message: string;
	at_s: number | null;
	level_db: number | null;
	at_hz: number | null;
	ramp_s: number | null;
	trip: MimoSineTrip | null;
}

export interface MimoSineIdentificationFacts {
	level_db: number;
	/** The level the law is told the structure was identified at, dB; null when not measured. */
	measured_level_db: number | null;
	realizations: number;
	/** The line spacing the identification ran at, Hz. */
	line_spacing_hz: number;
	/** How long it played on the station, s. */
	duration_s: number;
	drive_vrms: number[];
	coherence_mean: number[];
	limit_coherence_mean: number[];
	trusted_frac: number;
	condition_max: number;
	lag_frames: number;
	gate_coherence: number;
	passed: boolean;
}

/** A resonance the identification found, against the sweep's rate. */
export interface MimoSineResonance {
	freq_hz: number;
	bandwidth_hz: number;
	/** The resonance's own time constants the sweep spends inside its half-power bandwidth. */
	time_constants: number;
	/** Under two. */
	flagged: boolean;
}

export interface MimoSineReview {
	resonances: MimoSineResonance[];
	acknowledged: string | null;
}

export interface MimoSineLawRef {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	params: Record<string, unknown>;
	misses: number;
	capped_ticks: number;
	over_ceiling: number;
	/** The largest turn of a drive's phase in one tick the law has asked for, degrees. */
	rotation_max_deg: number | null;
	requests: { asked: number; granted: number };
}

/** The `mimo_sine.state` result and event. */
export interface MimoSineState {
	run_id: string | null;
	state: MimoSineRunState;
	phase: MimoSinePhase;
	title: string | null;
	source: DataSource | null;
	/** `structure` is the simulator's structure, null on NI. */
	station: (StationRef & { structure?: string | null }) | null;
	grid: SineGrid | null;
	clock: ClockDomain | null;
	test: MimoSineTestType | null;
	shape: MimoShape | null;
	phase_specified: boolean | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	level_db: number | null;
	rung: number | null;
	frequency_hz: number | null;
	direction: SineDirection | null;
	sweeps: { done: number; of: number | null } | null;
	steps: { done: number; of: number } | null;
	held: boolean;
	request_held: boolean;
	elapsed_s: number | null;
	at_level_s: number | null;
	remaining_s: number | null;
	alarm: boolean;
	/** True while the limit channels hold the reference down (the notch's depth under 1) or cut the drive vector. */
	notch: boolean;
	/** The scale the whole drive vector was cut to on the last update, 1 when none; null before the run. */
	notch_scale: number | null;
	identification: MimoSineIdentificationFacts | null;
	review: MimoSineReview | null;
	law: MimoSineLawRef | null;
	feasibility: FeasibilityVerdict | null;
	end: MimoSineEnd | null;
	record?: RunRecordRef;
}

export interface MimoSineLogResult {
	entries: LogEntry[];
	next: number;
}

export interface MimoSineLogEvent {
	run_id: string;
	entries: LogEntry[];
}

// --- report --------------------------------------------------------------------------------

export interface MimoSineReport {
	run_id: string;
	title: string | null;
	source: DataSource;
	grid: SineGrid;
	test: MimoSineTestType;
	shape: MimoShape;
	phase_specified: boolean;
	law: MimoSineLawRef;
	levels_db: number[];
	controls: {
		channel: number;
		worst_error_db: number | null;
		worst_error_hz: number | null;
		worst_phase_error_deg: number | null;
		alarm_s: number;
		notch_depth_max_db: number;
	}[];
	/** `notched_s`: the time the notch was in (the reference held down or the drive vector cut), s. */
	limits: { channel: number; worst_margin_db: number | null; worst_margin_hz: number | null; notched_s: number }[];
	drives: { channel: number; vpk_max: number; vpk_max_hz: number | null; ceiling_vpk: number; over_ceiling_ticks: number }[];
	readings: { opposition_ratio_max: number | null; largest_share_max: number | null };
	requests: { asked: number; granted: number; held_s: number };
	identification: MimoSineIdentificationFacts | null;
	end: MimoSineEnd;
}

/** One case of a MIMO Sine law's check (`laws.check` with `"world":"sine_mimo"`). */
export interface MimoSineLawCheckCase {
	case: string;
	/** The case's description and the structure it ran on; a law set up for weakly coupled pairs runs on `sine-weak-4x4`. */
	structure: string;
	scored: boolean;
	passed: boolean;
	/** The first requirement that failed, by name; null when every one held. */
	failed: string | null;
	number: number | null;
	detail: string;
	/** The law's refused answers, and of those the ones refused only for coming late; null when the case could not run. */
	misses: number | null;
	late: number | null;
	/** The law's calls, the slowest and the median, ms, and how many were over the setup's budget of `budget_ms`; null when the case could not run. */
	calls: number | null;
	worst_call_ms: number | null;
	median_call_ms: number | null;
	over_budget: number | null;
	budget_ms: number | null;
}

/** The `laws.check` result for a MIMO Sine law. */
export interface MimoSineLawsCheckResult {
	law: LawEntry;
	report: { passed: boolean; recorded: boolean; tool_version: string; cases: MimoSineLawCheckCase[] };
}

export interface MimoSineReportResult {
	report: MimoSineReport | null;
}

// --- display -------------------------------------------------------------------------------

export type MimoSineQuantity =
	| 'control'
	| 'phase'
	| 'error'
	| 'limit'
	| 'drive'
	| 'drive_phase'
	| 'readings'
	| 'time'
	| 'identification'
	| 'law';

export interface MimoSineSubscribePlot {
	id: string;
	quantity: MimoSineQuantity;
	/** `control`, `phase`, `limit`, `time` and `identification` only. */
	channels?: number[] | 'all';
	/** `time` only. */
	points?: number;
}

export type MimoSineSubscribeParams = { rate_hz?: number; plots: MimoSineSubscribePlot[] } | { default: true };

export interface MimoSineSubscribeResult {
	default: boolean;
	rate_hz: number;
	plots: MimoSineSubscribePlot[] | null;
	quantities: MimoSineQuantity[];
}

export interface MimoSineLiveControl {
	channel: number;
	description: string;
	unit: string;
	/** Tracked amplitude at the tone, peak. */
	amplitude: number | null;
	/** The response's phase relative to the tone, degrees, in (-180, 180]. */
	phase_deg: number | null;
	/** The reference at this frequency and level, peak. */
	reference: number | null;
	/** Null without `phase_specified`. */
	reference_phase_deg: number | null;
	/** 20 log10(amplitude / reference). */
	error_db: number | null;
	/** Null without `phase_specified`. */
	phase_error_deg: number | null;
	/** How far a taken notched reference sits under the reference, dB; 0 when none. */
	notch_depth_db: number;
	/** Sine's latch: 95 % of the top of the abort band reached. */
	overload: boolean;
}

export interface MimoSineLiveLimit {
	channel: number;
	description: string;
	unit: string;
	amplitude: number | null;
	phase_deg: number | null;
	/** The bound here; null where the channel has none. */
	bound: number | null;
	/** 20 log10(amplitude / bound), negative under it. */
	margin_db: number | null;
}

export interface MimoSineLiveDrive {
	channel: number;
	/** As played, volts peak. */
	vpk: number;
	/** Relative to the tone, degrees. */
	phase_deg: number;
	ceiling_vpk: number;
}

/** Shown and recorded, never judged. */
export interface MimoSineReadings {
	/** The drive vector's norm over the least norm giving the same control response; null untrusted. */
	opposition_ratio: number | null;
	/** The largest share of the drive vector's norm carried by one drive. */
	largest_share: number | null;
	/** The largest turn of a drive's phase this tick, degrees. */
	rotation_deg: number | null;
}

export type MimoSineRequest = ({ kind: 'hold' } | { kind: 'slow'; factor: number }) & { granted: boolean };

export interface MimoSineLawLive {
	values: Record<string, number>;
	misses: number;
	capped_ticks: number;
	faulted: boolean;
}

/** The numbers every live frame carries, `mimo_sine.live` and `mimo_sine.data` alike. */
export interface MimoSineLiveHead {
	run_id: string;
	source: DataSource;
	class: StationClass;
	state: 'running' | 'review' | 'stopping' | 'finished';
	phase: MimoSinePhase;
	test: MimoSineTestType;
	mode: LadderMode;
	t_s: number | null;
	elapsed_s: number | null;
	frequency_hz: number | null;
	direction: SineDirection | null;
	/** Exactly one of the two rates. */
	sweep_rate_oct_per_min?: number | null;
	sweep_rate_hz_per_s?: number | null;
	sweeps: { done: number; of: number | null } | null;
	steps: { done: number; of: number } | null;
	held: boolean;
	request_held: boolean;
	level_db: number | null;
	commanded_db: number | null;
	/** The state's: the reference held down or the drive vector cut. */
	notch: boolean;
	/** The state's: the drive vector's cut on the last update, 1 when none. */
	notch_scale: number;
	alarm: boolean;
	controls: MimoSineLiveControl[];
	limits: MimoSineLiveLimit[];
	drives: MimoSineLiveDrive[];
	readings: MimoSineReadings;
	request: MimoSineRequest | null;
	law: MimoSineLawLive | null;
}

/** The `mimo_sine.live` event: controls, drives and run views plus the numbers behind them. */
export interface MimoSineLive extends MimoSineLiveHead {
	views: [SpectrumView, SpectrumView, ReadingView];
}

/** The `mimo_sine.sweep` event: one pass's results against frequency. */
export interface MimoSineSweepEvent {
	run_id: string;
	/** From 1. */
	pass: number;
	level_db: number;
	/** Null in a stepped sine. */
	direction: SineDirection | null;
	views: View[];
}

export interface MimoSineDataPlot {
	id: string;
	quantity: MimoSineQuantity;
	channels: number[] | null;
	views: View[];
}

/** The `mimo_sine.data` event: a subscribed connection's own frame. */
export interface MimoSineData extends MimoSineLiveHead {
	plots: MimoSineDataPlot[];
}
