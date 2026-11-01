/**
 * TypeScript types for protocol `sds/1`.
 *
 * Hand-written from `schema/protocol.schema.json` and `schema/PROTOCOL.md`. They describe messages
 * AFTER decoding: every `{"$array": id}` reference has been replaced by a typed array. When the
 * schema changes, change these in the same commit (a generator can replace this file later).
 */

export const PROTOCOL = 'sds/1';

export type ServiceName = 'analysis' | 'control';

// ---------------------------------------------------------------------------------------------
// Arrays

export type Dtype = 'f32' | 'f64' | 'i32' | 'u32' | 'u8';

export type NumericArray = Float32Array | Float64Array | Int32Array | Uint32Array | Uint8Array;

/** Entry in a binary frame header's `arrays` list. */
export interface ArrayEntry {
	id: string;
	dtype: Dtype;
	shape: number[];
	offset: number;
	nbytes: number;
}

// ---------------------------------------------------------------------------------------------
// Messages

export interface Hello {
	type: 'hello';
	protocol: string;
	service: ServiceName;
	version: string;
}

export interface Request<P = Record<string, unknown>> {
	type: 'request';
	id: number;
	method: string;
	params: P;
}

export type ErrorCode = 'bad_request' | 'unknown_method' | 'not_found' | 'refused' | 'internal';

export interface ProtocolErrorBody {
	code: ErrorCode;
	message: string;
	/** A stable name for the cause; every hardware and Random error carries one. */
	reason?: RefusalReason;
	/** The facts behind `reason` (PROTOCOL.md, "Refusals"). */
	detail?: Record<string, unknown>;
}

export interface ResponseOk<R = Record<string, unknown>> {
	type: 'response';
	id: number;
	result: R;
}

export interface ResponseErr {
	type: 'response';
	id: number;
	error: ProtocolErrorBody;
}

export type Response<R = Record<string, unknown>> = ResponseOk<R> | ResponseErr;

export interface Event<D = Record<string, unknown>> {
	type: 'event';
	topic: string;
	seq: number;
	data: D;
}

export type Message = Hello | Request | Response | Event;

// ---------------------------------------------------------------------------------------------
// Views

export type Scale = 'log' | 'linear';

export interface Axis {
	label: string;
	unit: string | null;
	scale: Scale;
}

export type Role = 'measured' | 'reference' | 'tolerance' | 'abort' | 'drive' | 'other';

export interface Series {
	name: string;
	role: Role;
	x: NumericArray;
	y: NumericArray;
	/**
	 * How the points mean to be drawn (default `line`). `steps`: each y is a density over its
	 * own bin, drawn flat across `edges[i]..edges[i+1]` so the area drawn is the area a PSD
	 * integrates to. `law`: a specification already resampled along its power law by the
	 * service, drawn as a line.
	 */
	shape?: SeriesShape;
	/** Bin edges for `steps`: one more than there are y values, ascending. */
	edges?: NumericArray;
}

export type SeriesShape = 'line' | 'steps' | 'law';

export interface SpectrumView {
	view: 'spectrum';
	title: string;
	x: Axis;
	y: Axis;
	series: Series[];
	link?: string;
}

export type Status = 'pass' | 'warn' | 'fail' | 'info';

export interface ReadingRow {
	label: string;
	value: number | string;
	unit?: string | null;
	status: Status;
}

export interface ReadingView {
	view: 'reading';
	title: string;
	rows: ReadingRow[];
}

export interface Mode {
	index: number;
	frequency_hz: number;
	damping?: number | null;
	/** f32 [N, 3], same node order as `nodes`. */
	shape: Float32Array;
}

export interface Shape3DView {
	view: 'shape3d';
	title: string;
	unit: string;
	/** f32 [N, 3]. */
	nodes: Float32Array;
	/** u32 [L, 2]. */
	lines?: Uint32Array;
	/** u32 [T, 3]. */
	triangles?: Uint32Array;
	modes: Mode[];
}

export type View = SpectrumView | ReadingView | Shape3DView;
export type ViewKind = View['view'];

// ---------------------------------------------------------------------------------------------
// Operations and datasets

