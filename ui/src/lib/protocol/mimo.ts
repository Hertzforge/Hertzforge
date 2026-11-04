/**
 * MIMO Random and the control-law catalog on the wire (schema/PROTOCOL.md, "MIMO Random" and
 * "Control Laws"). Matrices travel packed: one Hermitian matrix per line as its upper triangle,
 * row by row, lines in order, real and imaginary parts as separate arrays.
 */
import type {
	BandDb,
	DataSource,
	Grid,
	LadderMode,
	RandomRunState,
	ReportItem,
	RunEnd,
	Schedule,
	StationRef
} from './types';

/** Event topics of MIMO Random. */
export const MIMO_TOPICS = {
	status: 'mimo_random.status',
	validate: 'mimo_random.validate',
	data: 'mimo_random.data',
	report: 'mimo_random.report'
} as const;

// --- setup ---------------------------------------------------------------------------------

export interface MimoTransform {
	rows: number;
	cols: number;
	/** Row-major. */
	values: number[];
}

export interface MimoControlRow {
	channel: number;
	alarm_db?: BandDb;
	abort_db?: BandDb;
}

export interface MimoDriveRow {
	channel: number;
	ceiling_vrms: number;
	abort_vrms?: number | null;
}

export type MimoSpace =
	| { kind: 'physical' }
	| { kind: 'virtual'; dofs: string[]; response_transform: MimoTransform; drive_transform?: MimoTransform | null };

/** Inline for small references; a native reference file, by path and SHA-256, for large ones. */
export type MimoReference =
	| { kind: 'inline'; freqs_hz: number[]; n: number; re: number[]; im: number[] }
	| { kind: 'file'; path: string; sha256: string };

export interface MimoLawChoice {
	id: string;
	params?: Record<string, unknown>;
}

export interface MimoEstimation {
	fs_hz?: number;
	n_pts?: number;
	frames_per_loop?: number;
	window?: 'hann' | 'rectangular';
	overlap?: number;
	memory_loops?: number;
}

export interface MimoIdentification {
	level_db?: number;
	frames?: number;
	noise_frames?: number;
	coherence_floor?: number;
	condition_limit?: number;
	review?: boolean;
}

export interface MimoControl {
	frf?: 'hold' | 'update';
	level_ramp_s?: number;
	shutdown_ramp_s?: number;
	max_loops_out?: number;
}

export interface MimoSetup {
	title?: string;
	channels: { controls: MimoControlRow[]; drives: MimoDriveRow[] };
	space?: MimoSpace;
	reference: MimoReference;
	law?: MimoLawChoice;
	estimation?: MimoEstimation;
	identification?: MimoIdentification;
	control?: MimoControl;
	schedule: Schedule;
	backend?: { kind: 'sim' | 'ni' } | null;
}

export interface MimoSetupParams {
	setup: MimoSetup;
	dry_run?: boolean;
}

// --- validation ----------------------------------------------------------------------------

export type MimoShape = 'square' | 'more_controls_than_drives' | 'fewer_controls_than_drives';

export interface MimoTransformCheck {
	rows: number;
	cols: number;
	rank: number;
	/** Null when the matrix is rank-deficient. */
	condition: number | null;
}

export interface MimoValidation {
	ok: boolean;
	errors: ReportItem[];
	warnings: ReportItem[];
	shape: { controls: number; drives: number; space: 'physical' | 'virtual'; dofs: number | null; shape: MimoShape };
	transforms: { response: MimoTransformCheck | null; drive: MimoTransformCheck | null };
	reference: {
		source: 'inline' | 'file';
		lines: number;
		n: number;
		band_hz: [number, number] | null;
		min_eigenvalue_ratio: number | null;
		rank_min: number | null;
		rank_max: number | null;
		repaired_lines: number | null;
	} | null;
	feasibility: {
		channel: number;
		ceiling_vrms: number;
		verdict: 'pass' | 'advisory' | 'exceeded' | 'not_evaluated';
		predicted_vrms: number | null;
		message: string;
	}[];
	law: { id: string; name: string; version: string; sha256: string | null; admitted: boolean } | null;
}

export interface MimoSetupResult {
	state: MimoStatus;
	validation: MimoValidation;
}

export interface MimoValidateResult {
	validation: MimoValidation | null;
}

// --- running -------------------------------------------------------------------------------

export interface MimoOverrideParams {
	/** The operator's own words; kept with the run. */
	ack: string;
}

