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

export type Role = 'measured' | 'reference' | 'tolerance' | 'abort' | 'limit' | 'drive' | 'other';

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
	/** Index of the series this one follows: a model's prediction of that measurement. */
	follows?: number;
}

export type SeriesShape = 'line' | 'steps' | 'law';

export interface SpectrumView {
	view: 'spectrum';
	title: string;
	x: Axis;
	y: Axis;
	series: Series[];
	link?: string;
	/** Spans of x shaded behind the curves: frames averaged from, or spans cut away. */
	regions?: Region[];
}

/** A span of x shaded behind a spectrum's curves. */
export interface Region {
	x0: number;
	x1: number;
	/** `frame`: a frame the spectra are averaged from; `cut`: a span left out; `span`: edges alone. */
	kind: 'frame' | 'cut' | 'span';
	label?: string;
	/** Present when the span is dragged: `op` runs with `params`, the new x0 as `low`, x1 as `high`. */
	drag?: { op: string; params: Record<string, unknown>; low: string; high?: string };
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
	/** f32 [N, 3], same node order as `nodes`; the real part of a complex shape. */
	shape: Float32Array;
	/** f32 [N, 3], the imaginary part of a complex shape; absent for a real one. */
	shape_imag?: Float32Array;
	/** Names a shape that is not a mode, in place of "Mode <index>". */
	label?: string;
}

/** Measured data played back on the mesh (schema/PROTOCOL.md, `shape3d` `motion`). */
export interface Motion {
	/** The abscissa: sample k is at start + k * step, in `unit`. */
	label: string;
	unit: string;
	start: number;
	step: number;
	quantity: string;
	value_unit: string;
	/** u32 [D]: the node of `nodes` each DOF moves. */
	node: Uint32Array;
	/** f32 [D, 3]: the unit direction each DOF moves in. */
	direction: Float32Array;
	/** f32 [D, S]: DOF d at sample k is values[d * S + k]. */
	values: Float32Array;
	/** The furthest any node gets, in value_unit. */
	peak: number;
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
	/** Measured data to play back, with `modes` empty. */
	motion?: Motion;
	/** The drawn nodes' ids, for the hover captions; absent on a view that is not a geometry's. */
	node_ids?: Uint32Array;
	/** The shape is read at a line of the plot it is shown with; that plot's cursor moves it. */
	cursor?: 'frequency';
	/** Static line work over the model in its own coordinates: DOF arrows, sensor bodies. */
	marks?: { nodes: Float32Array; lines: Uint32Array };
	/** A second set of shapes on the same nodes, for a matrix that selects this view. */
	overlays?: Mode[];
	/** f32 [R, C]: the angle that aligns overlay c with mode r. */
	overlay_phase?: Float32Array;
	/** Ties playback to a spectrum of the same result with this link, which marks its time. */
	link?: string;
	note?: string;
	/** What a click in the view does. */
	pick?: Pick3D;
}

/** A click in a `shape3d` view: the operation it runs, and what it sends (schema/PROTOCOL.md). */
export interface Pick3D {
	op: string;
	params: Record<string, unknown>;
	target: 'point' | 'nodes' | 'faces';
	hint: string;
	/** nodes: the node id of each pickable row, first rows first. */
	ids?: number[];
	/** nodes: how many make one; 0 takes any number, then Enter. */
	count?: number;
	/** nodes: the param the ids go in. */
	param?: string;
	/** nodes: how many of the last `ids` are left out, and drawn dim. */
	off?: number;
	/** faces: each triangle's surface, -1 for none. */
	faces?: Int32Array;
	/** faces: the surfaces in use now. */
	kept?: number[];
	out?: string;
	back?: string;
	/** point: [scale, offset] taking a clicked coordinate in the view's unit back to SI. */
	to_si?: [number, number];
}

/**
 * Many records of one quantity, each a polyline at its own depth (schema/PROTOCOL.md, `waterfall`).
 * Values are in display units and never logged; the shell takes log10 of a `log` axis.
 */