export type Workspace = 'acquire' | 'analyze' | 'modal' | 'model' | 'specify' | 'simulate' | 'report';

export type DatasetKind = 'random_run' | 'modal_model';

export type ParamType = 'number' | 'integer' | 'enum' | 'boolean' | 'string';

export interface ParamOption {
	value: string | number;
	label: string;
}

export interface ParamDescriptor {
	name: string;
	label: string;
	type: ParamType;
	unit?: string | null;
	default?: unknown;
	min?: number;
	max?: number;
	step?: number;
	options?: ParamOption[];
	options_from?: 'channels';
}

export interface OperationDescriptor {
	id: string;
	title: string;
	workspace: Workspace;
	accepts: DatasetKind;
	summary?: string;
	params: ParamDescriptor[];
	outputs: ViewKind[];
}

export interface DatasetInfo {
	id: string;
	kind: DatasetKind;
	title: string;
	description?: string;
}

export interface Catalog {
	operations: OperationDescriptor[];
	datasets: DatasetInfo[];
}

export interface OpenedDataset {
	handle: string;
	kind: DatasetKind;
	title: string;
	summary: Record<string, unknown>;
	channels: ParamOption[];
}

export interface OpResult {
	views: View[];
}

// ---------------------------------------------------------------------------------------------
// Control service

export interface SimProfile {
	id: string;
	title: string;
	summary: string;
}

export type RunState = 'idle' | 'running' | 'stopped';

export interface SimStateResult {
	state: RunState;
	run_id: string | null;
}

export type DataSource = 'SIM' | 'HARDWARE';

export interface DisplayEvent {
	run_id: string;
	source: DataSource;
	state: RunState;
	elapsed_s: number;
	level_db: number;
	views: View[];
	/** The controller's own phase name (system identification, ladder, at level, ...). */
	phase?: string;
	/** How the run ended; present once `state` is `stopped`. */
	outcome?: RunOutcome | null;
	/** The controller's words for why the run ended. */
	reason?: string | null;
}

export type RunOutcome = 'completed' | 'operator_stop' | 'aborted' | 'held' | 'fault';

// ---------------------------------------------------------------------------------------------
// Hardware and Random control (PROTOCOL.md, "Hardware and Random Control")

/** Event topics of the hardware and Random control methods. */
export const CONTROL_TOPICS = {
	session: 'session.changed',
	station: 'station.changed',
	channels: 'channels.changed',
	state: 'random.state',
	live: 'random.live',
	log: 'random.log'
} as const;

export type RefusalReason =
	| 'bad_params'
	| 'not_controller'
	| 'controller_held'
	| 'run_active'
	| 'not_configured'
	| 'run_finished'
	| 'wrong_state'
	| 'manual_mode_required'
	| 'not_a_rung'
	| 'feasibility_exceeded'
	| 'override_not_needed'
	| 'arm_refused'
	| 'ni_driver_unavailable'
	| 'no_ni_devices'
	| 'scan_failed'
	| 'devices_absent'
	| 'mixed_simulated_and_real'
	| 'mixed_timing_families'
	| 'not_enough_channels'
	| 'hardware_busy'
	| 'stale_revision'
	| 'table_invalid'
	| 'config_invalid'
	| 'shaker_record_invalid';

export type ItemReason =
	// channel table
	| 'duplicate_index'
	| 'no_channel_enabled'
	| 'zero_sensitivity'
	| 'non_positive_range'
	| 'control_group_gap'
	| 'reference_without_control'
	| 'unsupported_schema'
	| 'channel_unbound'
	| 'physical_channel_unknown'
	| 'physical_channel_wrong_direction'
	| 'physical_channel_shared'
	| 'mixed_timing_families'
	| 'iepe_without_bias'
	| 'bias_on_single_ended'
	| 'reference_count_mismatch'
	| 'drive_channel_present'
	// profile
	| 'profile_empty'
	| 'first_row_not_anchor'
	| 'row_level_ambiguous'
	| 'frequency_not_positive'
	| 'frequency_not_ascending'
	| 'psd_negative'
	| 'alarm_wider_than_abort'
	// acquisition
	| 'n_pts_not_power_of_two'
	| 'band_above_grid'
	| 'too_few_lines'
	// schedule
	| 'levels_empty'
	| 'levels_not_ascending'
	| 'dwell_missing'
	// control and limits
	| 'no_control_channel'
	| 'weight_unknown_channel'
	| 'weights_sum_not_positive'
	| 'limit_unknown_channel'
	| 'limit_rows_not_ascending'
	| 'out_of_range';