export interface MimoIterateParams {
	law?: MimoLawChoice | null;
}

export interface MimoLawRef {
	id: string;
	name: string;
	version: string;
	sha256: string | null;
}

export interface MimoPrediction {
	iteration: number;
	law: MimoLawRef;
	drive_rms_v: number[];
	ceiling_vrms: number[];
	error_db: number[];
	drives_pass: boolean;
}

export type MimoPhase =
	| 'idle'
	| 'armed'
	| 'sysid_noise'
	| 'sysid'
	| 'awaiting_review'
	| 'gate_failed'
	| 'engaged'
	| 'full_level'
	| 'ramping_down'
	| 'stopped'
	| 'aborted';

export interface MimoGates {
	coherence_min: number;
	coherence_floor: number;
	condition_max: number;
	condition_limit: number;
	passed: boolean;
}

export interface MimoShapeInfo {
	controls: number;
	drives: number;
	space: 'physical' | 'virtual';
	dofs: string[] | null;
	shape: MimoShape;
	/** The controlled quantities' names, in order: what a selector's index counts. */
	labels: string[];
	drive_labels: string[];
}

/** The `mimo_random.status` result and event. */
export interface MimoStatus {
	run_id: string | null;
	state: RandomRunState;
	phase: MimoPhase;
	title: string | null;
	source: DataSource | null;
	station: StationRef | null;
	grid: Grid | null;
	clock: 'shared' | 'independent' | 'unknown' | null;
	shape: MimoShapeInfo | null;
	mode: LadderMode | null;
	levels_db: number[] | null;
	level_db: number | null;
	elapsed_s: number | null;
	at_level_s: number | null;
	remaining_at_level_s: number | null;
	loop: number;
	alarm: boolean;
	control_error_db: number[];
	gates: MimoGates | null;
	caps: number[];
	drive_rms_v: number[];
	law: MimoLawRef | null;
	faults: { kind: string; message: string }[];
	override: { by: string; text: string } | null;
	prediction: MimoPrediction | null;
	end: RunEnd | null;
}

export interface MimoReport {
	run_id: string;
	title: string;
	source: DataSource;
	grid: Grid;
	shape: MimoShapeInfo;
	law: MimoLawRef;
	loops: number;
	controls: { label: string; rms: number | null; reference_rms: number | null; error_db: number | null }[];
	drives: { channel: number; label: string; rms_v: number | null; ceiling_vrms: number; min_cap: number | null }[];
	gates: MimoGates | null;
	override: { by: string; text: string } | null;
	end: RunEnd | null;
}

export interface MimoReportResult {
	report: MimoReport | null;
}

// --- the law catalog -----------------------------------------------------------------------

export type LawWorld = 'single' | 'mimo';

export type LawParam = { key: string; label: string; help: string } & (
	| { kind: 'number' | 'per_channel'; unit: string; min: number; max: number; step: number; default: number }
	| { kind: 'integer'; unit: string; min: number; max: number; step: number; default: number }
	| { kind: 'toggle'; default: boolean }
	| { kind: 'choice'; options: string[]; default: string }
	| { kind: 'curve'; unit: string; min: number; max: number; default: [number, number][] }
);

export type LawDiagnosticLayout = 'scalar' | 'per_line' | 'per_control_line' | 'per_drive_line';

export interface LawEntry {
	id: string;
	world: LawWorld;
	builtin: boolean;
	name: string;
	version: string;
	author: string;
	summary: string;
	sha256: string | null;
	check: {
		status: 'built_in' | 'unchecked' | 'passed' | 'failed' | 'unreadable';
		date: string | null;
		tool_version: string | null;
		cases: { name: string; passed: boolean; detail: string }[];
		/** Whether the law may drive hardware; any law runs on the simulator. */
		hardware: boolean;
		message: string;
	};
	params: LawParam[];
	diagnostics: { key: string; label: string; unit: string; help: string; layout: LawDiagnosticLayout | null }[];
	shapes: MimoShape[] | null;
	needs: ('last_drive' | 'response_drive_csd' | 'sysid_response' | 'sysid_drive' | 'sysid_noise')[] | null;
}

export interface LawsListParams {
	world?: LawWorld | null;
}

export interface LawsListResult {
	laws: LawEntry[];
	problems: { file: string; reason: string }[];
}

export interface LawsImportParams {
	/** The law file's full path on the computer that runs the test; files are never uploaded. */
	path: string;
}

export interface LawsImportResult {
	law: LawEntry;
}
