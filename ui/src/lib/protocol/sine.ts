/**
 * Sine on the wire (schema/PROTOCOL.md, "Sine"): one tone, its amplitude closed loop every
 * control tick, the drive in volts peak on hardware. The run's law is chosen from the
 * control-law catalog's `sine` world (PROTOCOL.md, "Control Laws"). Where a part of the wire is
 * Random's or classical shock's, the type is theirs.
 */
import type {
	BandDb,
	ChannelRole,
	ChannelSelector,
	ClockDomain,
	CompositeMode,
	DataSource,
	FeasibilityInputs,
	FeasibilityVerdict,
	ItemReason,
	LadderMode,
	LogEntry,
	RandomPhase,
	RandomRunState,
	ReadingView,
	RefusalReason,
	ReportItem,
	SpectrumView,
	StationClass,
	StationRef,
	View
} from './types';
import type { LawEntry } from './mimo';
import type { ShockLawChoice } from './shock';

/** Event topics of Sine. */
export const SINE_TOPICS = {
	state: 'sine.state',
	log: 'sine.log',
	live: 'sine.live',
	data: 'sine.data'
} as const;

/** Refusals of the `sine.*` methods: Random's, and the three of the sweep verbs and `sine.adjust`. */
export type SineRefusalReason = RefusalReason | 'not_a_sweep' | 'out_of_band' | 'adjust_refused';

/** `detail.why` of an `adjust_refused` refusal. */
export type SineAdjustRefusal = 'unknown_key' | 'not_live' | 'out_of_range' | 'law_refused';

// --- configuration -------------------------------------------------------------------------

/** What a profile row's value is: g peak, in/s peak, or in peak to peak. */
export type SineQuantity = 'accel_g' | 'velocity_ips' | 'disp_pp_in';

export interface SineProfileRow {
	freq_hz: number;
	quantity: SineQuantity;
	/** Above 0, in the row's quantity. */
	value: number;
	/** `[below, above]`, positive dB; the profile's bands when absent. */
	alarm_db?: BandDb;
	abort_db?: BandDb;
}

/** A control profile, or a limit channel's bound in the same form. */
export interface SineProfile {
	label?: string;
	/** At least two, frequencies strictly ascending. */
	rows: SineProfileRow[];
	/** Default `[1.5, 1.5]`. */
	alarm_db?: BandDb;
	/** Default `[3, 3]`. */
	abort_db?: BandDb;
}

export type SineRate = { law: 'log'; oct_per_min: number } | { law: 'linear'; hz_per_s: number };

export type SineDirection = 'up' | 'down';

/** How long a dwell (or one step of a step test) lasts. */
export type SineDwellFor = { seconds: number } | { cycles: number };

export interface SineSweepTest {
	kind: 'sweep';
	band_hz: [number, number];
	rate: SineRate;
	/** Default the band's low edge. */
	start_hz?: number;
	/** The first leg; default `up`. */
	direction?: SineDirection;
	/** When the run ends, counted over the whole run: one-way sweeps, test seconds, or both. */
	end: { sweeps?: number | null; duration_s?: number | null };
}

export interface SineDwellTest {
	kind: 'dwell';
	freq_hz: number;
	end: SineDwellFor;
}

export interface SineStepTest {
	kind: 'step';
	band_hz: [number, number];
	steps_per_octave: number;
	/** Default `log`. */
	spacing?: 'log' | 'linear';
	/** Null: a dwell computed per step. */
	dwell?: SineDwellFor | null;
}

/** Resonance search, track and dwell. On the wire; the shell does not offer it. */
export interface SineRstdTest {
	kind: 'rstd';
	band_hz: [number, number];
	level_db: number;
	rate: SineRate;
	tracking_bw_hz: number;
	/** Default 5. */
	settle_k?: number;
	dwell_mode: 'fixed' | 'phase_tracked';
	dwell: SineDwellFor;
}

export type SineTest = SineSweepTest | SineDwellTest | SineStepTest | SineRstdTest;

export type SineTestKind = SineTest['kind'];

