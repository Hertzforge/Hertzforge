/**
 * Classical shock on the wire (schema/PROTOCOL.md, "Classical Shock"): one pulse at a time,
 * closed loop between pulses, the drive in volts. The run's law is chosen from the control-law
 * catalog's `shock_classical` world (PROTOCOL.md, "Control Laws").
 */
import type {
	ClockDomain,
	DataSource,
	FeasibilityInputs,
	FeasibilityVerdict,
	ItemReason,
	LadderMode,
	LogEntry,
	RefusalReason,
	ReportItem,
	StationRef,
	View
} from './types';
import type { LawEntry } from './mimo';

/** Event topics of classical shock. */
export const SHOCK_TOPICS = {
	state: 'shock.state',
	log: 'shock.log',
	live: 'shock.live',
	pulse: 'shock.pulse',
	data: 'shock.data'
} as const;

/** Refusals of the `shock.*` methods: the shared ones and the three of their own. */
export type ShockRefusalReason = RefusalReason | 'not_yet' | 'wrong_mode' | 'drive_ceiling';

// --- configuration -------------------------------------------------------------------------

export type ShockShape =
	| 'half_sine'
	| 'haversine'
	| 'terminal_peak_sawtooth'
	| 'initial_peak_sawtooth'
	| 'trapezoid'
	| 'rectangle';

export type ShockPolarity = 'positive' | 'negative';

export type CompensationShape = 'half_sine' | 'rectangular' | 'rounded_rectangular';

export interface ShockCompensation {
	shape: CompensationShape;
	/** The pre-pulse's length, a multiple of the pulse duration. */
	pre_frac: number;
	/** The post-pulse's length; ignored when `symmetric`. */
	post_frac: number;
	symmetric: boolean;
}

export interface ShockPulse {
	shape: ShockShape;
	amplitude_g: number;
	duration_ms: number;
	/** Default `positive`. */
	polarity?: ShockPolarity;
	/** `trapezoid` only: fractions of the duration. */
	rise_frac?: number | null;
	fall_frac?: number | null;
	/** Default rectangular shoulders, 4 times the duration, symmetric. */
	compensation?: ShockCompensation;
}

export type ShockStandardName = 'mil_std_810_516' | 'iec_60068_2_27';

export interface ShockStandard {
	name?: ShockStandardName;
	/** Default `full_level`: failures below full level alarm. */
	tolerance_abort?: 'full_level' | 'every_level';
	/** Fraction over the commanded peak; null: twice the standard's amplitude tolerance. */
	overshoot_abort?: number | null;
}

export interface ShockAcquisition {
	fs_hz?: number;
	srs?: { f_lo_hz?: number; f_hi_hz?: number; bands_per_octave?: number; band_tol_db?: number };
	pvss?: boolean;
}

export type ShockAdvance = 'count' | 'on_pass';

export interface ShockSchedule {
	mode: LadderMode;
	levels_db: number[];
	/** One value for every level, or one per level. */
	pulses: number[];
	start_db?: number;
	/** Default `count`. */
	advance?: ShockAdvance;
	max_pulses_per_level?: number;
}

export interface ShockSpacing {
	cooling_s?: number;
	settling_s?: number;
	compute_s?: number;
}

export interface ShockIdentification {
	drive_vrms?: number;
	duration_s?: number;
	nfft?: number;
	band_hz?: [number, number];
	min_mean_coherence?: number;
	review?: boolean;
}

export interface ShockLawChoice {
	id: string;
	params?: Record<string, unknown>;
}

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface ShockControl {
	law?: ShockLawChoice;
	band_hz?: [number, number];
	coherence_floor?: number;
	tikhonov_floor?: number;
	ref_floor_db?: number;
	min_band_energy?: number;
	law_budget?: number;
	law_misses?: number;
	max_pulses?: number;
	max_peak_rise_db?: number;
	max_bin_rise_db?: number;
	first_drive_ratio?: number;
	divergence_pulses?: number;
}

