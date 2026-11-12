/**
 * MIMO Shock on the wire (schema/PROTOCOL.md, "MIMO Shock"): one pulse at a time on several
 * drives at once, judged at several control channels, a classical pulse at every control
 * (`waveform`) or a Shock Response Spectrum at every control (`srs`). The run's law is chosen
 * from the control-law catalog's `shock_mimo` world. Where a part of the wire is classical shock's
 * or SRS shock's, the type is theirs.
 */
import type { ClockDomain, DataSource, ItemReason, LadderMode, LogEntry, ReportItem, StationRef, View } from './types';
import type { LawEntry } from './mimo';
import type { ShockGrid, ShockPulse, ShockRefusalReason, ShockStandardName, ShockSubscribeResult } from './shock';
import type { SrsBands, SrsSynthesis, SrsTarget } from './srs';

/** Event topics of MIMO Shock. */
export const MIMO_SHOCK_TOPICS = {
	state: 'mimo_shock.state',
	log: 'mimo_shock.log',
	live: 'mimo_shock.live',
	pulse: 'mimo_shock.pulse',
	data: 'mimo_shock.data'
} as const;

/** The law world MIMO Shock's laws are listed under. */
export const MIMO_SHOCK_WORLD = 'shock_mimo';

/** Refusals of the `mimo_shock.*` methods: classical shock's and the unreachable hold's. */
export type MimoShockRefusalReason = ShockRefusalReason | 'unreachable_target';

export type MimoShockTargetKind = 'waveform' | 'srs';

/** A catalog entry of the `shock_mimo` world: every law's entry, with the target kinds it accepts. */
export type MimoShockLawEntry = LawEntry & { targets?: MimoShockTargetKind[] };

// --- configuration -------------------------------------------------------------------------

export interface MimoShockDrive {
	channel: number;
	ceiling_vpk: number;
	abort_vpk: number;
}

export interface MimoShockLimit {
	channel: number;
	abort_peak: number;
}

export interface MimoShockChannels {
	controls: { channel: number }[];
	drives: MimoShockDrive[];
	limits?: MimoShockLimit[];
}

export interface MimoShockSrsControl {
	target: SrsTarget;
	synthesis?: SrsSynthesis;
	polarity?: 'positive' | 'negative';
}

export type MimoShockTarget =
	| { kind: 'waveform'; standard?: ShockStandardName; pulses: ShockPulse[] }
	| { kind: 'srs'; bands: SrsBands; tolerance_db?: [number, number]; controls: MimoShockSrsControl[] };

export interface MimoShockTiming {
	mode: 'free' | 'hold';
	window_s?: number;
	/** `hold`: one per control, seconds; null for `free`. */
	delays_s?: number[] | null;
}

export type MimoShockVerdictMode = 'all' | 'global';

export interface MimoShockAcquisition {
	fs_hz?: number;
	record_len?: number | null;
	band_hz?: [number, number] | null;
	lead_s?: number;
}

export interface MimoShockSchedule {
	mode: LadderMode;
	levels_db: number[];
	max_pulses_per_level?: number;
	full_level_pulses?: number;
}

export interface MimoShockSpacing {
	cooling_s?: number;
	settling_s?: number;
	compute_s?: number;
}

export interface MimoShockIdentification {
	drive_vrms?: number;
	frames?: number | null;
	coherence_floor?: number;
	review?: boolean;
}

export interface MimoShockControl {
	law?: { id: string; params?: Record<string, unknown> };
	law_budget?: number;
	law_misses?: number;
	max_pulses?: number;
	max_growth_db?: number;
	max_band_growth_db?: number;
	divergence_pulses?: number;
	divergence_drive_ratio?: number;
	first_drive_ratio?: number;
	/** Each control's abort margin, dB over its target's peak (at full level: absolute; at the level played: scaled); null: the default. */
	abort_db?: number | null;
	max_velocity_ips?: number | null;
	max_displacement_in?: number | null;
	/**
	 * The abort lines scaled to the level played also stop the run (default true). Off, a pulse over
	 * its scaled line is judged failing and flagged, and the run goes on; the absolute lines always hold.
	 */
	scaled_aborts?: boolean;
}

export interface MimoShockConfig {
	title: string;
	channels: MimoShockChannels;
	target: MimoShockTarget;
	timing?: MimoShockTiming;
	verdict?: MimoShockVerdictMode;
	acquisition?: MimoShockAcquisition;
	schedule: MimoShockSchedule;
	spacing?: MimoShockSpacing;
	identification?: MimoShockIdentification;
	control?: MimoShockControl;
	simulator?: { plant: string | null };
	capture?: { auto_start: boolean; run_name: string };
	metadata?: { key: string; label: string; value: string }[];
}

