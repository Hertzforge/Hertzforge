/**
 * MIMO Mixed Mode on the wire: sine tones and narrowband random on a random floor, on several
 * shakers controlled together. Every `mimo_mixed.*` name and shape the screens and the mock use is
 * in this one file.
 *
 * Where the protocol leaves a name or a shape open, it is a sibling's: Mixed Mode's for the parts,
 * the schedules, the windows and the run verbs (`part`, `adjust`, `hold`, `resume`, the states and
 * phases); MIMO Random's for the channels, the reference, the estimation, the identification and
 * its gate (`override`, `exclude`), and the floor law (world `mimo`); MIMO Sine's for the tone law
 * (world `sine_mimo`). The parts' bodies follow the core's configuration: a value stated per
 * control is one number for every control or a list with one entry per control, in the controls'
 * order.
 */
import type {
	BandDb,
	ClockDomain,
	DataSource,
	LadderMode,
	LiveChannel,
	LogEntry,
	RandomRunState,
	RefusalReason,
	RunRecordRef,
	Schedule,
	StationClass,
	StationRef,
	View
} from './types';
import type {
	LawEntry,
	MimoBuildBreakpointsParams,
	MimoControlRow,
	MimoDriveRow,
	MimoEstimation,
	MimoGates,
	MimoIdentification,
	MimoImportParams,
	MimoLawChoice,
	MimoLimitRow,
	MimoReference,
	MimoReferenceResult
} from './mimo';
import type { MixedLogEntry, MixedPartKind, MixedPhase, MixedSchedule, MixedWindow } from './mixed';
import type { SineProfileRow } from './sine';

/** The test type's id in `tests.list`, setup files and run folders. */
export const MIMO_MIXED_TEST_TYPE = 'mimo_mixed';

/** Event topics. */
export const MIMO_MIXED_TOPICS = {
	state: 'mimo_mixed.state',
	log: 'mimo_mixed.log',
	live: 'mimo_mixed.live',
	data: 'mimo_mixed.data'
} as const;

/** Every method, by its verb. */
export const MIMO_MIXED_METHODS = {
	configure: 'mimo_mixed.configure',
	config: 'mimo_mixed.config',
	arm: 'mimo_mixed.arm',
	start: 'mimo_mixed.start',
	proceed: 'mimo_mixed.proceed',
	level: 'mimo_mixed.level',
	mode: 'mimo_mixed.mode',
	hold: 'mimo_mixed.hold',
	resume: 'mimo_mixed.resume',
	part: 'mimo_mixed.part',
	adjust: 'mimo_mixed.adjust',
	stop: 'mimo_mixed.stop',
	abort: 'mimo_mixed.abort',
	reset: 'mimo_mixed.reset',
	clear: 'mimo_mixed.clear',
	state: 'mimo_mixed.state',
	log: 'mimo_mixed.log',
	subscribe: 'mimo_mixed.subscribe',
	/** MIMO Random's reference verbs. */
	buildBreakpoints: 'mimo_mixed.reference.build_breakpoints',
	importReference: 'mimo_mixed.reference.import',
	/** MIMO Random's identification verbs: past a failed gate with words, or a control taken out. */
	override: 'mimo_mixed.override',
	exclude: 'mimo_mixed.exclude',
	/** The law catalog's, shared by every test type. */
	lawsList: 'laws.list',
	lawsImport: 'laws.import',
	lawsCheck: 'laws.check'
} as const;

/** Which world `laws.list` is asked for, by role. */
export const MIMO_MIXED_LAW_WORLDS = { floor: 'mimo', tones: 'sine_mimo' } as const;

export const MIMO_MIXED_MAX_CONTROLS = 16;
export const MIMO_MIXED_MAX_DRIVES = 8;
export const MIMO_MIXED_MAX_LIMITS = 8;
export const MIMO_MIXED_MAX_TONES = 12;
export const MIMO_MIXED_MAX_BANDS = 12;

/** The core's defaults, as the setup shows them. */
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
	control_tick_s: 0.01
} as const;

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

/** A tone's full-level peak amplitude at a control. */
export type MimoMixedToneAmplitude = { kind: 'constant'; g_pk: number } | { kind: 'profile'; rows: SineProfileRow[] };

/** A narrowband's full width: Hz, or a share of its center (a swept band widens as it rises). */
export type MimoMixedBandWidth = { kind: 'hz'; hz: number } | { kind: 'share'; share: number };

/**
 * What a narrowband does to the floor between its edges. `raise`: the floor's matrix lifted by
 * `db` at each control. `added`: a density added at each control, with one coherence between the
 * controls (0: independent). `matrix` is reserved and refused `band_matrix_not_built`.
 */