export interface ShockMonitor {
	channel: number;
	unit: string;
	peak_ceiling: number;
	kind: 'predictive' | 'between_pulse';
}

export interface ShockLimits {
	drive_vpk_ceiling: number;
	drive_vpk_abort: number;
	peak_accel_g?: number | null;
	monitors?: ShockMonitor[];
}

export interface ShockFeasibility extends Omit<FeasibilityInputs, 'crest_factor'> {
	residual_velocity_ips?: number;
	residual_disp_in?: number;
}

export interface ShockConfig {
	title: string;
	pulse: ShockPulse;
	standard?: ShockStandard;
	acquisition?: ShockAcquisition;
	schedule: ShockSchedule;
	spacing?: ShockSpacing;
	identification?: ShockIdentification;
	control?: ShockControl;
	limits: ShockLimits;
	feasibility?: ShockFeasibility | null;
	train?: { count: number; spacing_s: number };
	gunfire?: { rounds_per_min: number; rounds: number } | null;
	capture?: { auto_start: boolean; run_name: string };
	metadata?: { key: string; label: string; value: string }[];
}

export interface ShockConfigureParams {
	config: ShockConfig;
	dry_run?: boolean;
}

export type ShockItemReason =
	| ItemReason
	| 'pulses_length_mismatch'
	| 'record_too_long'
	| 'spacing_below_floor'
	| 'quiet_window_exceeded';

export type ShockReportItem = Omit<ReportItem, 'reason'> & { reason: ShockItemReason };

export interface ShockGrid {
	fs_hz: number;
	record_len: number;
	/** True once the run has armed and the rate is the one the station runs at. */
	actual: boolean;
}

export interface ShockReferenceFacts {
	peak_g: number;
	velocity_change_ips: number;
	displacement_pk_pk_in: number;
	main_start: number;
	main_len: number;
}

export interface ShockQuietWindow {
	/** The composite's worst deviation in the quiet window, a fraction of the peak. */
	worst_frac: number;
	/** The standard's amplitude tolerance, a fraction of the peak. */
	limit_frac: number;
	inside: boolean;
}

export interface ShockDerived {
	grid: ShockGrid;
	reference: ShockReferenceFacts;
	quiet_window: ShockQuietWindow;
	/** The least time between pulses, seconds. */
	spacing_s: number;
	/** One law call's time budget, seconds. */
	call_budget_s: number;
	feasibility: FeasibilityVerdict;
}

export interface ShockConfigureResult {
	state: ShockState;
	derived: ShockDerived;
	warnings: ShockReportItem[];
}

export interface ShockDryRunResult {
	errors: ShockReportItem[];
	warnings: ShockReportItem[];
	derived: ShockDerived | null;
	views: View[];
}

/** `shock.config`: what the present run was configured with; all null when unconfigured. */
export interface ShockConfigResult {
	run_id: string | null;
	config: ShockConfig | null;
	derived: ShockDerived | null;
	views: View[];
}

export interface ShockPreviewParams {
	pulse: ShockPulse;
	standard?: ShockStandard;
	fs_hz?: number;
}

export interface ShockPreviewResult {
	derived: { reference: ShockReferenceFacts; quiet_window: ShockQuietWindow };
	/** Pulse (with its tolerance band), Velocity, Displacement. */
	views: View[];
}

// --- running -------------------------------------------------------------------------------

export interface ShockArmParams {
	/** The operator's acknowledgement to arm past a feasibility exceedance; logged verbatim. */
	override_ack?: string;
}

export type ShockLevelParams = { delta_db: number } | { rung: number };

export interface ShockLevelResult {
	level_db: number;
}

export interface ShockTrainParams {
	count?: number;
	spacing_s?: number;
}

export type ShockRunState =
	| 'unconfigured'
	| 'configured'
	| 'blocked'
	| 'armed'
	| 'running'
	| 'review'
	| 'stopping'
	| 'finished';