export interface WaterfallView {
	/** The view it opens in; a coherence opens on the map, seen from above. */
	facing?: 'perspective' | 'top' | 'front';
	view: 'waterfall';
	title: string;
	x: Axis;
	z: Axis;
	/** A stage of paths: every point at a depth of its own on this axis (the sine stage's time). */
	y?: Axis;
	/** f32 [P]: each point's depth on `y`. */
	y_values?: Float32Array;
	y_range?: [number, number];
	/** One label per record, front to back. */
	records: string[];
	/** u32 [R + 1]: record r is points offsets[r] .. offsets[r + 1]. */
	offsets: Uint32Array;
	/** f32 [P]. */
	x_values: Float32Array;
	/** f32 [P]; NaN is a gap. */
	z_values: Float32Array;
	/** Where something is drawn on x. */
	x_range: [number, number];
	/** A level range the data pins (a coherence's 0 to 1.05). */
	z_range?: [number, number];
	/** Which page of how many, of how many records. */
	page?: { number: number; count: number; records: number };
	/** The quantities the object holds, two or more; choosing one reruns with `quantity` set. */
	quantities?: { value: string; label: string; records: number }[];
	/** The value of the quantity drawn. */
	quantity?: string;
	note?: string;
	/** A stage: the depth axis's labels, front to back, in place of the records'. */
	stations?: string[];
	/** Each record's station; two records can share one. */
	depths?: number[];
	/** Each record's pen: 'measured' is colored by level, any other role in its color. */
	roles?: Role[];
	/** Limit bands; one with no `outer` runs to the stage's top or floor. */
	zones?: StageZone[];
	/** Lines past an abort limit: one rectangle each, from the limit to the top or floor. */
	exceed?: StageExceed[];
	/** One abscissa a station each: translucent walls from floor to top. */
	walls?: Float32Array[];
	/** The records are one surface, each on the same abscissa. */
	surface?: boolean;
	/** Spans of the abscissa across the whole stage, dragged as a spectrum's regions are. */
	regions?: Region[];
}

export interface StageZone {
	station: number;
	kind: 'warning' | 'abort';
	side: 'upper' | 'lower';
	x: Float32Array;
	/** On a stage of paths: the depth of each point, so the band follows the path. */
	y?: Float32Array;
	inner: Float32Array;
	outer?: Float32Array;
}

export interface StageExceed {
	station: number;
	side: 'upper' | 'lower';
	x0: Float32Array;
	x1: Float32Array;
	/** On a stage of paths: each rectangle's depths at its two edges. */
	y0?: Float32Array;
	y1?: Float32Array;
	z: Float32Array;
}

/** One value per row and column, such as a MAC (schema/PROTOCOL.md, `matrix`). */
export interface MatrixView {
	view: 'matrix';
	title: string;
	rows: { label: string; ticks: string[] };
	columns: { label: string; ticks: string[] };
	/** f32 [R, C], row-major: row r, column c is values[r * C + c]. */
	values: Float32Array;
	value_label: string;
	/** The color scale, pinned. */
	value_range: [number, number];
	/** Clicking cell (r, c) shows the result's shape3d view's mode r with overlay c. */
	selects?: 'shape3d';
	/** [row, column] pairs already kept as matched modes, marked in red. */
	matched?: [number, number][];
}

/** Rows of text under column headings, such as a mode table. */
export interface TableView {
	view: 'table';
	title: string;
	columns: string[];
	/** One cell per column, as text. */
	rows: string[][];
	/** Present when the cells are typed into: the operation that sets one, and each column's key (null: read only). */
	edit?: TableEdit;
}

/** How a table's cells are edited: `op` runs with `params` plus `row` (from 1), `column` and `value`. */
export interface TableEdit {
	op: string;
	params: Record<string, unknown>;
	columns: (string | null)[];
	/** What a column offers, by its key: a shortlist the cell can still be typed past. */
	choices?: Record<string, string[]>;
	/** How a column's cells are edited, by key: a flag is a check box, a choice a drop-down, a date a date field. */
	kinds?: Record<string, 'choice' | 'flag' | 'date' | 'unit' | 'number' | 'text'>;
	/** The row each shown row is in the object, from 1, when the table shows a pick of them. */
	rows?: number[];
}

/** A photo of the test: the file's bytes as read, png or jpeg, drawn under its name. */
export interface ImageView {
	view: 'image';
	title: string;
	format: 'png' | 'jpeg';
	/** u8 [N]: the encoded file. */
	bytes: Uint8Array;
}

export type View = SpectrumView | ReadingView | Shape3DView | WaterfallView | MatrixView | TableView | ImageView;
export type ViewKind = View['view'];

// ---------------------------------------------------------------------------------------------
// Operations and datasets

/**
 * The modules on the rail. The analysis is split by what is studied (signals, random, shock, sine,
 * modal) and what is built (geometry, model), each a lens on the one project, rather than by a
 * project type as Visual Dynamics did.
 */