export interface SineAcquisition {
	/** The rate asked of the station, 1000 to 204800 Hz; null asks for 16384 Hz. */
	fs_hz?: number | null;
	/** Default 10; converted to whole frames at the rate the station adopts. */
	tick_ms?: number;
}

/** How long a level is held: one-way sweeps (a sweep test only) or seconds. */
export type SineRungFor = { sweeps: number } | { seconds: number };

export interface SineSchedule {
	/** Default `auto`. */
	mode?: LadderMode;
	/** Ascending dB; the last is full level. Default `[-12, -9, -6, -3, 0]`. */
	levels_db?: number[];
	/** A level of the schedule; default the first. */
	start_db?: number;
	per_level?: SineRungFor;
	full_level?: SineRungFor;
}

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface SineIdentification {
	level_db?: number;
	gate_coherence?: number;
	trust_coherence?: number;
	trust_h_floor_frac?: number;
	review?: boolean;
}

export type SineTracking = { kind: 'fixed'; hz: number } | { kind: 'proportional'; pct: number; max_hz: number };

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface SineControl {
	composite?: CompositeMode;
	weights?: { channel: number; weight: number }[];
	tracking?: SineTracking;
	/** The law by catalog id, with parameter values by key (world `sine`). */
	law?: ShockLawChoice;
	law_budget?: number;
	law_misses?: number;
	max_step_db?: number;
	max_rise_db_per_s?: number;
	divergence_s?: number;
	fault_debounce_ticks?: number;
}

export interface SineLimitChannel {
	/** A table `index`: an enabled input that is not a control row. */
	channel: number;
	/** The bound at each frequency, in the profile's form. */
	profile: SineProfile;
	shared_name?: string | null;
}

export interface SineNotch {
	drive_floor?: number;
	smoothing_ticks?: number;
	release_ramp_db_per_s?: number;
}

export interface SineLimits {
	/** Volts peak; required (the wire has no default). */
	drive_vpk_ceiling: number;
	/** Volts peak, at least the ceiling; required. */
	drive_vpk_abort: number;
	/** Default true; needs `feasibility`. */
	kinematic_monitor?: boolean;
	channels?: SineLimitChannel[];
	notch?: SineNotch;
}

export interface SineCola {
	enabled: boolean;
	volts_pk?: number;
	waveform?: 'sine' | 'square' | 'dc';
}

export type SineCaptureRule = 'off' | 'entire_test' | 'manual' | 'first_seconds';

export interface SineCapture {
	/** Default `off`. */
	rule?: SineCaptureRule;
	/** With `first_seconds`: the seconds captured at each level. */
	first_s?: number | null;
	level_settle_s?: number;
	periodic_save_s?: number | null;
	/** The run folder's name under the local data root. */
	run_name?: string;
}

/** Random's shaker record and moving mass, without a crest factor (a sine's is 1). */
export type SineFeasibility = Omit<FeasibilityInputs, 'crest_factor'>;

export interface SineConfig {
	title?: string;
	profile: SineProfile;
	test: SineTest;
	acquisition?: SineAcquisition;
	schedule?: SineSchedule;
	identification?: SineIdentification;
	control?: SineControl;
	limits: SineLimits;
	cola?: SineCola;
	feasibility?: SineFeasibility | null;
	capture?: SineCapture;
	/** Random's fields; `label` is the key when absent. */
	metadata?: { key: string; label?: string; value: string }[];
}

export interface SineConfigureParams {
	config: SineConfig;
	dry_run?: boolean;
}

export type SineItemReason =
	| ItemReason
	| 'profile_too_short'
	| 'value_not_positive'
	| 'quantity_ambiguous'
	| 'profile_not_covering'
	| 'start_not_a_level'
	| 'end_before_full_level'
	| 'tick_out_of_range'
	| 'abort_below_ceiling'
	| 'limit_on_control_channel'
	| 'cola_needs_two_outputs'
	| 'safe_stop_budget'
	| 'tracking_lag'
	| 'profile_file'
	| 'join_discontinuity'
	| 'units_column_ignored';

export type SineReportItem = Omit<ReportItem, 'reason'> & { reason: SineItemReason };