export type ShockPhase =
	| 'idle'
	| 'feasibility_blocked'
	| 'armed'
	| 'sysid'
	| 'waiting'
	| 'firing'
	| 'awaiting_review'
	| 'stopping'
	| 'stopped'
	| 'aborted'
	| 'held';

export type ShockOutcome =
	| 'completed'
	| 'operator_stop'
	| 'operator_abort'
	| 'safe_stop'
	| 'aborted'
	| 'not_converged'
	| 'held'
	| 'law_fault'
	| 'fault';

/** Why a classical law call was not used. */
export type ShockLawFaultReason =
	| 'error'
	| 'panicked'
	| 'over_budget'
	| 'over_memory'
	| 'wrong_length'
	| 'non_finite'
	| 'undeclared_value'
	| 'too_many_values'
	| 'peak_rise'
	| 'bin_rise'
	| 'first_drive';

export type ShockTrip = { pulse: number; at_level_db: number | null } & (
	| {
			kind: 'tolerance';
			envelope_pass: boolean;
			velocity_change_pass: boolean;
			error_pct: number;
			worst_deviation_frac: number;
	  }
	| { kind: 'overshoot'; peak_ratio: number; limit_ratio: number }
	| { kind: 'kinematic'; axis: 'velocity' | 'displacement'; value: number; limit: number; predicted: boolean }
	| { kind: 'drive_vpk'; vpk: number; abort_vpk: number; predicted: boolean }
	| { kind: 'accel'; predicted_g: number; limit_g: number }
	| { kind: 'monitor'; channel: number; value: number; limit: number }
	| { kind: 'divergence'; pulses: number; error_pct: number }
	| { kind: 'not_converged'; limit: 'per_level' | 'run'; pulses: number }
	| { kind: 'law_fault'; reason: ShockLawFaultReason; consecutive: number }
);

export interface ShockEnd {
	outcome: ShockOutcome;
	message: string;
	at_s: number | null;
	level_db: number | null;
	/** The pulse the end came at; null before any pulse. */
	pulse: number | null;
	/** Present when `outcome` is `aborted`, `not_converged` or `law_fault`. */
	trip: ShockTrip | null;
}

export interface ShockIdentificationFacts {
	drive_vrms: number;
	drive_vpk: number;
	coherence_mean: number;
	min_mean_coherence: number;
	passed: boolean;
}

export interface ShockPrediction {
	drive_vpk: number;
	drive_vpk_ceiling: number;
	drive_vpk_abort: number;
	response_peak_g: number;
	velocity_pk_ips: number;
	displacement_pk_pk_in: number;
}

export interface ShockLawRef {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	/** Unusable calls in a row now. */
	misses: number;
}

/** One fired pulse (`shock.pulses`, `shock.pulse`, the `shock.pulse` event). */
export interface ShockPulseRow {
	index: number;
	origin: 'ladder' | 'train' | 'gunfire';
	scheduled_db: number;
	commanded_db: number;
	/** The drive-limit scale applied; 1 when none. */
	limit_scale: number;
	drive_vpk: number;
	peak_g: number | null;
	peak_ratio: number | null;
	error_pct: number | null;
	velocity_change_ips: number | null;
	velocity_change_ratio: number | null;
	verdicts: { envelope: boolean; velocity: boolean; substitution: boolean } | null;
	passed: boolean | null;
	srs: { bands: number; passed_bands: number } | null;
	at_s: number;
	/** The call that produced the drive; null for a pulse fired in force by a train. */
	law: {
		used: boolean;
		miss: ShockLawFaultReason | null;
		capped_bins: number;
		peak_capped: boolean;
		call_ms: number;
	} | null;
}