/** One validation finding, located by whichever of `channel`, `row` and `field` applies. */
export interface ReportItem {
	reason: ItemReason;
	message: string;
	/** A channel table row's `index`; null for a finding about the whole table. */
	channel?: number | null;
	/** A breakpoint row, from 0. */
	row?: number;
	/** A configuration field path such as `control.damping`. */
	field?: string;
}

export interface ValidationReport {
	errors: ReportItem[];
	warnings: ReportItem[];
}

// --- controller and observers --------------------------------------------------------------

export type SessionRole = 'controller' | 'observer';

export interface ControllerSeat {
	name: string;
	/** False while the holder is gone and its seat is kept for `timeout_s`. */
	present: boolean;
}

export interface SessionState {
	role: SessionRole;
	controller: ControllerSeat | null;
	/** Seconds before an unattended live run is stopped, or null. */
	orphan_s: number | null;
	heartbeat_s: number;
	timeout_s: number;
}

export interface SessionClaimParams {
	name?: string;
	/** The lease of a seat this client held, to resume it. */
	lease?: string;
}

export interface SessionClaimResult {
	role: 'controller';
	lease: string;
	resumed: boolean;
	heartbeat_s: number;
	timeout_s: number;
}

export interface SessionReleaseResult {
	role: 'observer';
	orphan_s: number | null;
}

/** Each connection receives its own copy: `role` is the receiving connection's. */
export interface SessionChanged {
	role: SessionRole;
	controller: ControllerSeat | null;
	orphan_s: number | null;
}

// --- stations ------------------------------------------------------------------------------

export type StationKind = 'sim' | 'ni';
/** `sim`: the simulator's plant. `ni_simulated`: driver-simulated NI devices. `ni_real`: physical. */
export type StationClass = 'sim' | 'ni_simulated' | 'ni_real';
export type ClockDomain = 'shared' | 'independent' | 'unknown';

export interface DiscoveredChannel {
	id: string;
	label: string;
}

export interface NiDevice {
	id: string;
	product: string;
	serial: string;
	/** The driver's own statement that no physical device is behind this id. */
	simulated: boolean;
	parent: string | null;
	slot: number | null;
	/** Devices that can share one sample clock; a run's channels all sit in one family. */
	timing_family: string | null;
	inputs: DiscoveredChannel[];
	outputs: DiscoveredChannel[];
}

export interface SimStation {
	kind: 'sim';
	title: string;
	available: true;
	class: 'sim';
	inputs: number;
	outputs: number;
	clock: 'shared';
}

export interface NiStation {
	kind: 'ni';
	title: string;
	available: boolean;
	unavailable: { reason: 'ni_driver_unavailable' | 'no_ni_devices' | 'scan_failed'; message: string } | null;
	scanned_unix: number | null;
	devices: NiDevice[];
}

export interface StationSelected {
	kind: StationKind;
	devices: string[];
	class: StationClass;
	source: DataSource;
	inputs: number;
	outputs: number;
	timing_family: string | null;
}

export interface StationList {
	stations: [SimStation, NiStation];
	selected: StationSelected;
}

export type StationSelectParams = { kind: 'sim' } | { kind: 'ni'; devices: string[] };

// --- channel table -------------------------------------------------------------------------

export type ChannelRole = 'control' | 'monitor' | 'drive' | 'reference';

export interface PhysicalChannel {
	device: string;
	channel: string;
}

export interface ChannelAbortLimits {
	accel_g: number | null;
	velocity_ips: number | null;
	displacement_mil: number | null;
}