export type Workspace = 'acquire' | 'simulate' | 'signals' | 'sysid' | 'random' | 'shock' | 'transient' | 'sine' | 'modal' | 'geometry' | 'model' | 'specify' | 'report';

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
	/** The dataset's channels, or the open project's objects of the kinds named (`objects:psd|frf`). */
	options_from?: 'channels' | `objects:${string}` | `verb:${string}`;
	/** Set from the result's view, not shown in the panel (a waterfall's quantity). */
	in_view?: boolean;
	/** The setting of the `source` object the param opens on, as `<group>.<field>`. */
	setting?: string;
}

/** One object of the open project (schema/PROTOCOL.md, Projects). */
export interface ProjectObject {
	name: string;
	/** The class in snake case: `frf`, `time_history`, `shape_set`, `geometry`. */
	kind: string;
	records?: number;
	source?: DataSource;
	/** False for an object imported without its units. */
	units_defined?: boolean;
	/** A PSD or a specification on proportional bands, which changes its icon. */
	banded?: boolean;
	/** A few words on what the object holds: its size, not its name. */
	holds?: string;
	/** Why the object does not fit what it answers to, and the records, modes or channels at fault. */
	issue?: { message: string; sub_items: number[] };
	/** Why a derived object no longer matches its source; absent while it does. */
	stale?: string;
	groups: string[];
	/** The core's processing verbs that apply to it, which `verb:` params are filled from. */
	verbs?: string[];
	/** A time history's analysis settings, each group present only when set; a report's features. */
	settings?: Record<string, Record<string, number | string | boolean>>;
}

/** The connection's own project, as `project.*` and project operations return it. */
export interface ProjectSummary {
	title: string;
	file: string | null;
	modified: boolean;
	objects: ProjectObject[];
	/** The geometry data is drawn on when nothing else names one. */
	active_geometry?: string | null;
	/** The object groups, as the tree brackets them: members, `Basis` or null, and a name or blank. */
	links?: { members: string[]; role: string | null; name: string }[];
	/** Objects held by reference to a file that was not found or has changed: not loaded, kept. */
	unresolved?: { name: string; kind: string; path: string; reason: 'not found' | 'changed' }[];
}

/** `project.actions`: the action table for a selection. */
export interface Actions {
	table: ActionTable;
}

/** One row of the action table: what a default draws, an operation, or an act on the selection itself. */
/** A lone shape set's views, shown one at a time. */
export type ShapeView = 'animate' | 'table' | 'mac' | 'values';

export interface ExportFormat {
	/** The `format` param's value. */
	value: string;
	label: string;
	/** The file's suffix with its dot; empty for a folder. */
	suffix: string;
}

export interface ActionRow {
	/** `show.*` for a default, an operation's id, or `self.*`. */
	id: string;
	verb: string;
	kind: 'default' | 'reading' | 'act' | 'self';
	ready: boolean;
	/** Why it cannot run now. */
	reason?: string;
	/** The modules that offer it; absent, every module. */
	modules?: Workspace[];
	fill?: Record<string, string>;
	/** In a combination column, the objects that complete the row. */
	candidates?: string[];
	/** An operation on the part of a geometry picked in the tree. */
	part?: true;
	/** Export: the formats this object can be written in. */
	formats?: ExportFormat[];
}

/** One column: the selection (`current`), or the selection with one more object of the last kind ('*' for any). */
export interface ActionColumn {
	kinds: string[];
	current?: true;
	rows: ActionRow[];
}

export interface ActionTable {
	columns: ActionColumn[];
}

/** `project.show`: what a selection in the tree shows, as Visual Dynamics' window shows it. */
export interface Shown {
	title: string;
	views: View[];
	/** What was left out or could not be drawn, in a sentence; empty when nothing. */
	note: string;
	/** Which of the bar's choices this selection takes. */
	offers?: {
		flat: boolean;
		complex: boolean;
		frequency: boolean;
		projection: boolean;
		/** A lone shape set: its three views and where its values are read. */
		shapes: boolean;
		values_dofs: string[];
		/** A stage on a geometry with DOFs to mark, or a channel table that places sensors. */
		arrows: boolean;
		sensors: boolean;
		/** A lone channel table: its control loops, and the views the picked one holds. */
		environments: string[];
		table_views: string[];
		/** A sine specification or level set: its tones, and with both the Level Error reading. */
		tones: string[];
		sine_readings: boolean;
	};
}

