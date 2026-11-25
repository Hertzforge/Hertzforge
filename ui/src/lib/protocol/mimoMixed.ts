/**
 * MIMO Mixed Mode on the wire: sine tones and narrowband random on a random floor, on several
 * shakers controlled together. Every `mimo_mixed.*` name and shape the screens and the mock use is
 * in this one file, as `schema/protocol.schema.json` and `schema/PROTOCOL.md` ("MIMO Mixed Mode")
 * state them.
 *
 * The configuration is the controller's own, in its serialized form, less its reference, with the
 * wire's own members (`title`, `operator`, `article`, `notes`, `specification`, `reference`,
 * `record`). A value stated per control is one value for every control or a list
 * with one entry per control, in the controls' order.
 */
import type { BandDb, DataSource, LadderMode, LogEntry, RandomRunState, ReportItem, RunRecordRef, StationClass, StationRef, View } from './types';
import type { MimoBuildBreakpointsParams, MimoImportParams, MimoReference, MimoReferenceResult } from './mimo';

/** The test type's id in `tests.list`, setup files and run folders. */
export const MIMO_MIXED_TEST_TYPE = 'mimo_mixed';

/** Event topics. */
export const MIMO_MIXED_TOPICS = {
	state: 'mimo_mixed.state',
	log: 'mimo_mixed.log',
	live: 'mimo_mixed.live',
	data: 'mimo_mixed.data'
} as const;

/** Every method the service serves, by its verb. */
export const MIMO_MIXED_METHODS = {
	configure: 'mimo_mixed.configure',
	config: 'mimo_mixed.config',
	arm: 'mimo_mixed.arm',
	start: 'mimo_mixed.start',
	proceed: 'mimo_mixed.proceed',
	override: 'mimo_mixed.override',
	level: 'mimo_mixed.level',
	mode: 'mimo_mixed.mode',
	hold: 'mimo_mixed.hold',
	resume: 'mimo_mixed.resume',
	part: 'mimo_mixed.part',
	stop: 'mimo_mixed.stop',
	abort: 'mimo_mixed.abort',
	reset: 'mimo_mixed.reset',
	clear: 'mimo_mixed.clear',
	state: 'mimo_mixed.state',
	log: 'mimo_mixed.log',
	subscribe: 'mimo_mixed.subscribe',
	/** MIMO Random's reference verbs under this module's names. */
	buildBreakpoints: 'mimo_mixed.reference.build_breakpoints',
	buildRigidBody: 'mimo_mixed.reference.build_rigid_body',
	validateReference: 'mimo_mixed.reference.validate',
	importReference: 'mimo_mixed.reference.import',
	/** The law catalog's, shared by every test type. */
	lawsList: 'laws.list',
	lawsImport: 'laws.import',
	lawsCheck: 'laws.check'
} as const;

/**
 * Names the module keeps for what it does not build: each is refused by its reason, whatever its
 * parameters. The shell offers none of them as a working control.
 */
export const MIMO_MIXED_NOT_BUILT = {
	'mimo_mixed.adjust': 'adjust_not_built',
	'mimo_mixed.exclude': 'exclude_not_built',
	'mimo_mixed.identification.save': 'identification_save_not_built',
	'mimo_mixed.identification.load': 'identification_load_not_built'
} as const;

export type MimoMixedNotBuiltReason = (typeof MIMO_MIXED_NOT_BUILT)[keyof typeof MIMO_MIXED_NOT_BUILT];

/** Which world `laws.list` is asked for, by role. */
export const MIMO_MIXED_LAW_WORLDS = { floor: 'mimo', tones: 'sine_mimo' } as const;

export const MIMO_MIXED_MAX_CONTROLS = 24;
export const MIMO_MIXED_MAX_DRIVES = 12;
export const MIMO_MIXED_MAX_LIMITS = 8;
export const MIMO_MIXED_MAX_TONES = 12;
export const MIMO_MIXED_MAX_BANDS = 12;