export type MimoMixedBandStatement =
	| { kind: 'raise'; db: PerControl<number> }
	| { kind: 'added'; level_g2_hz: PerControl<number>; coherence?: number }
	| { kind: 'matrix'; values: unknown };

export interface MimoMixedPartBase {
	/** 1 to 32 letters, digits, `_` and `-`; unique in the list. */
	id: string;
	name?: string;
	/** Default true; the floor is always on. */
	enabled?: boolean;
	window?: MixedWindow;
}

/** The floor: MIMO Random's reference, its bands per control. */
export interface MimoMixedFloorPart extends MimoMixedPartBase {
	kind: 'floor';
	alarm_db?: PerControl<BandDb>;
	abort_db?: PerControl<BandDb>;
}

export interface MimoMixedTonePart extends MimoMixedPartBase {
	kind: 'tone';
	schedule: MixedSchedule;
	amplitude: PerControl<MimoMixedToneAmplitude>;
	/** False: the tone law chooses the phases between the controls. */
	phases_stated: boolean;
	/** One per control, degrees against the first control; read only when stated. */
	phase_deg: number[];
	alarm_db?: PerControl<BandDb>;
	abort_db?: PerControl<BandDb>;
}

export interface MimoMixedBandPart extends MimoMixedPartBase {
	kind: 'narrowband';
	schedule: MixedSchedule;
	width: MimoMixedBandWidth;
	statement: MimoMixedBandStatement;
	/** Null or absent: the floor's. */
	alarm_db?: PerControl<BandDb> | null;
	abort_db?: PerControl<BandDb> | null;
}

export type MimoMixedPart = MimoMixedFloorPart | MimoMixedTonePart | MimoMixedBandPart;

// --- configuration -------------------------------------------------------------------------------

/** The five choices: two laws from the catalog, three the service names. */
export interface MimoMixedLawChoice {
	/** World `mimo`; default `pinv_closed`. */
	floor?: MimoLawChoice;
	/** World `sine_mimo`; default `matrix`. */
	tones?: MimoLawChoice;
	bands?: { id: MimoMixedBandLaw };
	ceiling_rule?: { id: MimoMixedCeilingRule };
	fill_rule?: { id: MimoMixedFillRule };
}

/** A drive: MIMO Random's row, its peak the summed ceiling the tones share with the floor. */
export type MimoMixedDriveRow = MimoDriveRow & { peak_v: number };

/** MIMO Random's run settings, with Mixed Mode's ramps and the floor's settle. Every field optional. */
export interface MimoMixedControl {
	/** `hold` only: an updated model is not built (`frf_update_not_built`). */
	frf?: 'hold';
	cap_up_db?: number;
	cap_down_db?: number;
	law_budget?: number;
	law_misses?: number;
	level_ramp_s?: number;
	shutdown_ramp_s?: number;
	/** One part's ramp in or out; default 3. */
	part_ramp_s?: number;
	/** The most the floor alone is given to equalize; default 30. */
	floor_settle_s?: number;
	soft_start_db?: number;
	soft_start_relax_db?: number;
	/** The random part's crest factor; default 3. */
	crest_factor?: number;
	/** Null: no clip. */
	drive_clip_sigma?: number | null;
	/** The tone tick, ms; default 10. */
	tick_ms?: number;
}

/** MIMO Random's protections. Every field optional. */
export interface MimoMixedProtections {
	envelope_abort_lines?: number;
	divergence_loops?: number;
	divergence_floor_db?: number;
	overload_fraction?: number;
	settle_loops?: number;
}

export interface MimoMixedConfig {
	title?: string;
	channels: { controls: MimoControlRow[]; drives: MimoMixedDriveRow[]; limits?: MimoLimitRow[]; article_mass_lb?: number };
	/** The floor's reference: MIMO Random's, inline or a reference file. */
	reference: MimoReference;
	/** Exactly one floor first, then at most 12 tones and 12 narrowbands. */
	parts: MimoMixedPart[];
	/** Hz each side of a tone that stands still; default 2. */
	exclusion_hz?: number;
	/** The tone tracker's bandwidth, Hz; default 0.5. */
	tracking_hz?: number;
	estimation?: MimoEstimation;
	identification?: MimoIdentification;
	/** Random's ladder; every part moves by the same dB. */
	schedule: Schedule;
	control?: MimoMixedControl;
	protections?: MimoMixedProtections;
	law?: MimoMixedLawChoice;
	operator?: string;
	article?: string;
	notes?: string;
	specification?: { document: string; clause: string } | null;
	record?: { streams: boolean } | null;
	metadata?: { key: string; label?: string; value: string }[];
}