export interface SineGrid {
	fs_hz: number;
	tick_frames: number;
	tick_s: number;
	/** True once the run has armed and the rate and tick are the station's. */
	actual: boolean;
}

/** Where the profile changes quantity. */
export interface SineCrossover {
	freq_hz: number;
	from: SineQuantity;
	to: SineQuantity;
}

export interface SineProfileFacts {
	crossovers: SineCrossover[];
	peak_accel_g: number;
	peak_velocity_ips: number;
	peak_disp_pp_in: number;
}

/** The sweep against the tracking filter. */
export interface SineTrackingVerdict {
	verdict: 'ok' | 'advisory' | 'refused';
	ratio: number;
	at_hz: number | null;
	message: string | null;
}

export interface SineLawFacts {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	/** The keys of the law's live parameters. */
	live: string[];
}

export interface SineDerived {
	band_hz: [number, number];
	grid: SineGrid;
	profile: SineProfileFacts;
	/** Test time of the schedule (holds excluded); null in manual. */
	test_s: number | null;
	identification_s: number;
	call_budget_s: number;
	/** The controlled stop's ramp: the service's own, read only. */
	stop_ramp_db_per_s: number;
	/** The ramp's length from the drive ceiling to zero, station time. */
	safe_stop_s: number;
	tracking: SineTrackingVerdict;
	law: SineLawFacts;
	feasibility: FeasibilityVerdict;
}

export interface SineConfigureResult {
	state: SineState;
	derived: SineDerived;
	warnings: SineReportItem[];
}

export interface SineDryRunResult {
	errors: SineReportItem[];
	warnings: SineReportItem[];
	derived: SineDerived | null;
	/** Profile, Velocity and Displacement; empty with errors. */
	views: SpectrumView[];
}

/** `sine.config`: what the present run was configured with; all null when unconfigured. */
export interface SineConfigResult {
	run_id: string | null;
	config: SineConfig | null;
	derived: SineDerived | null;
	views: SpectrumView[];
}

// --- profiles ------------------------------------------------------------------------------

/** Give `text` (pasted rows) or `path` (a full path on the computer that runs the test). */
export type SineProfileImportParams = { text: string } | { path: string };

export interface SineProfileImportResult {
	profile: SineProfile | null;
	errors: SineReportItem[];
	warnings: SineReportItem[];
}

export interface SineProfileValidateParams {
	profile: SineProfile;
	band_hz?: [number, number] | null;
	feasibility?: SineFeasibility | null;
}

export interface SineKinematics {
	peak_accel_g: number;
	peak_velocity_ips: number;
	peak_disp_pp_in: number;
	verdict: FeasibilityVerdict['verdict'];
	message: string;
}

export interface SineProfileValidateResult {
	errors: SineReportItem[];
	warnings: SineReportItem[];
	crossovers: SineCrossover[];
	kinematics: SineKinematics | null;
	/** Profile, Velocity and Displacement: a computation, never a measurement. */
	views: SpectrumView[];
}

// --- running -------------------------------------------------------------------------------

export interface SineArmParams {
	/** The operator's acknowledgement to arm past a feasibility exceedance; logged verbatim. */
	override_ack?: string;
}

export type SineLevelParams = { delta_db: number } | { to_db: number };

export interface SineLevelResult {
	level_db: number;
}

export interface SineModeParams {
	mode: LadderMode;
}

export interface SineModeResult {
	mode: LadderMode;
}

export interface SineJogParams {
	delta_hz: number;
}

export interface SineGotoParams {
	hz: number;
}

/** `sine.jog` and `sine.goto`: the frequency applied. */
export interface SineFrequencyResult {
	frequency_hz: number;
}

export interface SineAdjustParams {
	key: string;
	value: number;
}

export interface SineAdjustResult {
	key: string;
	value: number;
}

export type SineRunState = RandomRunState;

export type SinePhase = Extract<
	RandomPhase,
	| 'idle'
	| 'feasibility_blocked'
	| 'armed'
	| 'sysid'
	| 'coherence_gate'
	| 'engaged'
	| 'full_level'
	| 'awaiting_review'
	| 'ramping_down'
	| 'stopped'
	| 'aborted'
	| 'coherence_hold'