export interface MimoShockConfigureParams {
	config: MimoShockConfig;
	dry_run?: boolean;
}

export type MimoShockItemReason =
	| ItemReason
	| 'levels_empty'
	| 'levels_not_ascending'
	| 'band_above_grid'
	| 'record_too_long'
	| 'controls_out_of_range'
	| 'drives_out_of_range'
	| 'duplicate_channel'
	| 'control_unknown_channel'
	| 'drive_unknown_channel'
	| 'limit_unknown_channel'
	| 'targets_length_mismatch'
	| 'delays_length_mismatch'
	| 'target_invalid'
	| 'bands_empty'
	| 'verdict_needs_srs'
	| 'plant_unknown'
	| 'law_unknown'
	| 'law_wrong_world'
	| 'law_wrong_target'
	| 'law_param_unknown'
	| 'law_param_invalid';

export type MimoShockReportItem = Omit<ReportItem, 'reason'> & { reason: MimoShockItemReason };

/** The pulse grid: the rate, the record length and its lines. */
export type MimoShockGrid = ShockGrid & { lines?: number };

export interface MimoShockDerived {
	grid: MimoShockGrid;
	/** Each control's full-level target: its peak and, when given, its rms over the record. */
	controls: { channel: number; peak: number; rms?: number }[];
	spacing_s: number;
	call_budget_s: number;
}

export interface MimoShockConfigureResult {
	state: MimoShockState;
	derived: MimoShockDerived;
	warnings: MimoShockReportItem[];
}

export interface MimoShockDryRunResult {
	errors: MimoShockReportItem[];
	warnings: MimoShockReportItem[];
	derived: MimoShockDerived | null;
	views: View[];
}

export interface MimoShockConfigResult {
	run_id: string | null;
	config: MimoShockConfig | null;
	derived: MimoShockDerived | null;
	views: View[];
}

// --- the run -------------------------------------------------------------------------------

export type MimoShockRunState = 'unconfigured' | 'configured' | 'armed' | 'running' | 'review' | 'stopping' | 'finished';

export type MimoShockPhase = 'idle' | 'armed' | 'sysid' | 'waiting' | 'firing' | 'awaiting_review' | 'stopping' | 'stopped' | 'aborted' | 'held';

export type MimoShockOutcome = 'completed' | 'operator_stop' | 'operator_abort' | 'safe_stop' | 'aborted' | 'not_converged' | 'held' | 'law_fault' | 'fault';

export type MimoShockLawMiss =
	| 'error'
	| 'panicked'
	| 'over_budget'
	| 'wrong_length'
	| 'non_finite'
	| 'undeclared_value'
	| 'too_many_values'
	| 'peak_rise'
	| 'bin_rise'
	| 'predicted_peak'
	| 'first_drive'
	| 'target_not_accepted';

export interface MimoShockEnd {
	outcome: MimoShockOutcome;
	/** Why, by name: `divergence`, `control_abort_absolute`, `control_abort_scaled`, a law's miss, ... */
	reason: string | null;
	message: string;
	at_s: number | null;
	level_db: number | null;
	pulse: number | null;
}

export interface MimoShockIdentificationFacts {
	realizations: number;
	drive_vrms: number[];
	drive_vpk: number[];
	coherence_mean: number[];
	coverage: number[];
	trusted_lines: number;
	band_lines: number;
	condition_max: number;
	coherence_floor: number;
	passed: boolean;
}

export interface MimoShockReachable {
	channel: number;
	reachable: number;
	floor_db: number;
	reachable_ok: boolean;
	predicted_peak_ratio: number | null;
}

export interface MimoShockReview {
	controls: MimoShockReachable[];
	hold: { reason: 'unreachable_target'; message: string } | null;
	acknowledged: string | null;
}

export type MimoShockFailReason = 'tolerance' | 'timing' | 'polarity' | 'duration';

/** One control's judgement of one pulse. */
export interface MimoShockControlVerdict {
	channel: number;
	passed: boolean;
	reasons: MimoShockFailReason[];
	error_db: number;
	/** The worst signed error, dB: positive an over-test. */
	over_test_db: number;
	delay_s: number;
	peak: number;
	peak_ratio: number;
	timing_ok: boolean;
	polarity_ok: boolean;
	duration_ratio: number | null;
	duration_ok: boolean;
	energy_db: number;
	rms_duration_ratio: number;
	velocity_ips: number;
	displacement_pk_pk_in: number;
	predicted_peak_ratio: number | null;
	srs: { worst_err_db: number; worst_band_hz: number; bands: number; passed_bands: number } | null;
	/** The measured peak against the abort line scaled to the level played, dB: positive over it. */
	scaled_margin_db: number;
	/** The measured peak against the absolute (full-level) abort line, dB: positive over it. */
	absolute_margin_db: number;
}