export interface ChannelRow {
	index: number;
	enabled: boolean;
	description: string;
	role: ChannelRole;
	physical: PhysicalChannel | null;
	/** IEPE is off unless a client turns it on for this row. */
	interface: 'single_ended' | 'iepe';
	iepe_bias_v: number | null;
	/** `g`, `in/s`, `mil`, `lbf`, `V`, or another unit's name. */
	units: string;
	sensitivity_mv_per_eu: number;
	range_max: number;
	polarity: 'positive' | 'negative';
	control_group: number | null;
	abort: ChannelAbortLimits;
}

export interface ChannelTable {
	revision: number;
	channels: ChannelRow[];
	report: ValidationReport;
}

export interface ChannelsSetParams {
	revision: number;
	channels: ChannelRow[];
	dry_run?: boolean;
}

export interface ChannelsSetResult {
	revision: number;
	report: ValidationReport;
}

// --- Random configuration ------------------------------------------------------------------

/** `[below, above]` in positive dB around the reference. */
export type BandDb = [number, number];

/** A PSD anchor (`psd_g2_hz`) or a slope from the row before (`slope_db_oct`), never both. */
export type Breakpoint = { freq_hz: number; alarm_db?: BandDb; abort_db?: BandDb } & (
	| { psd_g2_hz: number; slope_db_oct?: never }
	| { slope_db_oct: number; psd_g2_hz?: never }
);

export interface RandomProfile {
	breakpoints: Breakpoint[];
	alarm_db?: BandDb;
	abort_db?: BandDb;
}

export interface Acquisition {
	fs_hz: number;
	n_pts: number;
	n_avg: number;
}

export type LadderMode = 'auto' | 'manual';

export interface ScheduleLevel {
	db: number;
	/** Required on every level but the last. */
	dwell_s?: number;
}

export interface Schedule {
	mode: LadderMode;
	levels: ScheduleLevel[];
	full_level_s: number;
}

export type CompositeMode = 'weighted_avg' | 'min' | 'max';

export interface PretestParams {
	averages?: number;
	max_vrms?: number | null;
	min_vrms?: number | null;
	shaped?: boolean;
}

/** Every field optional; the defaults are in PROTOCOL.md. */
export interface ControlParams {
	composite?: CompositeMode;
	weights?: { channel: number; weight: number }[];
	control_dof?: number;
	damping?: number;
	max_lines_out?: number;
	max_loops_out?: number;
	max_sigma?: number | null;
	drive_ceiling_vrms?: number;
	drive_abort_vrms?: number | null;
	sysid_level_db?: number;
	coherence_floor?: number;
	review_after_sysid?: boolean;
	pretest?: PretestParams;
	soft_start_db?: number;
	soft_start_relax_db?: number;
	level_ramp_s?: number;
	shutdown_ramp_s?: number;
}

export interface LimitProfile {
	channel: number;
	kind: 'response' | 'force';
	rows: { freq_hz: number; ceiling: number }[];
}

export interface FeasibilityInputs {
	/** A shaker record in the controller's shaker-database form. */
	shaker: Record<string, unknown>;
	moving_mass_lb: {
		armature: number;
		slip_table: number;
		drive_adapter: number;
		fixturing: number;
		test_article: number;
	};
	crest_factor?: number;
}

export interface RandomConfig {
	title: string;
	profile: RandomProfile;
	acquisition: Acquisition;
	schedule: Schedule;
	control?: ControlParams;
	limits?: LimitProfile[];
	feasibility?: FeasibilityInputs | null;
}

export interface RandomConfigureParams {
	config: RandomConfig;
	dry_run?: boolean;
}

export interface Grid {
	fs_hz: number;
	n_pts: number;
	df_hz: number;
	/** Lines in the band. */
	lines: number;
	/** Time one control loop takes. */
	loop_s: number;
	/** True once the run has armed and the grid is the one the hardware runs at. */
	actual: boolean;
}

export interface Derived {
	grms_g: number;
	velocity_rms_ips: number;
	displacement_rms_mil: number;
	band_hz: [number, number];
	grid: Grid;
	control_channels: number[];
}

export interface RandomConfigureResult {
	state: RandomState;
	derived: Derived;
	warnings: ReportItem[];
}

export interface RandomDryRunResult {
	errors: ReportItem[];
	warnings: ReportItem[];
	derived: Derived | null;
	/** `[reference]`: the profile on the requested grid at 0 dB, stepped; empty with errors. */
	views: SpectrumView[];
}