/** The `shock.state` result and event. */
export interface ShockState {
	run_id: string | null;
	state: ShockRunState;
	phase: ShockPhase;
	title: string | null;
	source: DataSource | null;
	station: StationRef | null;
	grid: ShockGrid | null;
	clock: ClockDomain | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	/** The commanded level; null when nothing is scheduled. */
	level_db: number | null;
	rung: number | null;
	pulses: { fired: number; passed: number; at_level: number; of_level: number };
	/** Seconds to the next pulse; null when none is due. */
	next_pulse_s: number | null;
	elapsed_s: number | null;
	alarm: boolean;
	identification: ShockIdentificationFacts | null;
	prediction: ShockPrediction | null;
	last_pulse: ShockPulseRow | null;
	law: ShockLawRef | null;
	feasibility: FeasibilityVerdict | null;
	end: ShockEnd | null;
}

export interface ShockLogResult {
	entries: LogEntry[];
	next: number;
}

export interface ShockLogEvent {
	run_id: string;
	entries: LogEntry[];
}

export interface ShockPulsesParams {
	since?: number;
}

export interface ShockPulsesResult {
	pulses: ShockPulseRow[];
	next: number;
}

export interface ShockPulseParams {
	index: number;
	/** The width the views are decimated to; the service's default when left out. */
	points?: number;
}

export interface ShockPulseResult {
	pulse: ShockPulseRow;
	views: View[];
}

// --- display -------------------------------------------------------------------------------

export type ShockQuantity =
	| 'pulse'
	| 'velocity'
	| 'displacement'
	| 'srs'
	| 'spectrum'
	| 'identification'
	| 'law'
	| 'readings'
	| 'pulses';

export interface ShockSubscribePlot {
	id: string;
	quantity: ShockQuantity;
	points?: number;
}

export type ShockSubscribeParams = { rate_hz?: number; plots: ShockSubscribePlot[] } | { default: true };

export interface ShockSubscribeResult {
	default: boolean;
	rate_hz: number;
	plots: ShockSubscribePlot[] | null;
	quantities: ShockQuantity[];
}

/** The `shock.live` event, 5 times a second while the run is armed through finished. */
export interface ShockLive {
	run_id: string;
	source: DataSource;
	state: ShockRunState;
	phase: ShockPhase;
	mode: LadderMode | null;
	level_db: number | null;
	rung: number | null;
	pulses: { fired: number; passed: number; at_level: number; of_level: number };
	next_pulse_s: number | null;
	/** The fire in flight: frames delivered of expected; null between pulses. */
	fire: { done: number; of: number } | null;
}

/** The `shock.pulse` event: to every connection, once per pulse. */
export interface ShockPulseEvent {
	run_id: string;
	pulse: ShockPulseRow;
	views: View[];
}

/** One plot's data in a `shock.data` frame. */
export interface ShockDataPlot {
	id: string;
	quantity: ShockQuantity;
	views: View[];
}

/** The `shock.data` event: a subscribed connection's own frame. */
export interface ShockData extends ShockLive {
	plots: ShockDataPlot[];
}

// --- the law catalog and check (PROTOCOL.md, "Control Laws") ------------------------------

export type ShockCheckRequirement = 'every_pulse' | 'pass_by' | 'survive';

/** One simulated test article's result in a classical law check. */
export interface ShockLawCheckCase {
	name: string;
	plant: string;
	requirement: ShockCheckRequirement;
	passed: boolean;
	detail: string;
	end: ShockOutcome;
	trip: string | null;
	pulses: number;
	first_pass_pulse: number | null;
	full_level_pulses: number;
	full_level_passed: number;
	misses: number;
	refused: number;
	capped_bins: number;
	calls: number;
	worst_call_ms: number | null;
	budget_ms: number;
	max_drive_vpk: number;
}

export interface ShockLawsCheckResult {
	/** The law's entry after the check: a law file's `check` now holds the record. */
	law: LawEntry;
	report: { passed: boolean; recorded: boolean; tool_version: string; cases: ShockLawCheckCase[] };
}