/** The controller's defaults, as the setup shows them. */
export const MIMO_MIXED_DEFAULTS = {
	floor_law: 'pinv_closed',
	tone_law: 'matrix',
	band_law: 'auto',
	ceiling_rule: 'common_scale',
	fill_rule: 'no_error',
	exclusion_hz: 2,
	tracking_hz: 0.5,
	floor_alarm_db: [3, 3] as BandDb,
	floor_abort_db: [6, 6] as BandDb,
	tone_alarm_db: [1.5, 1.5] as BandDb,
	tone_abort_db: [3, 3] as BandDb,
	band_alarm_db: [3, 3] as BandDb,
	band_abort_db: [6, 6] as BandDb,
	/** A drive's peak abort over its peak ceiling when `abort_vpk` is absent, dB. */
	abort_over_peak_db: 1,
	level_ramp_s: 0.5,
	part_ramp_s: 3,
	floor_settle_s: 30,
	soft_start_db: 9,
	soft_start_relax_db: 6,
	crest_factor: 3,
	control_tick_s: 0.01,
	/** A level or part ramp is at least two control ticks. */
	min_ramp_s: 0.02
} as const;

/** A drive's peak abort when `abort_vpk` is absent: its peak ceiling plus 1 dB. */
export function defaultAbortVpk(peakV: number): number {
	return peakV * 10 ** (MIMO_MIXED_DEFAULTS.abort_over_peak_db / 20);
}

// --- the three choices the service names itself ----------------------------------------------

/** How a narrowband is held: `auto` picks per band, a scalar servo, or one servo per control. */
export type MimoMixedBandLaw = 'auto' | 'scalar_servo' | 'per_control';
export const MIMO_MIXED_BAND_LAWS: readonly MimoMixedBandLaw[] = ['auto', 'scalar_servo', 'per_control'];

/** What happens when the tones ask more of a drive than its ceiling leaves them. */
export type MimoMixedCeilingRule = 'common_scale' | 'in_turn';
export const MIMO_MIXED_CEILING_RULES: readonly MimoMixedCeilingRule[] = ['common_scale', 'in_turn'];

/** What the floor's law is shown on lines a tone or a servoed band occupies. */
export type MimoMixedFillRule = 'no_error' | 'model_fill';
export const MIMO_MIXED_FILL_RULES: readonly MimoMixedFillRule[] = ['no_error', 'model_fill'];

// --- parts -------------------------------------------------------------------------------------

/** One value for every control, or one per control in the controls' order. */
export type PerControl<T> = T | T[];

/** A part's schedule: the controller's own form. */
export type MimoMixedSchedule =
	| { kind: 'fixed'; hz: number; for_s?: number; for_cycles?: number }
	| {
			kind: 'sweep';
			lo_hz: number;
			hi_hz: number;
			start_hz: number;
			direction?: 'up' | 'down';
			law: 'linear_hz_per_s' | 'log_oct_per_min';
			rate: number;
			target: { kind: 'sweeps'; n: number } | { kind: 'duration'; s: number } | { kind: 'whichever_first'; sweeps: number; duration_s: number };
	  }
	| { kind: 'follow'; of: string; ratio: number }
	| { kind: 'segments'; segments?: unknown[] };

/** A part's time window, parts seconds; no `off_s`: to the end of the run. */
export interface MimoMixedWindow {
	on_s?: number;
	off_s?: number | null;
}

/** A tone's full-level peak amplitude at a control. */
export type MimoMixedToneAmplitude = { kind: 'constant'; g_pk: number } | { kind: 'profile'; rows: { freq_hz: number; quantity: string; value: number; alarm_db?: BandDb | null; abort_db?: BandDb | null }[] };

/** A narrowband's full width: Hz, or a share of its center (a swept band widens as it rises). */
export type MimoMixedBandWidth = { kind: 'hz'; hz: number } | { kind: 'share'; share: number };