>;

export type SineOutcome =
	| 'completed'
	| 'operator_stop'
	| 'operator_abort'
	| 'safe_stop'
	| 'aborted'
	| 'held'
	| 'law_fault'
	| 'fault';

/** Why a Sine law call was not used. */
export type SineLawFaultReason =
	| 'error'
	| 'panicked'
	| 'over_budget'
	| 'over_memory'
	| 'non_finite'
	| 'negative'
	| 'undeclared_value'
	| 'too_many_values';

export type SineTrip = { at_hz: number; at_level_db: number; at_tick: number } & (
	| { kind: 'abort_band'; side: 'high' | 'low'; measured_g: number; limit_g: number }
	| { kind: 'kinematic'; axis: 'velocity' | 'displacement'; value: number; limit: number }
	| { kind: 'limit_channel'; channel: number; measured: number; limit: number }
	| { kind: 'drive_vpk'; vpk: number; abort_vpk: number }
	| { kind: 'servo_fault'; consecutive_ticks: number }
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
	| { kind: 'divergence'; capped_s: number }
	| { kind: 'law_fault'; reason: SineLawFaultReason; consecutive: number }
);

export interface SineEnd {
	outcome: SineOutcome;
	message: string;
	/** Station time from engage. */
	at_s: number | null;
	level_db: number | null;
	at_hz: number | null;
	/** The controlled stop's length on the station's clock; null for an abort. */
	ramp_s: number | null;
	/** Present when `outcome` is `aborted` or `law_fault`. */
	trip: SineTrip | null;
}

export interface SineIdentificationFacts {
	level_db: number;
	band_coherence: number;
	gate_coherence: number;
	lag_frames: number;
	trusted_frac: number;
	passed: boolean;
}

export interface SineLawRef {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	/** Unusable calls in a row now. */
	misses: number;
	/** Ticks a cap has bound this run. */
	capped_ticks: number;
	/** The live parameters' values in force. */
	live: Record<string, number>;
}

/** The `sine.state` result and event. */
export interface SineState {
	run_id: string | null;
	state: SineRunState;
	phase: SinePhase;
	title: string | null;
	source: DataSource | null;
	station: StationRef | null;
	grid: SineGrid | null;
	clock: ClockDomain | null;
	test: SineTestKind | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	level_db: number | null;
	rung: number | null;
	frequency_hz: number | null;
	direction: SineDirection | null;
	sweeps: { done: number; of: number | null } | null;
	held: boolean;
	/** Station time (holds excluded). */
	elapsed_s: number | null;
	at_level_s: number | null;
	remaining_s: number | null;
	alarm: boolean;
	notch: boolean;
	identification: SineIdentificationFacts | null;
	law: SineLawRef | null;
	feasibility: FeasibilityVerdict | null;
	end: SineEnd | null;
}

export interface SineLogResult {
	entries: LogEntry[];
	next: number;
}

export interface SineLogEvent {
	run_id: string;
	entries: LogEntry[];
}

// --- display -------------------------------------------------------------------------------

export type SineLiveQuantity =
	| 'control'
	| 'drive'
	| 'response'
	| 'phase'
	| 'thd'
	| 'time'
	| 'identification'
	| 'law'
	| 'readings';

export interface SineSubscribePlot {
	id: string;
	quantity: SineLiveQuantity;
	/** `response`, `phase` and `time` only. */
	channels?: ChannelSelector;
	/** `time` only. */
	points?: number;
}

export type SineSubscribeParams = { rate_hz?: number; plots: SineSubscribePlot[] } | { default: true };

export interface SineSubscribeResult {
	default: boolean;
	rate_hz: number;
	plots: SineSubscribePlot[] | null;
	quantities: SineLiveQuantity[];
}

export interface SineLiveChannel {
	index: number;
	description: string;
	role: Exclude<ChannelRole, 'drive'>;
	/** Tracked amplitude at the tone's frequency, peak, in the row's unit. */
	amplitude: number | null;
	/** Against the drive, degrees. */
	phase_deg: number | null;
	/** Broadband rms in the row's unit. */
	rms: number | null;
	unit: string;
	/**
	 * For a control row: true once its tracked amplitude has reached 95 % of the top of the
	 * profile's abort band at the present frequency and level, then true to the end of the run.
	 * False for every other row. It is not an input-range latch.
	 */
	overload: boolean;
}

