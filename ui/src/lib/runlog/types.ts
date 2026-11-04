/** The run log as the analysis service describes it (schema/PROTOCOL.md, Run log). */

export interface RunLogRow {
	id: number;
	kind: string;
	test: number | null;
	/** The event's date, YYYY-MM-DD. */
	date: string;
	/** The entry on one line, as written when a correction changed it. */
	line: string;
	/** What each correction changed, in words; struck through above them when any. */
	corrections: string[];
	/** When a late entry was entered (HH:MM:SS), or null. */
	late: string | null;
}

export interface RunLogStatus {
	test: number | null;
	title?: string;
	kind?: string;
	axis?: string | null;
	run?: number | null;
	level_db?: number | null;
	/** 'running', 'aborted', 'stopped', 'completed', or '' with no run yet. */
	outcome?: string;
	started?: string | null;
	stopped?: string | null;
}

export interface RunLogView {
	path: string;
	title: string;
	writable: boolean;
	read_only: string;
	mirror: string | null;
	mirror_problem: string;
	problems: string[];
	rows: RunLogRow[];
	tests: { number: number; title: string; kind: string; state: string }[];
	status: RunLogStatus;
	header: Record<string, unknown>;
	holds: string[];
	owed: { title: string; test: number | null; lines: number; owed: string[] }[];
	run: number | null;
	level: number | null;
	/** The readings forms the current test's runs take. */
	readings: string[];
}

export type BoxKind = 'text' | 'number' | 'whole' | 'numbers' | 'texts' | 'flag' | 'choice' | 'path' | 'entry' | 'notches' | 'files' | 'answers' | 'heading';

export interface FormField {
	field: string;
	label: string;
	box: BoxKind;
	choices?: string[];
	/** The text the box opens with; absent for a heading. */
	value?: string;
}

export interface FormSpec {
	name: string;
	title: string;
	fields: FormField[];
	required: string[];
	/** The label of the form's line of free text; '' for none. */
	text: string;
	note: string;
	with_time: boolean;
	/** Whether the form computes something as it is typed (`runlog.preview`). */
	live: boolean;
	/** When the form was opened (ISO), and that time as HH:MM:SS. */
	taken: string;
	time: string;
	preview: string;
	/** The entry a correction corrects. */
	target?: number;
}