/**
 * What a narrowband does to the floor between its edges. `raise`: the floor's matrix lifted by
 * `db` at each control. `added`: a density added at each control, with one coherence between the
 * controls (0: independent). `matrix` is reserved and refused `band_matrix_not_built`.
 */
export type MimoMixedBandStatement = { kind: 'raise'; db: PerControl<number> } | { kind: 'added'; level_g2_hz: PerControl<number>; coherence?: number } | { kind: 'matrix'; values?: unknown };

export interface MimoMixedFloorBody {
	kind: 'floor';
	alarm_db?: PerControl<BandDb>;
	abort_db?: PerControl<BandDb>;
}

export interface MimoMixedToneBody {
	kind: 'tone';
	schedule: MimoMixedSchedule;
	amplitude: PerControl<MimoMixedToneAmplitude>;
	/** False: the tone law chooses the phases between the controls. */
	phases_stated?: boolean;
	/** One per control, degrees against the first control; read only when stated. */
	phase_deg?: number[];
	alarm_db?: PerControl<BandDb>;
	abort_db?: PerControl<BandDb>;
}

export interface MimoMixedBandBody {
	kind: 'narrowband';
	schedule: MimoMixedSchedule;
	width: MimoMixedBandWidth;
	statement: MimoMixedBandStatement;
	/** Absent: the floor's defaults, 3 and 6 dB. */
	alarm_db?: PerControl<BandDb>;
	abort_db?: PerControl<BandDb>;
}

export type MimoMixedPartBody = MimoMixedFloorBody | MimoMixedToneBody | MimoMixedBandBody;

export type MimoMixedPartKind = MimoMixedPartBody['kind'];

/** One part: exactly one floor, then at most 12 tones and 12 narrowbands. */
export interface MimoMixedPart<B extends MimoMixedPartBody = MimoMixedPartBody> {
	/** 1 to 32 letters, digits, `_` and `-`; unique in the list. */
	id: string;
	name?: string;
	/** Default true; the floor is always on. */
	enabled?: boolean;
	window?: MimoMixedWindow;
	body: B;
}

// --- configuration -------------------------------------------------------------------------------

/** A law by catalog id, with its parameter values by key. */
export interface MimoMixedLawPick {
	id: string;
	params?: Record<string, unknown>;
}

/** The five choices: two laws from the catalog, three the service names. */
export interface MimoMixedLawChoice {
	/** World `mimo`; default `pinv_closed`. */
	floor?: MimoMixedLawPick;
	/** World `sine_mimo`; default `matrix`. */
	tones?: MimoMixedLawPick;
	bands?: { id: MimoMixedBandLaw };
	ceiling_rule?: { id: MimoMixedCeilingRule };
	fill_rule?: { id: MimoMixedFillRule };
}

/** A drive output. */
export interface MimoMixedDrive {
	/** The drive row's channel index in the channel table. */
	channel_index: number;
	/** The random part's ceiling, V rms. */
	ceiling_vrms: number;
	/** The summed drive's peak ceiling, V: the floor and every tone share it. */
	peak_v: number;
	/** The realized rms that aborts the run; absent: the rms ceiling plus 1 dB. */
	abort_vrms?: number;
	/** The peak that aborts the run (`drive_vpk`), above `peak_v`; absent: `peak_v` plus 1 dB. */
	abort_vpk?: number;
	/** The highest rms the identification may use on this output, V. */
	pretest_max_vrms: number;
	shaker?: Record<string, unknown>;
	moving_mass?: Record<string, number>;
}

/** A monitored response held under a ceiling. */
export interface MimoMixedLimit {
	channel_index: number;
	/** The PSD ceiling at full level, one value or one per line. */
	ceiling_psd: number[];
	abort_margin_db: number;
}