/** How the window reads data, kept across selections and launches (`project.show` `prefs`). */
export interface ShowPrefs {
	flat: boolean;
	log_frequency: boolean;
	component: 'magnitude' | 'real' | 'imag' | 'phase';
	/** The frequency an operating shape or envelope is read at; null for the loudest line. Not kept. */
	line?: number | null;
	/** Two shape sets on two geometries: animate the second projected onto the first's geometry. */
	projection: boolean;
	/** Where and in what form a lone shape set's values are read. */
	values_dofs: string;
	values_form: 'shapes' | 'response' | 'reference';
	/** DOF arrows and the channel table's sensors over every stage. */
	arrows: boolean;
	sensors: boolean;
	/** A channel table: which loop's control channels the Control column shows, and the table or one of its transformations. */
	environment: string | null;
	table_view: 'channels' | 'response' | 'reference';
	/** A sine specification or level set: the tone read, and the levels as curves or as error bars. */
	tone: string | null;
	sine_reading: 'curves' | 'bars';
	/** A MAC as 3-D bars (VD's default) or the flat grid. The shell's own; not sent. */
	bars: boolean;
	/** A picked MAC pair animated overlaid, the second mode phase-aligned. The shell's own; not sent. */
	overlay: boolean;
}

/** `project.children`: what an object opens into in the tree, by what it is. */
export type ProjectChildren =
	| {
			kind: 'geometry';
			parts: { id: 'nodes' | 'coordinate_systems'; label: string; count: number }[];
			families: { id: string; label: string; count: number; groups: { id: number; label: string; count: number }[] }[];
			solids: SolidRow[];
			mates: { id: number; label: string }[];
	  }
	| {
			kind: 'grid';
			rows: { label: string; dof: string; quantity: string; role?: 'reference' | 'response' | 'monitor' }[];
			columns: string[];
			/** A record index per row and column, -1 for a hole. */
			cells: number[][];
			/** Each record's icon key: a quantity, `response/reference`, or empty for the object's own icon. */
			icons: string[];
			/** The records whose units are not declared. */
			undefined: number[];
	  }
	| { kind: 'modes'; modes: { index: number; frequency: number; damping: number; description: string }[] }
	| { kind: 'channels'; channels: { dof: string; quantity: string }[] }
	| { kind: 'photos'; photos: string[] }
	| { kind: 'matches'; matches: { first: number; second: number; mac: number }[] }
	| { kind: 'none' };

export interface SolidRow {
	id: number;
	label: string;
	kind: string;
	children: SolidRow[];
}

/** One row of `project.units`: a channel, or the object itself, and its unit. */
export interface UnitsRow {
	key: string;
	label: string;
	quantity: string | null;
	unit: string | null;
	choices: string[];
}

/** `project.units`: the tables an object's units are declared in. */
export interface UnitsTables {
	name: string;
	tables: { side: 'object' | 'channels' | 'response' | 'reference'; rows: UnitsRow[] }[];
}

/** A mode of a fit: confirmed, or the preview. */
export interface FitMode {
	frequency: number;
	/** A fraction of critical. */
	damping: number;
	description?: string;
}

/** `fit.*`: the mode fit under way on the project. */
export interface FitStateView {
	frf: string;
	/** The shape set the fit edits or last published, or null. */
	shapes: string | null;
	pending: { frequency: number; damping: number; held: boolean };
	preview: FitMode | null;
	modes: FitMode[];
	view: SpectrumView;
	/** The confirmed modes' auto-MAC, the preview last; null for fewer than two. */
	mac?: MatrixView | null;
	/** The coherence the fit is weighted by, or null. */
	coherence?: string | null;
	/** Whether the view draws the residual's own CMIF. */
	residual?: boolean;
	/** The half-power reading of the residual CMIF peak under the cursor. */
	half_power?: HalfPowerReading;
}

/** Q read off the half-power width of the residual CMIF peak; null where it could not be read. */
export interface HalfPowerReading {
	frequency: number;
	low: number | null;
	high: number | null;
	q: number | null;
	damping: number | null;
	notes: string[];
}

/** `project.progress`: how far an import has read. */
export interface ProjectProgress {
	done: number;
	total: number;
	path: string;
}

export interface OperationDescriptor {
	id: string;
	title: string;
	/** The module that offers it, its home. */
	workspace: Workspace;
	/** The modules that offer it. */
	modules: Workspace[];
	summary?: string;
	params: ParamDescriptor[];
	outputs: ViewKind[];
}

export interface Catalog {
	operations: OperationDescriptor[];
}