export interface MimoShockStepNote {
	from_db: number;
	planned_db: number;
	taken_db: number;
	predicted_planned_db: number;
	predicted_taken_db: number;
	tolerance_db: number;
}

/** One fired pulse (`mimo_shock.pulses`, `mimo_shock.pulse`, the `mimo_shock.pulse` event). */
export interface MimoShockPulseRow {
	index: number;
	scheduled_db: number;
	level_db: number;
	passed: boolean;
	at_s: number;
	drives: { channel: number; vpk: number; limit_scale: number }[];
	controls: MimoShockControlVerdict[];
	over_test_db: number;
	global_srs_error_db: number | null;
	arrival_spread_s: number;
	warnings: string[];
	step_note: MimoShockStepNote | null;
	law: {
		used: boolean;
		miss: MimoShockLawMiss | null;
		call_ms: number;
		delays_s: number[] | null;
		values: { key: string; value: number }[];
	};
}

export interface MimoShockLawRef {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	params: Record<string, unknown>;
	misses: number;
}

export interface MimoShockPulseCounts {
	fired: number;
	passed: number;
	at_level: number;
	of_level: number;
}

/** The `mimo_shock.state` result and event. */
export interface MimoShockState {
	run_id: string | null;
	state: MimoShockRunState;
	phase: MimoShockPhase;
	title: string | null;
	source: DataSource | null;
	station: (StationRef & { plant?: string | null }) | null;
	grid: MimoShockGrid | null;
	clock: ClockDomain | null;
	target: MimoShockTargetKind | null;
	timing: 'free' | 'hold' | null;
	verdict: MimoShockVerdictMode | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	level_db: number | null;
	rung: number | null;
	pulses: MimoShockPulseCounts;
	next_pulse_s: number | null;
	elapsed_s: number | null;
	identification: MimoShockIdentificationFacts | null;
	review: MimoShockReview | null;
	last_pulse: MimoShockPulseRow | null;
	law: MimoShockLawRef | null;
	end: MimoShockEnd | null;
	/** Whether the abort lines scaled to the level stop the run (the configuration's). */
	scaled_aborts: boolean;
	/** Null while scaled aborts are on; with them off, a standing sentence saying so. */
	notice: string | null;
}

export interface MimoShockProceedParams {
	/** The operator's own words, needed past the unreachable hold. */
	acknowledge?: string;
}

export interface MimoShockLogResult {
	entries: LogEntry[];
	next: number;
}

export interface MimoShockLogEvent {
	run_id: string;
	entries: LogEntry[];
}

export interface MimoShockPulsesResult {
	pulses: MimoShockPulseRow[];
	next: number;
}

export interface MimoShockPulseResult {
	pulse: MimoShockPulseRow;
	views: View[];
}

export type MimoShockQuantity = 'pulse' | 'srs' | 'drives' | 'verdicts' | 'ladder' | 'identification' | 'review' | 'law' | 'readings' | 'pulses';

export interface MimoShockSubscribePlot {
	id: string;
	quantity: MimoShockQuantity;
	points?: number;
	channel?: number;
}

export type MimoShockSubscribeParams = { rate_hz?: number; plots: MimoShockSubscribePlot[] } | { default: true };

export type MimoShockSubscribeResult = Omit<ShockSubscribeResult, 'plots' | 'quantities'> & {
	plots: MimoShockSubscribePlot[];
	quantities: MimoShockQuantity[];
};

export interface MimoShockLive {
	run_id: string;
	source: DataSource;
	state: MimoShockRunState;
	phase: MimoShockPhase;
	mode: LadderMode;
	level_db: number | null;
	rung: number | null;
	pulses: MimoShockPulseCounts;
	next_pulse_s: number | null;
}

export interface MimoShockPulseEvent {
	run_id: string;
	pulse: MimoShockPulseRow;
	views: View[];
}

export interface MimoShockDataPlot {
	id: string;
	quantity: MimoShockQuantity;
	views: View[];
}

export interface MimoShockData extends MimoShockLive {
	plots: MimoShockDataPlot[];
}