export interface MimoMixedEstimation {
	/** The wire's: the grid is `fs_hz / n_pts` hertz between lines. */
	fs_hz: number;
	n_pts: number;
	frames_per_loop: number;
	memory_loops: number;
	window?: 'Hann' | 'Rectangular';
	overlap?: number;
}

export interface MimoMixedIdentification {
	start_vrms: number;
	below_reference_db: number;
	tolerance_db: number;
	max_steps: number;
	frames: number;
	noise_frames: number;
}

export interface MimoMixedLevelSchedule {
	mode: LadderMode;
	start_db: number;
	table_db: number[];
	/** One for every rung, or one value for all. */
	dwell_s: number[];
}

export interface MimoMixedConfig {
	title?: string;
	operator?: string;
	article?: string;
	notes?: string;
	specification?: { document: string; clause: string };
	/** Control channels, by table index. */
	controls: number[];
	drives: MimoMixedDrive[];
	limits?: MimoMixedLimit[];
	/** The floor's reference: MIMO Random's, inline or a reference file. */
	reference: MimoReference;
	parts: MimoMixedPart[];
	/** Hz each side of a tone that stands still; default 2. */
	exclusion_hz?: number;
	/** The tone tracker's bandwidth, Hz; default 0.5. */
	tracking_hz?: number;
	estimation: MimoMixedEstimation;
	identification?: MimoMixedIdentification;
	gates?: { coherence_threshold: number; condition_threshold: number };
	caps?: { up_db: number; down_db: number };
	protections?: { envelope_abort_lines: number; divergence_loops: number; divergence_floor_db: number; overload_fraction: number; settle_loops: number };
	schedule: MimoMixedLevelSchedule;
	full_level_duration_s?: number;
	/** A level change's ramp; default 0.5, at least two control ticks. */
	level_ramp_s?: number;
	shutdown_ramp_s?: number;
	/** One part's ramp in or out; default 3, at least two control ticks. */
	part_ramp_s?: number;
	floor_settle_s?: number;
	soft_start_db?: number;
	soft_start_relax_db?: number;
	/** `Hold` only: an updated model is refused `frf_update_not_built`. */
	frf_mode?: 'Hold';
	crest_factor?: number;
	drive_clip_sigma?: number;
	article_mass_lb?: number;
	law?: MimoMixedLawChoice;
	control_tick_s?: number;
	metadata?: { key: string; label?: string; value: string }[];
	seed?: number;
	record?: { streams: boolean };
}

export interface MimoMixedConfigureParams {
	config: MimoMixedConfig;
	dry_run?: boolean;
}

// --- refusals --------------------------------------------------------------------------------------

/** A configuration item's name: the controller's refusals, the table's and the laws'. */
export type MimoMixedItemReason =
	| 'no_floor'
	| 'more_than_one_floor'
	| 'unknown_part_kind'
	| 'part_out_of_band'
	| 'parts_too_close'
	| 'too_many_tones'
	| 'too_many_bands'
	| 'duplicate_part_id'
	| 'follow_unknown_part'
	| 'follow_self'
	| 'follow_loop'
	| 'follow_no_frequency'
	| 'segments_not_built'
	| 'bad_part'
	| 'bad_config'
	| 'wrong_control_count'
	| 'band_not_additive'
	| 'band_matrix_not_built'
	| 'law_refuses_tone'
	| 'budget_cannot_fit'
	| 'tone_model_not_finite'
	| 'frf_update_not_built'
	| 'unknown_band_law'
	| 'unknown_ceiling_rule'
	| 'unknown_fill_rule'
	| 'abort_below_ceiling'
	| 'ramp_too_short'
	| 'duplicate_drive'
	| 'channel_unknown'
	| 'reference_invalid'
	| 'law_unknown'
	| 'law_wrong_world'
	| 'param_unknown'
	| 'param_out_of_range'
	| 'out_of_range'
	| 'config_unreadable'
	| 'drive_channel_present';