export interface OpResult {
	views: View[];
	/** The project's summary, from an operation that runs on it. */
	project?: ProjectSummary;
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
	data: 'random.data',
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
	| 'shaker_record_invalid'
	| 'shutting_down';

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
	/** The control response identification steps up to, dB relative to the reference. */
	level_db?: number;
	max_vrms?: number | null;
	/** The drive identification starts from, V rms. */
	start_vrms?: number;
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
	| { kind: 'law_fault'; reason: LawFaultReason; consecutive: number }
);

/** What was wrong with the control law's last unusable output. */
export type LawFaultReason = 'error' | 'panicked' | 'over_budget' | 'wrong_length' | 'non_finite' | 'negative' | 'non_finite_level' | 'over_memory' | 'bad_diagnostics';

export type RandomOutcome = 'completed' | 'operator_stop' | 'operator_abort' | 'safe_stop' | 'aborted' | 'held' | 'sysid_level_unreachable' | 'fault' | 'law_fault';

export interface RunEnd {
	outcome: RandomOutcome;
	message: string;
	at_s: number | null;
	level_db: number | null;
	/** Present when `outcome` is `aborted` or `law_fault`. */
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
	/** Latched once the row reached 90 % of its range this run, or the station saw 98 % of it. */
	overload: boolean;
}

export type Alarm = { kind: 'envelope'; lines: number; worst_db: number } | { kind: 'overload'; channel: number };

/** The numbers every live frame carries, `random.live` and `random.data` alike. */
export interface RandomLiveHead {
	run_id: string;
	source: DataSource;
	class: StationClass;
	state: 'running' | 'review' | 'stopping' | 'finished';
	phase: RandomPhase;
	mode: LadderMode;
	elapsed_s: number;
	/** The level the drive is at now; null when nothing drives. */
	level_db: number | null;
	/** The level the run is headed for (the run state's `level_db`); differs from `level_db` while ramping. */
	commanded_db: number | null;
	at_level_s: number | null;
	remaining_at_level_s: number | null;
	/** The time the schedule holds the present level; null in manual and while nothing drives. */
	level_s: number | null;
	loop: number;
	control_rms_g: number | null;
	/** The target: the reference rms at the last control loop's level. */
	reference_rms_g: number | null;
	/** 20 log10(control / reference), from the same loop. */
	control_error_db: number | null;
	drive_rms_v: number | null;
	lines: { in_band: number; alarm: number; abort: number };
	channels: LiveChannel[];
	alarms: Alarm[];
}

/** The `random.live` event: control, drive and run views plus the numbers behind them. */
export interface RandomLive extends RandomLiveHead {
	/** Control spectrum, drive spectrum, run readings, in that order. */
	views: [SpectrumView, SpectrumView, ReadingView];
}

/** What a live plot can show (PROTOCOL.md, "Live Data Subscriptions"). */
export type LiveQuantity = 'control' | 'drive' | 'response' | 'time' | 'coherence' | 'readings';

/** Which channels a `response` or `time` plot shows: indexes, or a set the service resolves. */
export type ChannelSelector = number[] | 'all' | 'control' | 'limited';

export interface SubscribePlot {
	id: string;
	quantity: LiveQuantity;
	channels?: ChannelSelector;
	points?: number;
}

export type RandomSubscribeParams = { rate_hz?: number; plots: SubscribePlot[] } | { default: true };

export interface RandomSubscribeResult {
	default: boolean;
	rate_hz: number;
	plots: SubscribePlot[] | null;
	quantities: LiveQuantity[];
}

/** One plot's data in a `random.data` frame. */
export interface DataPlot {
	id: string;
	quantity: LiveQuantity;
	/** The resolved channels, in table order; null for the quantities without channels. */
	channels: number[] | null;
	views: View[];
}

/** The `random.data` event: a subscribed connection's own frame. */
export interface RandomData extends RandomLiveHead {
	plots: DataPlot[];
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

/** One field of a report block the editor's pane sets: the objects it may name. */
export interface ReportField {
	field: string;
	label: string;
	value: string;
	options: [string, string][];
}

/** `report.editor`: the edit page and the pane for the selection. */
export interface ReportEditor {
	/** The edit page; left out when only the pane was asked for. */
	html?: string;
	selected: number | null;
	title: string;
	marking: string;
	marking_color: 'ink' | 'red';
	blocks: number;
	/** The Insert menu, `[kind, label]`. */
	insert: [string, string][];
	/** `[label, caption]` of each numbered figure and table, for `{{figure:<caption>}}`; left out with `html`. */
	references?: [string, string][];
	block: {
		index: number;
		kind: string;
		mode: string;
		fields: ReportField[];
		text?: string;
		caption?: string;
		special?: string | null;
	} | null;
	project?: ProjectSummary;
}