/** `random.config`: what the present run was configured with; all null when unconfigured. */
export interface RandomConfigResult {
	run_id: string | null;
	config: RandomConfig | null;
	derived: Derived | null;
	views: SpectrumView[];
}

// --- running -------------------------------------------------------------------------------

export interface RandomArmParams {
	/** The operator's acknowledgement to arm past a feasibility exceedance; logged verbatim. */
	override_ack?: string;
}

export type RandomLevelParams = { delta_db: number } | { to_db: number };

export interface RandomLevelResult {
	level_db: number;
}

export interface RandomModeParams {
	mode: LadderMode;
}

export type RandomRunState = 'unconfigured' | 'configured' | 'blocked' | 'armed' | 'running' | 'review' | 'stopping' | 'finished';

export type RandomPhase =
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
	| 'coherence_hold';

export interface StationRef {
	kind: StationKind;
	class: StationClass;
	devices: { id: string; serial: string }[];
}

export type Trip = { at_level_db: number; at_loop: number } & (
	| { kind: 'envelope'; abort_lines: number; worst_db: number }
	| { kind: 'kinematic'; channel: number; axis: 'accel' | 'velocity' | 'displacement'; value: number; limit: number }
	| { kind: 'drive_vrms'; realized_vrms: number; limit_vrms: number }
	| { kind: 'divergence'; consecutive_loops: number; divergent_bins: number }
);

export type RandomOutcome = 'completed' | 'operator_stop' | 'operator_abort' | 'safe_stop' | 'aborted' | 'held' | 'fault';

export interface RunEnd {
	outcome: RandomOutcome;
	message: string;
	at_s: number | null;
	level_db: number | null;
	/** Present when `outcome` is `aborted`. */
	trip: Trip | null;
}

export interface FeasibilityVerdict {
	verdict: 'pass' | 'advisory' | 'exceeded' | 'not_evaluated';
	message: string;
}

/** The `random.state` result and event. */
export interface RandomState {
	run_id: string | null;
	state: RandomRunState;
	phase: RandomPhase;
	title: string | null;
	source: DataSource | null;
	station: StationRef | null;
	grid: Grid | null;
	clock: ClockDomain | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	/** The commanded level; null when nothing drives. */
	level_db: number | null;
	full_level_s: number | null;
	elapsed_s: number | null;
	at_level_s: number | null;
	remaining_at_level_s: number | null;
	alarm: boolean;
	feasibility: FeasibilityVerdict | null;
	end: RunEnd | null;
}

export interface LiveChannel {
	index: number;
	description: string;
	role: Exclude<ChannelRole, 'drive'>;
	/** rms over the band in the row's own unit. */
	rms: number | null;
	unit: string;
	/** Latched once the row reached 90 % of its range this run. */
	overload: boolean;
}

export type Alarm = { kind: 'envelope'; lines: number; worst_db: number } | { kind: 'overload'; channel: number };

/** The `random.live` event: control, drive and run views plus the numbers behind them. */
export interface RandomLive {
	run_id: string;
	source: DataSource;
	class: StationClass;
	state: 'running' | 'review' | 'stopping' | 'finished';
	phase: RandomPhase;
	mode: LadderMode;
	elapsed_s: number;
	level_db: number | null;
	at_level_s: number | null;
	remaining_at_level_s: number | null;
	loop: number;
	control_rms_g: number | null;
	reference_rms_g: number | null;
	drive_rms_v: number | null;
	lines: { in_band: number; alarm: number; abort: number };
	channels: LiveChannel[];
	alarms: Alarm[];
	/** Control spectrum, drive spectrum, run readings, in that order. */
	views: [SpectrumView, SpectrumView, ReadingView];
}

export type LogKind = 'state' | 'feasibility' | 'override' | 'station' | 'gate' | 'level' | 'alarm' | 'overload' | 'trip' | 'note';

export interface LogEntry {
	seq: number;
	at_unix: number;
	kind: LogKind;
	text: string;
}

export interface RandomLogResult {
	entries: LogEntry[];
	next: number;
}

export interface RandomLogEvent {
	run_id: string;
	entries: LogEntry[];
}
