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