/** One refusal or warning of the setup, by name, with where it is: a field path, a table row. */
export interface MimoMixedReportItem {
	reason: MimoMixedItemReason | ReportItem['reason'];
	message: string;
	field?: string;
	channel?: number | null;
	row?: number;
}

/** The verbs' refusals by name. */
export type MimoMixedRefusalReason =
	| 'not_configured'
	| 'wrong_state'
	| 'run_finished'
	| 'run_active'
	| 'shutting_down'
	| 'config_invalid'
	| 'config_unreadable'
	| 'table_invalid'
	| 'law_not_checked'
	| 'arm_refused'
	| 'record_unwritable'
	| 'hardware_busy'
	| 'gate_failed'
	| 'prediction_exceeded'
	| 'override_not_needed'
	| 'budget_cannot_fit'
	| 'tone_model_not_finite'
	| 'manual_mode_required'
	| 'not_a_rung'
	| 'part_refused'
	| 'bad_params'
	| MimoMixedNotBuiltReason;

/** `detail.why` of a `part_refused` refusal. */
export type MimoMixedPartRefusal = 'unknown_part' | 'floor' | 'band_folded' | string;

// --- what the service derives ----------------------------------------------------------------------

/** One role's law as the configuration resolves it. */
export interface MimoMixedLawRole {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	builtin: boolean;
	world: 'mimo' | 'sine_mimo';
	/** Whether it may drive hardware in its role now. */
	hardware: boolean;
	/** Why not, in words; null when it may. */
	not_cleared: string | null;
}

export interface MimoMixedDerived {
	grid: { fs_hz: number; n_pts: number; df_hz: number; lines: number };
	band_hz: [number, number];
	/** Each control's reference rms at full level. */
	reference_rms: number[];
	/** The reference in words. */
	reference_source: string;
	law: { floor: MimoMixedLawRole; tones: MimoMixedLawRole };
}

export interface MimoMixedConfigureResult {
	state: MimoMixedState;
	derived: MimoMixedDerived;
	warnings: MimoMixedReportItem[];
}

export interface MimoMixedDryRunResult {
	errors: MimoMixedReportItem[];
	warnings: MimoMixedReportItem[];
	derived: MimoMixedDerived | null;
	views: View[];
}

export interface MimoMixedConfigResult {
	run_id: string | null;
	config: MimoMixedConfig | null;
	derived: MimoMixedDerived | null;
	views: View[];
}

// --- running ---------------------------------------------------------------------------------------

export type MimoMixedLevelParams = { delta_db: number } | { to_db: number };

export interface MimoMixedModeParams {
	mode: LadderMode;
}

export interface MimoMixedPartParams {
	id: string;
	on: boolean;
}

/** The two review holds an override releases. */
export type MimoMixedOverrideReason = 'gate_failed' | 'prediction_exceeded';

export interface MimoMixedOverrideParams {
	/** The operator's words; not empty. */
	ack: string;
	/** When given, the hold in force, or the override is refused `override_not_needed`. */
	reason?: MimoMixedOverrideReason;
}

export type MimoMixedBuildBreakpointsParams = MimoBuildBreakpointsParams;
export type MimoMixedImportParams = MimoImportParams;
export type MimoMixedReferenceResult = MimoReferenceResult;

/** Random's states; `blocked` is never one of this module's. */
export type MimoMixedRunState = RandomRunState;

export type MimoMixedPhase = 'idle' | 'armed' | 'sysid' | 'coherence_gate' | 'gate_failed' | 'prediction_exceeded' | 'awaiting_review' | 'floor_only' | 'engaged' | 'full_level' | 'ramping_down' | 'stopped' | 'aborted';

export const MIMO_MIXED_PHASES: readonly MimoMixedPhase[] = ['idle', 'armed', 'sysid', 'coherence_gate', 'gate_failed', 'prediction_exceeded', 'awaiting_review', 'floor_only', 'engaged', 'full_level', 'ramping_down', 'stopped', 'aborted'];