export interface MimoMixedConfigureParams {
	config: MimoMixedConfig;
	dry_run?: boolean;
}

// --- refusals --------------------------------------------------------------------------------------

/** The verbs' refusals: Random's, Mixed Mode's two, and the configuration's. */
export type MimoMixedRefusalReason = RefusalReason | 'adjust_refused' | 'part_refused' | 'config_invalid' | 'bad_state' | 'station_error' | 'law_error';

/** A configuration refusal's name. */
export type MimoMixedItemReason =
	| 'no_floor'
	| 'unknown_part_kind'
	| 'part_out_of_band'
	| 'parts_too_close'
	| 'too_many_tones'
	| 'too_many_bands'
	| 'duplicate_part_id'
	| 'follow_unknown_part'
	| 'follow_self'
	| 'follow_loop'
	| 'schedule_unsupported'
	| 'bad_part'
	| 'bad_config'
	| 'wrong_control_count'
	| 'band_not_additive'
	| 'band_matrix_not_built'
	| 'law_refuses_tone'
	| 'budget_cannot_fit'
	| 'frf_update_not_built'
	| 'unknown_band_law'
	| 'unknown_ceiling_rule'
	| 'unknown_fill_rule'
	| 'reference_invalid'
	| 'law_unknown'
	| 'too_many_controls'
	| 'too_many_drives'
	| 'too_many_limits'
	| 'out_of_range'
	/** A warning: a tone sweeps faster than its tracker follows, and will abort. */
	| 'sweep_beyond_tracker';

/**
 * One refusal or warning of the setup, by name, with where it is: the part by its id, the control
 * and the drive by their position from 0, and a field path where there is one.
 */
export interface MimoMixedReportItem {
	reason: MimoMixedItemReason;
	message: string;
	field?: string | null;
	part?: string | null;
	control?: number | null;
	drive?: number | null;
	channel?: number | null;
	row?: number;
	/** The refusal's own numbers: `{expected, got}`, `{line, min_eigenvalue_rel}`, `{time_s, need_v, ceiling_v}`. */
	detail?: Record<string, unknown>;
}

/** `detail.why` of a `part_refused` refusal: Mixed Mode's. */
export type MimoMixedPartRefusal = 'unknown_part' | 'floor' | 'window_closed' | 'schedule_ended';

// --- what the service derives ----------------------------------------------------------------------

export interface MimoMixedGrid {
	fs_hz: number;
	n_pts: number;
	df_hz: number;
	lines: number;
	loop_s: number;
	tick_s: number;
	actual: boolean;
}

export interface MimoMixedDerivedPart {
	id: string;
	kind: MixedPartKind;
	name: string;
	/** Lowest and highest frequency or center of its whole motion; null for the floor. */
	freq_hz: [number, number] | null;
	/** A tone's largest amplitude per control, g peak; null otherwise. */
	peak_g: number[] | null;
	/** The band law a narrowband is held by (`auto` resolved); null otherwise. */
	band_law: Exclude<MimoMixedBandLaw, 'auto'> | null;
}

/** One drive's prediction at the start, at full level (the core's `DrivePrediction`). */
export interface MimoMixedDrivePrediction {
	drive: number;
	random_rms_v: number;
	random_peak_v: number;
	tone_peak_sum_v: number;
	tone_rms_v: number;
	total_rms_v: number;
	peak_v: number;
	ceiling_vrms: number;
	ceiling_peak_v: number;
	within: boolean;
}

/** One control's prediction at full level. */
export interface MimoMixedControlPrediction {
	control: number;
	random_rms: number;
	tone_rms: number;
	total_rms: number;
}

/** The budget along the schedule: each drive's worst moment, and the first that cannot fit. */
export interface MimoMixedForecast {
	step_s: number;
	/** Per drive, its worst moment: the peak needed against its peak ceiling. */
	drives: { drive: number; time_s: number; need_v: number; ceiling_v: number }[];
	/** The worst of them. */
	worst: { drive: number; time_s: number; need_v: number; ceiling_v: number } | null;
	/** The first moment and drive that cannot fit; null when every moment fits. */
	cannot_fit: { drive: number; time_s: number; need_v: number; ceiling_v: number } | null;
}

/** What the model predicts once identified (the review's), null before. */
export interface MimoMixedPrediction {
	drives: MimoMixedDrivePrediction[];
	controls: MimoMixedControlPrediction[];
	forecast: MimoMixedForecast;
	within: boolean;
}

export interface MimoMixedLawFacts {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	live: string[];
	hardware: boolean;
}