export type SineAlarm =
	| { kind: 'band'; side: 'high' | 'low'; error_db: number }
	| { kind: 'overload'; channel: number };

export interface SineLawLive {
	/** The law's values on its last call, by declared key. */
	values: Record<string, number>;
	misses: number;
	capped_ticks: number;
}

/** The numbers every live frame carries, `sine.live` and `sine.data` alike. */
export interface SineLiveHead {
	run_id: string;
	source: DataSource;
	class: StationClass;
	state: 'running' | 'review' | 'stopping' | 'finished';
	phase: SinePhase;
	test: SineTestKind;
	mode: LadderMode;
	/** Station time of the tick these numbers are from, since engage, holds included. */
	t_s: number | null;
	/** Test time (holds excluded). */
	elapsed_s: number | null;
	frequency_hz: number | null;
	direction: SineDirection | null;
	/** Signed; 0 at a dwell or held. A linear sweep reports `sweep_rate_hz_per_s` instead. */
	sweep_rate_oct_per_min?: number | null;
	sweep_rate_hz_per_s?: number | null;
	sweeps: { done: number; of: number | null } | null;
	held: boolean;
	/** The level the drive is at now. */
	level_db: number | null;
	/** The scheduled level. */
	commanded_db: number | null;
	/** The profile at this frequency and level, control unit peak. */
	target: number | null;
	/** The composite control amplitude, control unit peak. */
	measured: number | null;
	/** 20 log10(measured / target). */
	error_db: number | null;
	drive_vpk: number | null;
	/** Null without COLA. */
	cola_vpk: number | null;
	notch: boolean;
	alarm: boolean;
	channels: SineLiveChannel[];
	alarms: SineAlarm[];
	law: SineLawLive | null;
}

/** The `sine.live` event: control, drive and run views plus the numbers behind them. */
export interface SineLive extends SineLiveHead {
	/** Control, drive, run readings, in that order. */
	views: [SpectrumView, SpectrumView, ReadingView];
}

/** One plot's data in a `sine.data` frame. */
export interface SineDataPlot {
	id: string;
	quantity: SineLiveQuantity;
	/** The resolved channels, in table order; null for the quantities without channels. */
	channels: number[] | null;
	views: View[];
}

/** The `sine.data` event: a subscribed connection's own frame. */
export interface SineData extends SineLiveHead {
	plots: SineDataPlot[];
}

// --- the law catalog and check (PROTOCOL.md, "Control Laws") ------------------------------

/** A Sine law's catalog entry: `world` is `sine` and only it carries `live`. */
export type SineLawEntry = LawEntry & { world: 'sine'; live: string[] };

export type SineCheckRequirement = 'within' | 'through' | 'within_away' | 'survive' | 'each_rung_within' | 'limit_held';

/** One simulated test article's result in a Sine law check. */
export interface SineLawCheckCase {
	name: string;
	plant: string;
	requirement: SineCheckRequirement;
	passed: boolean;
	detail: string;
	end: SineOutcome;
	trip: string | null;
	ticks: number;
	judged_ticks: number;
	worst_error_db: number | null;
	worst_error_hz: number | null;
	worst_away_error_db: number | null;
	/** Per level of the ladder: `[level_db, worst_error_db]`; null outside the ladder case. */
	rungs: [number, number | null][] | null;
	worst_limit_over_db: number | null;
	worst_release_overshoot_db: number | null;
	calls: number;
	misses: number;
	refused: number;
	capped_ticks: number;
	worst_call_ms: number | null;
	budget_ms: number;
	max_drive_vpk: number;
}

export interface SineLawsCheckResult {
	/** The law's entry after the check: a law file's `check` now holds the record. */
	law: LawEntry;
	report: { passed: boolean; recorded: boolean; tool_version: string; cases: SineLawCheckCase[] };
}