export type MimoMixedOutcome = 'completed' | 'operator_stop' | 'operator_abort' | 'safe_stop' | 'aborted' | 'law_fault' | 'fault' | 'sysid_level_unreachable';

export const MIMO_MIXED_OUTCOMES: readonly MimoMixedOutcome[] = ['completed', 'operator_stop', 'operator_abort', 'safe_stop', 'aborted', 'law_fault', 'fault', 'sysid_level_unreachable'];

/** The controller's names for an end, as `end.name` carries them. */
export const MIMO_MIXED_END_NAMES = [
	'completed',
	'stopped',
	'operator_abort',
	'drive_vpk',
	'drive_vrms',
	'drive_not_finite',
	'random_underrun',
	'input_overload',
	'kinematic',
	'divergence',
	'floor_envelope',
	'floor_not_equalized',
	'tone_abort',
	'tone_abort_at_ceiling',
	'tone_unreadable',
	'band_abort',
	'limit_abort',
	'station_stop',
	'law_fault',
	'ident_unreachable',
	'run_error',
	'run_thread_failure'
] as const;

/** How a run ended: the controller's name for it, and the part, control or drive it names. */
export interface MimoMixedEnd {
	outcome: MimoMixedOutcome;
	/** The controller's name for the end; null when it gave none. */
	name: string | null;
	message: string;
	at_s: number | null;
	level_db: number | null;
	part: string | null;
	/** The control's channel, when the end names one. */
	control: number | null;
	/** The drive's channel, when the end names one. */
	drive: number | null;
	/** The ramp down's length; null for an end that did not ramp. */
	ramp_s: number | null;
}

/** One control's numbers on a part's row. */
export interface MimoMixedControlCell {
	channel: number;
	/** The floor's rms at the level, a tone's peak amplitude; null on a band's row. */
	target: number | null;
	measured: number | null;
	/** A band's added level against the stated. */
	error_db: number | null;
	alarm: boolean;
}

/** One part's row in the state and in every frame, in the parts' order. */
export interface MimoMixedPartRow {
	id: string;
	kind: MimoMixedPartKind;
	name: string;
	/** As configured, or as `mimo_mixed.part` last set it. */
	enabled: boolean;
	/** Playing now. */
	on: boolean;
	/** A tone's frequency or a band's center; null for the floor. */
	freq_hz: number | null;
	held: boolean;
	/** A tone held down by the shared ceiling, or a band by the drive ceilings or the change cap. */
	limiting: boolean;
	/** The ceiling rule's scale on a tone, or a band's ramp; null for the floor. */
	scale: number | null;
	/** True for a band folded into the floor's reference; null for the others. */
	folded: boolean | null;
	/** How the row's numbers read. */
	amplitude: 'rms' | 'peak' | 'db';
	unit: string;
	controls: MimoMixedControlCell[];
}

/** One drive's numbers now, volts. */
export interface MimoMixedDriveRow {
	channel: number;
	/** The random part's rms alone (the floor and the bands). */
	random_rms_v: number | null;
	/** The largest magnitude played in the last chunk. */
	peak_v: number | null;
	/** The random part's peak held over the last second. */
	random_peak_v: number | null;
	/** The tones' summed amplitude in force. */
	tone_sum_v: number | null;
	/** The peak ceiling. */
	ceiling_v: number | null;
	/** The peak the run aborts at (`drive_vpk`). */
	abort_vpk: number | null;
	headroom_v: number | null;
	headroom_db: number | null;
	/** The tones' common scale on this drive. */
	scale: number | null;
}

/** One limit channel: what it carries on its worst line over its ceiling, and that line. */
export interface MimoMixedLimitRow {
	channel: number;
	/** dB over the ceiling (negative: under); null before anything is read. */
	over_db: number | null;
	at_hz: number | null;
}