export interface MimoMixedDerived {
	band_hz: [number, number];
	grid: MimoMixedGrid;
	control_labels: string[];
	drive_labels: string[];
	parts: MimoMixedDerivedPart[];
	/** Station time the floor needs alone before the parts come in. */
	floor_equalize_s: number;
	identification_s: number;
	/** Parts time of the ladder; null in manual. */
	test_s: number | null;
	/** The tracker's delay at its bandwidth, s (about 0.63 / tracking_hz). */
	tracker_delay_s: number;
	law: { floor: MimoMixedLawFacts; tones: MimoMixedLawFacts; bands: MimoMixedBandLaw; ceiling_rule: MimoMixedCeilingRule; fill_rule: MimoMixedFillRule };
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

export interface MimoMixedArmParams {
	override_ack?: string;
}

export interface MimoMixedProceedParams {
	acknowledge?: string;
}

export type MimoMixedLevelParams = { delta_db: number } | { to_db: number };

export interface MimoMixedModeParams {
	mode: LadderMode;
}

export interface MimoMixedPartParams {
	id: string;
	on: boolean;
}

export interface MimoMixedAdjustParams {
	key: string;
	value: number;
}

export type MimoMixedOverrideReason = 'gate_failed' | 'prediction_exceeded';

export interface MimoMixedOverrideParams {
	ack: string;
	reason?: MimoMixedOverrideReason;
}

export interface MimoMixedExcludeParams {
	/** Control positions from 0. */
	controls: number[];
	ack: string;
}

export type MimoMixedBuildBreakpointsParams = MimoBuildBreakpointsParams;
export type MimoMixedImportParams = MimoImportParams;
export type MimoMixedReferenceResult = MimoReferenceResult;

export type MimoMixedRunState = RandomRunState;

/** Mixed Mode's phases: the floor alone (`floor_only`) before the parts come in. */
export type MimoMixedPhase = MixedPhase | 'gate_failed' | 'prediction_exceeded';

export type MimoMixedOutcome = 'completed' | 'operator_stop' | 'operator_abort' | 'safe_stop' | 'aborted' | 'held' | 'sysid_level_unreachable' | 'law_fault' | 'fault';

/** How a run ended: the trip by name, and the part, control or drive it names. */
export interface MimoMixedEnd {
	outcome: MimoMixedOutcome;
	message: string;
	/** The trip's name (`tone_abort`, `floor_abort`, `drive_vpk`, `tracker_lost` ...); null for an operator's stop. */
	name: string | null;
	part: string | null;
	control: number | null;
	drive: number | null;
	at_s: number | null;
	level_db: number | null;
	ramp_s: number | null;
}

/** One control's numbers on a part's row. */
export interface MimoMixedControlCell {
	target: number | null;
	measured: number | null;
	error_db: number | null;
	alarm: boolean;
}

/** One part's row in the state and in every frame, in the parts' order. */
export interface MimoMixedPartRow {
	id: string;
	kind: MixedPartKind;
	name: string;
	/** As configured, or as `mimo_mixed.part` last set it. */
	enabled: boolean;
	/** Playing now, ramps included. */
	on: boolean;
	/** Its ramp now, 0 to 1. */
	ramp: number;
	/** A tone's frequency or a narrowband's center now; null for the floor. */
	freq_hz: number | null;
	direction: 'up' | 'down' | null;
	sweeps: { done: number; of: number | null } | null;
	width_hz: number | null;
	/** Its schedule's clock stands: the run is held, or a tone waits for its tracker. */
	held: boolean;
	/** The ceiling rule is scaling this tone down. */
	limiting: boolean;
	/** The ceiling rule's scale on this tone, 0 to 1; null for the floor and a narrowband. */
	scale: number | null;
	/** `g` (a tone peak, the floor rms) or `g²/Hz` (a narrowband). */
	unit: string;
	/** One per control. */
	controls: MimoMixedControlCell[];
	/** A narrowband's band law in force; null otherwise. */
	band_law: Exclude<MimoMixedBandLaw, 'auto'> | null;
	follows: { of: string; ratio: number } | null;
}

/** One drive's numbers now. */
export interface MimoMixedDriveRowLive {
	channel: number;
	/** The random part's rms and its held peak, volts. */
	random_rms_v: number;
	random_peak_v: number;
	/** The sum of the tones' amplitudes on the drive, volts peak. */
	tone_sum_v: number;
	/** The drive's peak ceiling. */
	ceiling_v: number;
	/** What is left to the tones at this level, volts. */
	headroom_v: number;
}

export interface MimoMixedLawRef {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
	misses: number;
	capped: number;
	live: Record<string, number>;
}

export interface MimoMixedLaws {
	floor: MimoMixedLawRef;
	tones: MimoMixedLawRef;
	bands: MimoMixedBandLaw;
	ceiling_rule: MimoMixedCeilingRule;
	fill_rule: MimoMixedFillRule;
}

export interface MimoMixedShape {
	controls: number;
	drives: number;
	labels: string[];
	drive_labels: string[];
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
	clock: ClockDomain | null;
	shape: MimoMixedShape | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	level_db: number | null;
	full_level_s: number | null;
	t_s: number | null;
	elapsed_s: number | null;
	parts_s: number | null;
	at_level_s: number | null;
	remaining_at_level_s: number | null;
	held: boolean;
	alarm: boolean;
	limiting: boolean;
	/** The floor's error per control over its clean lines, dB. */
	floor_error_db: number[];
	floor_equalized: boolean;
	/** The common scale on the tones (`common_scale`), 1 when nothing binds; null under `in_turn` or before the tones. */
	scale: number | null;
	parts: MimoMixedPartRow[];
	drives: MimoMixedDriveRowLive[];
	/** The identification's gate, once identified. */
	gates: MimoGates | null;
	/** The model's prediction and the budget forecast, once identified. */
	prediction: MimoMixedPrediction | null;
	law: MimoMixedLaws | null;
	excluded: number[];
	override: { by: string; text: string; reason?: MimoMixedOverrideReason } | null;
	end: MimoMixedEnd | null;
	record?: RunRecordRef | null;
}

export type MimoMixedLogEntry = MixedLogEntry;

export interface MimoMixedLogResult {
	entries: (MimoMixedLogEntry | LogEntry)[];
	next: number;
}

export interface MimoMixedLogEvent {
	run_id: string;
	entries: (MimoMixedLogEntry | LogEntry)[];
}

// --- display ---------------------------------------------------------------------------------------

/**
 * What a plot can show. `control`: each control's spectrum with the tones in, against its
 * reference. `residual`: each control's autospectrum with the tones taken out, against the floor's
 * reference with the alarm and abort lines. `cross`: the chosen pairs' coherence and phase against
 * the reference's. `drive`: each drive's spectrum. `tone_amplitude` and `tone_phase`: each tone's
 * history per control. `band_level`: each narrowband's level history.
 */
export type MimoMixedQuantity =
	| 'control'
	| 'residual'
	| 'cross'
	| 'drive'
	| 'tone_amplitude'
	| 'tone_phase'
	| 'band_level'
	| 'identification'
	| 'coherence'
	| 'law'
	| 'readings';

export interface MimoMixedSubscribePlot {
	id: string;
	quantity: MimoMixedQuantity;
	/** `control`, `residual`, `cross` and `drive`: table indexes, or every one. */
	channels?: number[] | 'all' | 'control' | 'limited';
	/** `cross` only: the control pairs, positions from 0; default the first pairs among the chosen channels. */
	pairs?: [number, number][];
	/** `tone_amplitude`, `tone_phase` and `band_level`: ids or a set; default `all`. */
	parts?: string[] | 'all' | 'tones' | 'bands';
}

export type MimoMixedSubscribeParams = { rate_hz?: number; plots: MimoMixedSubscribePlot[] } | { default: true };

export interface MimoMixedSubscribeResult {
	default: boolean;
	rate_hz: number;
	plots: MimoMixedSubscribePlot[] | null;
	quantities: MimoMixedQuantity[];
}

export interface MimoMixedLiveHead {
	run_id: string;
	source: DataSource;
	class: StationClass;
	state: 'running' | 'review' | 'stopping' | 'finished';
	phase: MimoMixedPhase;
	mode: LadderMode;
	t_s: number | null;
	elapsed_s: number | null;
	parts_s: number | null;
	level_db: number | null;
	commanded_db: number | null;
	at_level_s: number | null;
	remaining_at_level_s: number | null;
	loop: number;
	held: boolean;
	alarm: boolean;
	limiting: boolean;
	floor_error_db: number[];
	floor_equalized: boolean;
	scale: number | null;
	parts: MimoMixedPartRow[];
	drives: MimoMixedDriveRowLive[];
	channels: LiveChannel[];
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

export type MimoMixedLawRole = 'floor' | 'tones';

/** `laws.check` with a role: the law in that role with the other role's default. */
export interface MimoMixedLawsCheckParams {
	id?: string;
	path?: string;
	params?: Record<string, unknown> | null;
	mimo_mixed: MimoMixedLawRole;
}

export interface MimoMixedLawsListResult {
	laws: LawEntry[];
	problems: { file: string; reason: string }[];
}