/** One override of a review hold: the client, the operator's words, the hold it released. */
export interface MimoMixedOverride {
	by: string;
	text: string;
	reason: MimoMixedOverrideReason;
}

export interface MimoMixedForecastPoint {
	/** The drive's channel. */
	drive: number;
	time_s: number | null;
	need_v: number | null;
	ceiling_v: number | null;
}

/** One drive's prediction at full level. */
export interface MimoMixedDrivePrediction {
	drive: number;
	random_rms_v: number | null;
	random_peak_v: number | null;
	random_peak_factor: number | null;
	tone_peak_sum_v: number | null;
	tone_rms_v: number | null;
	total_rms_v: number | null;
	peak_v: number | null;
	ceiling_vrms: number | null;
	ceiling_peak_v: number | null;
	within: boolean;
}

/** What the model predicts once identified (the review's), null before. */
export interface MimoMixedPrediction {
	drives: MimoMixedDrivePrediction[];
	controls: { control: number; random_rms: number | null; tone_rms: number | null; total_rms: number | null }[];
	/** The budget along the schedule: the moment nearest a ceiling, and the first that does not fit. */
	forecast: { step_s: number; worst: MimoMixedForecastPoint | null; cannot_fit: MimoMixedForecastPoint | null; refused: string | null };
	within: boolean;
}

export interface MimoMixedIdentificationView {
	level_db: number | null;
	steps: number;
	frames: number;
	lag_frames: number;
	/** Each control's mean coherence over the band. */
	band_coherence: (number | null)[];
	/** The gate. */
	gate_coherence: number | null;
	passed: boolean;
	/** The identification's drive per drive, V rms. */
	drive_vrms: (number | null)[];
}

export interface MimoMixedFloorCounts {
	equalized: boolean;
	loops: number;
	misses: number;
	underruns: number;
	dropped: number;
	clamped: number;
}

export interface MimoMixedGrid {
	fs_hz: number;
	n_pts: number;
	df_hz: number;
	lines: number;
	band_hz: [number, number];
	actual: boolean;
}

/** The `mimo_mixed.state` result and event. */
export interface MimoMixedState {
	run_id: string | null;
	state: MimoMixedRunState;
	phase: MimoMixedPhase;
	title: string | null;
	source: DataSource | null;
	station: StationRef | null;
	grid: MimoMixedGrid | null;
	clock: 'shared' | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	level_db: number | null;
	/** Station time since engage; null before. */
	t_s: number | null;
	/** Time at the present rung or at full level, from the start of its ramp, holds left out; null outside them. */
	at_level_s: number | null;
	/** Time left there: the rung's dwell in auto, or the full-level duration when set; null otherwise. */
	remaining_at_level_s: number | null;
	held: boolean;
	alarm: boolean;
	limiting: boolean;
	parts: MimoMixedPartRow[];
	drives: MimoMixedDriveRow[];
	scale: number | null;
	floor_error_db: (number | null)[];
	floor_equalized: boolean;
	floor: MimoMixedFloorCounts | null;
	limits: MimoMixedLimitRow[];
	identification: MimoMixedIdentificationView | null;
	prediction: MimoMixedPrediction | null;
	override: MimoMixedOverride | null;
	overrides: MimoMixedOverride[];
	law: { floor: MimoMixedLawRole; tones: MimoMixedLawRole } | null;
	end: MimoMixedEnd | null;
	record?: RunRecordRef;
}

export type MimoMixedLogEntry = LogEntry & { data?: Record<string, unknown> | null };

export interface MimoMixedLogResult {
	entries: MimoMixedLogEntry[];
	next: number;
}

export interface MimoMixedLogEvent {
	run_id: string;
	entries: MimoMixedLogEntry[];
}

// --- display ---------------------------------------------------------------------------------------

/**
 * What a plot can show. `control`: each control's spectrum with the tones in, against its
 * reference. `residual`: the floor alone, the tones taken out, with the reference and its lines.
 * `cross`: the residual's cross spectrum between each control and the next. `drive`: each drive's
 * floor autospectrum. `tone_amplitude` and `tone_phase`: each tone's history per control.
 * `band_level`: each band's added level against its statement per control.
 */
export type MimoMixedQuantity = 'control' | 'residual' | 'cross' | 'drive' | 'tone_amplitude' | 'tone_phase' | 'band_level';

export const MIMO_MIXED_QUANTITIES: readonly MimoMixedQuantity[] = ['control', 'residual', 'cross', 'drive', 'tone_amplitude', 'tone_phase', 'band_level'];

export interface MimoMixedSubscribePlot {
	id: string;
	quantity: MimoMixedQuantity;
	/** Control channels; absent or null: every control. */
	channels?: number[] | null;
	/** Part ids; absent or null: every part the quantity takes. */
	parts?: string[] | null;
}

export type MimoMixedSubscribeParams = { rate_hz?: number; plots: MimoMixedSubscribePlot[] } | { default: true };

export interface MimoMixedSubscribeResult {
	default: boolean;
	rate_hz: number;
	plots: Required<MimoMixedSubscribePlot>[] | null;
	quantities: MimoMixedQuantity[];
}

/** The live frame: the state's members of these names, the station's class and the floor loops run. */
export interface MimoMixedLiveHead {
	run_id: string;
	source: DataSource;
	class: StationClass;
	state: MimoMixedRunState;
	phase: MimoMixedPhase;
	mode: LadderMode | null;
	t_s: number | null;
	at_level_s: number | null;
	remaining_at_level_s: number | null;
	level_db: number | null;
	held: boolean;
	alarm: boolean;
	limiting: boolean;
	floor_error_db: (number | null)[];
	floor_equalized: boolean;
	scale: number | null;
	parts: MimoMixedPartRow[];
	drives: MimoMixedDriveRow[];
	loop: number;
}

export interface MimoMixedLive extends MimoMixedLiveHead {
	views: View[];
}

export interface MimoMixedDataPlot {
	id: string;
	quantity: MimoMixedQuantity;
	channels: number[] | null;
	parts: string[] | null;
	views: View[];
}

/** The `mimo_mixed.data` event: a subscribed connection's own frame. */
export interface MimoMixedData extends MimoMixedLiveHead {
	plots: MimoMixedDataPlot[];
}

// --- the laws ------------------------------------------------------------------------------------

export type MimoMixedRole = 'floor' | 'tones';

/** `laws.check` with a role: the law in that role with the other role's built-in. */
export interface MimoMixedLawsCheckParams {
	id?: string;
	path?: string;
	params?: Record<string, unknown> | null;
	mimo_mixed: MimoMixedRole;
}

/** The MIMO Mixed check's cases, in order; `swept_tone_through_mode` is informational. */
export const MIMO_MIXED_CHECK_CASES = [
	'floor_only',
	'fixed_tones_on_floor',
	'swept_tone_through_mode',
	'narrowband_fixed',
	'narrowband_swept',
	'level_ladder',
	'shared_ceiling',
	'more_drives_than_controls',
	'transport_delay',
	'limit_channel'
] as const;

export const MIMO_MIXED_CHECK_VERSION = 'mimo-mixed-check 2';

export interface MimoMixedCheckCase {
	name: string;
	case: (typeof MIMO_MIXED_CHECK_CASES)[number];
	role: MimoMixedRole;
	plant: string;
	requirement: string;
	/** False: run and reported, no part of the verdict. */
	required: boolean;
	passed: boolean;
	detail: unknown;
}

export interface MimoMixedLawsCheckResult {
	law: MimoMixedLawRole;
	report: { passed: boolean; recorded: boolean; role: MimoMixedRole; tool_version: string; cases: MimoMixedCheckCase[] };
}
