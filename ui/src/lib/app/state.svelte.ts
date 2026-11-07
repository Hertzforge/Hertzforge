/**
 * Shell state: the two service clients, the catalog, and each live workspace's working state.
 * Workspace components mount and unmount as the user moves; their state lives here so nothing
 * is lost on the way back.
 */

import { DisplayState } from '$lib/acquire/display/display.svelte';
import { pairsText, togglePick } from '$lib/viz/macBars';
import { chooseFiles, chooseObjectFile, chooseSavePath, hasNativeFiles, pastedFiles } from '$lib/project/files';
import { EMPTY, clickObject, clickPart, clickRecord, pruned, renamed, type Modifiers, type Selection } from '$lib/project/selection';
import { AcquireState } from '$lib/acquire/store.svelte';
import { liveTestOf, TestTypeView, type LiveTest, type TestTypeId } from '$lib/acquire/testTypes.svelte';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { createControlServer } from '$lib/protocol/mock/control';
import { openerMock, shareMock } from '$lib/protocol/mock/shared';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ActionColumn, ActionRow, Actions, ExportFormat, ShapeView, ProjectChildren, ShowPrefs, Shown,
	Catalog,
	DisplayEvent,
	OperationDescriptor,
	OpResult,
	ParamDescriptor,
	FitStateView,
	ProjectProgress,
	UnitsTables,
	ProjectSummary,
	ReportEditor,
	RunState,
	SimProfile,
	SimStateResult,
	TableEdit,
	View,
	Workspace, MatrixView, Pick3D } from '$lib/protocol/types';
import { defaultValues, hasObjectsFor, objectKinds, objectVerb, reconcile, rescaled, validateAll, withDisplayLengths, withObjects, withSettings, type ParamValues } from '$lib/forms/params';
import { baseName, isProjectFile } from '$lib/project/files';
import { RunLogState } from '$lib/runlog/store.svelte';
import { theme } from '$lib/theme/theme.svelte';
import { readConfig, type ShellConfig } from './config';
import { isWorkspace, offeredIn, STUDIES } from './workspaces';

function message(err: unknown): string {
	if (err instanceof RequestError) return err.message;
	if (err instanceof Error) return err.message;
	return String(err);
}

// ---------------------------------------------------------------------------------------------

class AnalyzeState {
	opId = $state<string | null>(null);
	/** Form values per operation id, kept while the project stays open. */
	values = $state<Record<string, ParamValues>>({});
	triedRun = $state(false);
	running = $state(false);
	runError = $state<string | null>(null);
	/** What the tree's selection shows, in place of a result run by hand; null with nothing selected. */
	shown = $state.raw<Shown | null>(null);
	/** The bar's choices, remembered across launches as VD remembers 3D. */
	prefs = $state<ShowPrefs>(readPrefs());
	/** The frequency the stage's shape is read at, from the plot's cursor; null for the loudest line. */
	line = $state<number | null>(null);
	private lineTimer: ReturnType<typeof setTimeout> | null = null;
	private lineFor = '';
	/** The MAC pair the selection's stage animates, and every picked square; Add Matches keeps the picks. */
	pair = $state<{ row: number; column: number } | null>(null);
	picks = $state<{ row: number; column: number }[]>([]);
	showError = $state<string | null>(null);
	/** A reading run on the selection, drawn in place of what the selection shows until Back. */
	reading = $state.raw<{ title: string; views: View[] } | null>(null);
	/** What the selection can take, as the service last answered; null while nothing is selected. */
	actions = $state.raw<Actions | null>(null);
	/** The operation's form is open in the action column, in place of the table. */
	formOpen = $state(false);
	/** The table's row the open form came from; null when it came from elsewhere. */
	formRow = $state<ActionRow | null>(null);
	/** Which of a lone shape set's views a verb asked for; null, the one selecting it shows. */
	shapeView = $state<ShapeView | null>(null);
	/** The reading drawn in place of what the selection shows, by its row's id. */
	readingId = $state<string | null>(null);
	/** A combination's row whose partner is being picked in the tree. */
	picking = $state<{ verb: string; kind: string; row: ActionRow } | null>(null);
	/** The row to run once the object it needed has joined the selection. */
	private joined: string | null = null;
	/** The fit about to begin by hand, when a coherence could weight it. */
	fitWeighing = $state(false);
	private showSeq = 0;
	private actionSeq = 0;

	constructor(private readonly app: AppState) {}

	/** The module's own tools and the ones every module offers, of those with something to run on. */
	toolsFor(workspace: Workspace): OperationDescriptor[] {
		return this.tools.filter((o) => offeredIn(o, workspace));
	}

	/** Tools follow the data: only the operations with an object in the project to run on. */
	get tools(): OperationDescriptor[] {
		if (!this.onProject) return [];
		return (this.app.catalog?.operations ?? []).filter((o) => hasObjectsFor(this.paramsOf(o)));
	}

	get op(): OperationDescriptor | null {
		return this.tools.find((o) => o.id === this.opId) ?? null;
	}

	/** Whether there is a project to work on. */
	get onProject(): boolean {
		return this.app.project.summary !== null;
	}

	/** An operation's params as the form shows them: object params list the project's objects. */
	paramsOf(op: OperationDescriptor): ParamDescriptor[] {
		const params = withObjects(op.params, this.app.project.summary?.objects ?? []);
		const length = this.app.units.length;
		if (!length || !op.params.some((p) => p.length)) return params;
		return withDisplayLengths(params, this.undefinedGeometry(op) ? null : length.unit);
	}

	/** Whether the geometry an operation's form names has no units, its numbers its own. */
	private undefinedGeometry(op: OperationDescriptor): boolean {
		const name = this.values[op.id]?.geometry;
		return this.app.project.summary?.objects.find((o) => o.name === name)?.units_defined === false;
	}

	/** The display system changed: each form holds the same lengths, in the new unit. */
	lengthsChanged(ratio: number): void {
		const next = { ...this.values };
		for (const op of this.app.catalog?.operations ?? []) {
			if (next[op.id] && op.params.some((p) => p.length) && !this.undefinedGeometry(op)) next[op.id] = rescaled(op.params, next[op.id], ratio);
		}
		this.values = next;
	}

	get params(): ParamDescriptor[] {
		return this.op ? this.paramsOf(this.op) : [];
	}

	/** What the form asks: not the objects the selection already filled, and of a format only those the object can be written in. */
	get formParams(): ParamDescriptor[] {
		const row = this.formRow;
		return this.params
			.filter((p) => !p.in_view && !(p.options_from && row?.fill && p.name in row.fill))
			.map((p) => (p.name === 'format' && row?.formats ? { ...p, options: row.formats.map(({ value, label }) => ({ value, label })) } : p));
	}

	get currentValues(): ParamValues {
		return (this.opId && this.values[this.opId]) || {};
	}

	get errors(): Record<string, string> {
		const op = this.op;
		if (!op) return {};
		return validateAll(this.paramsOf(op), this.currentValues);
	}

	/** A project was opened: its tools take its objects. */
	selectProject(): void {
		this.projectChanged();
	}

	/** Per operation, the source and its settings its form was last filled from. */
	private filledFrom: Record<string, string> = {};

	/**
	 * Open the current form on its source object's settings: each `setting` param takes the value
	 * the object holds. Done again only when the source, or its settings, change, so a value typed
	 * after it stays.
	 */
	followSource(): void {
		const op = this.op;
		if (!op || !this.onProject || !op.params.some((p) => p.setting)) return;
		const values = this.currentValues;
		const source = this.app.project.summary?.objects.find((o) => o.name === values.source);
		const key = `${values.source}\u0000${JSON.stringify(source?.settings ?? {})}`;
		if (this.filledFrom[op.id] === key) return;
		this.filledFrom[op.id] = key;
		this.values = { ...this.values, [op.id]: withSettings(this.paramsOf(op), values, source) };
	}

	/** The project's objects changed: every form's object choices follow. */
	projectChanged(): void {
		if (!this.onProject) return;
		const next = { ...this.values };
		for (const op of this.tools) next[op.id] = reconcile(this.paramsOf(op), this.values[op.id]);
		this.values = next;
	}

	/** The tree's selection changed: ask the service what it shows, dropping answers overtaken by later picks. */
	async showSelection(): Promise<void> {
		const selection = this.app.project.selection;
		const seq = ++this.showSeq;
		this.reading = null;
		this.readingId = null;
		if (this.lineFor !== selection.objects.join('\u0000')) {
			this.lineFor = selection.objects.join('\u0000');
			this.line = null;
			this.pair = null;
			this.picks = [];
			this.shapeView = null;
		}
		void this.loadActions();
		if (!selection.objects.length || !this.app.project.summary) {
			this.shown = null;
			this.showError = null;
			return;
		}
		try {
			const res = await this.app.analysis.request<Shown>(
				'project.show',
				{
					objects: selection.objects,
					records: selection.records,
					part: selection.part ? { kind: selection.part.kind, id: selection.part.id } : null,
					prefs: {
						flat: this.prefs.flat,
						log_frequency: this.prefs.log_frequency,
						component: this.prefs.component,
						line: this.line,
						projection: this.prefs.projection,
						shape_view: this.shapeView,
						values_dofs: this.prefs.values_dofs,
						values_form: this.prefs.values_form,
						arrows: this.prefs.arrows,
						sensors: this.prefs.sensors,
						environment: this.prefs.environment,
						table_view: this.prefs.table_view,
						tone: this.prefs.tone,
						sine_reading: this.prefs.sine_reading
					}
				},
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			if (seq !== this.showSeq) return;
			this.shown = res;
			this.showError = null;
		} catch (err) {
			if (seq !== this.showSeq) return;
			this.shown = null;
			this.showError = message(err);
		}
	}

	/** What the selection can take; the current tool stays when it still applies, else the first reading. */
	async loadActions(): Promise<void> {
		const seq = ++this.actionSeq;
		if (!this.app.project.summary || !this.app.analysis.isOpen) {
			this.actions = null;
			return;
		}
		const selection = this.app.project.selection;
		try {
			const res = await this.app.analysis.request<Actions>('project.actions', {
				objects: selection.objects,
				records: selection.records,
				part: selection.part ? { kind: selection.part.kind, id: selection.part.id } : null
			});
			if (seq !== this.actionSeq) return;
			this.actions = res;
			if (!this.onProject) return;
			const current = res.table.columns.find((column) => column.current);
			if (this.joined) {
				const row = current?.rows.find((r) => r.id === this.joined);
				this.joined = null;
				if (row?.ready && current) void this.perform(row, current);
				return;
			}
			// an open form follows the selection while the selection can still take it
			if (this.formOpen) {
				const row = current?.rows.find((r) => r.id === this.opId && r.ready);
				if (row) this.openRow(row);
				else this.closeForm();
			}
			this.fitWeighing = false;
		} catch (err) {
			if (seq === this.actionSeq) this.actions = null;
			void err;
		}
	}

	/** One of a lone shape set's views takes the frame; null is the one selecting it shows. */
	private showShapeView(view: ShapeView | null): void {
		this.shapeView = view;
		this.pair = null;
		this.picks = [];
		void this.showSelection();
	}

	/** A row of the action table, clicked. */
	async perform(row: ActionRow, column: ActionColumn): Promise<void> {
		if (!row.ready) return;
		if (!column.current) {
			const partner = column.kinds[column.kinds.length - 1];
			const candidates = row.candidates ?? [];
			if (candidates.length === 1) this.join(candidates[0], row);
			else this.picking = { verb: row.verb, kind: partner, row };
			return;
		}
		if (row.kind === 'default') {
			this.endReading();
			if (this.shapeView) this.showShapeView(null);
			return;
		}
		if (row.id.startsWith('show.')) {
			this.showShapeView(row.id.slice('show.'.length) as ShapeView);
			return;
		}
		if (row.kind === 'self') {
			await this.app.project.ownAct(row.id);
			return;
		}
		if (row.formats && hasNativeFiles()) {
			await this.exportObject(row, row.formats);
			return;
		}
		this.openRow(row);
		const op = this.op;
		if (!op) return;
		const filled = new Set(Object.keys(row.fill ?? {}));
		const asks = this.paramsOf(op).filter((p) => !p.in_view && !filled.has(p.name));
		if (asks.length) this.formOpen = true;
		else await this.run();
	}

	/** A row's operation, filled from the selection, with a format the object can be written in. */
	private openRow(row: ActionRow): void {
		this.pickAction({ id: row.id, fill: row.fill ?? {} });
		this.formRow = row;
		const formats = row.formats;
		if (formats && !formats.some((f) => f.value === this.currentValues.format)) {
			this.pickAction({ id: row.id, fill: { format: formats[0].value } });
		}
	}

	/** Export in the desktop app: the native save window says where and in which format, as in Visual Dynamics. */
	private async exportObject(row: ActionRow, formats: ExportFormat[]): Promise<void> {
		this.runError = null;
		try {
			const chosen = await chooseObjectFile(row.fill?.object ?? '', formats);
			if (!chosen) return;
			this.pickAction({ id: row.id, fill: { ...row.fill, format: chosen.format, path: chosen.path, units: 'display', creator: '' } });
			await this.run();
		} catch (err) {
			this.runError = message(err);
		}
	}

	/** The object a combination needed was chosen: it joins the selection, and the row runs once the table has caught up. */
	join(name: string, row: ActionRow): void {
		this.picking = null;
		this.joined = row.kind === 'default' ? null : row.id;
		const p = this.app.project;
		p.clickObject(name, p.summary?.objects.map((o) => o.name) ?? [], { toggle: true });
	}

	/** Back from a form to the table. */
	closeForm(): void {
		this.formOpen = false;
		this.formRow = null;
		this.runError = null;
		this.triedRun = false;
	}

	/** A row chosen in the action table: its tool, with the selection filled into its params. */
	pickAction(entry: { id: string; fill: Record<string, string> }): void {
		if (this.opId !== entry.id) this.selectOp(entry.id);
		const op = this.op;
		if (!op) return;
		const values = { ...this.currentValues };
		let changed = false;
		for (const [key, text] of Object.entries(entry.fill)) {
			const value = op.params.find((p) => p.name === key)?.type === 'integer' ? Number(text) : text;
			if (values[key] !== value) {
				values[key] = value;
				changed = true;
			}
		}
		if (changed) this.values = { ...this.values, [op.id]: values };
	}

	/** The plot's cursor moved on a stage that reads at a line: the line follows, once the cursor rests. */
	moveLine(frequency: number | null): void {
		if (frequency === null) return;
		if (this.lineTimer) clearTimeout(this.lineTimer);
		this.lineTimer = setTimeout(() => {
			this.lineTimer = null;
			if (this.line === frequency) return;
			this.line = frequency;
			void this.showSelection();
		}, 150);
	}

	/** One of the bar's choices changed: kept, and the selection read again. */
	setPref<K extends keyof ShowPrefs>(key: K, value: ShowPrefs[K]): void {
		this.prefs = { ...this.prefs, [key]: value };
		try {
			localStorage.setItem(PREFS_KEY, JSON.stringify(this.prefs));
		} catch {
			// kept for this session only
		}
		// the shell's own choices redraw what is already here; the rest are the service's to answer
		if (key !== 'bars' && key !== 'overlay') void this.showSelection();
	}

	/** A MAC square clicked: picked alone, or toggled among the picks with the modifier; the last picked animates. */
	pickCell(cell: { row: number; column: number }, extend = false): void {
		const next = togglePick(this.picks, cell, extend);
		this.picks = next.picks;
		this.pair = next.active;
	}

	/** The matrix the selection shows, when it compares two sets. */
	get comparison(): MatrixView | null {
		const m = this.shown?.views.find((v): v is MatrixView => v.view === 'matrix' && v.selects === 'shape3d' && v.rows.label !== v.columns.label);
		return m ?? null;
	}

	/** The picked squares kept as matched modes of the two sets (`project.add_matches`); the marks follow. */
	async addMatches(): Promise<void> {
		const m = this.comparison;
		if (!m || !this.picks.length) return;
		const done = await this.app.project.act('Adding matches', 'project.add_matches', {
			first: m.rows.label,
			second: m.columns.label.replace(/, projected$/, ''),
			pairs: pairsText(this.picks)
		});
		if (done) void this.showSelection();
	}

	/** Back from a reading to what the selection shows. */
	endReading(): void {
		this.reading = null;
		this.readingId = null;
	}

	/** A reading's own control changed (a waterfall's quantity): run it again with the patch. */
	async rerunReading(patch: ParamValues): Promise<void> {
		const op = this.op;
		if (!op || !this.reading) return;
		this.values = { ...this.values, [op.id]: { ...this.currentValues, ...patch } };
		await this.run();
	}

	private previewTimer: ReturnType<typeof setTimeout> | null = null;
	private previewKey = '';
	private previewSeq = 0;

	/** A form that previews, typed into: once the typing rests, what it would do is drawn, on a copy. */
	followPreview(): void {
		const op = this.op;
		const values = this.currentValues;
		if (!op?.previews || !this.onProject || Object.keys(this.errors).length) return;
		const key = `${op.id}\u0000${JSON.stringify(values)}`;
		if (key === this.previewKey) return;
		this.previewKey = key;
		if (this.previewTimer) clearTimeout(this.previewTimer);
		const seq = ++this.previewSeq;
		this.previewTimer = setTimeout(async () => {
			try {
				const res = await this.app.analysis.request<OpResult>('op.run', { op: op.id, params: values, preview: true });
				if (seq !== this.previewSeq || this.opId !== op.id) return;
				this.reading = { title: `${op.title}, preview`, views: res.views };
				this.readingId = op.id;
				this.runError = null;
			} catch (err) {
				if (seq === this.previewSeq && this.opId === op.id) this.runError = message(err);
			}
		}, 300);
	}

	/** A click in the reading's 3-D view: runs the view's operation with what was clicked, and the form holds what it left. */
	async pickInReading(pick: { op: string; params: Record<string, unknown> }, patch: Record<string, unknown>): Promise<void> {
		const title = this.reading?.title ?? '';
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: pick.op, params: { ...pick.params, ...patch } }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			this.reading = { title, views: res.views };
			const next = res.views.find((v) => v.view === 'shape3d' && v.pick?.op === pick.op);
			if (next?.view === 'shape3d' && next.pick && this.values[pick.op]) {
				this.values = { ...this.values, [pick.op]: { ...this.values[pick.op], ...(next.pick.params as ParamValues) } };
			}
		} catch (err) {
			this.runError = message(err);
		}
	}

	/**
	 * An object selected in the tree: the current tool's first object param that takes its kind
	 * (or whose verb it answers to) is set to it, so picking in the tree is picking the source.
	 */
	followSelection(name: string): void {
		const op = this.op;
		const obj = this.app.project.summary?.objects.find((o) => o.name === name);
		if (!op || !this.onProject || !obj) return;
		for (const p of this.paramsOf(op)) {
			const kinds = objectKinds(p);
			const verb = objectVerb(p);
			if ((kinds.length && kinds.includes(obj.kind)) || (verb && obj.verbs?.includes(verb))) {
				if (this.currentValues[p.name] === name) return;
				this.values = { ...this.values, [op.id]: { ...this.currentValues, [p.name]: name } };
				return;
			}
		}
	}

	selectOp(id: string | null): void {
		this.opId = id;
		this.runError = null;
		this.triedRun = false;
		const op = this.op;
		if (op && !this.values[op.id]) this.values[op.id] = defaultValues(this.paramsOf(op));
	}

	resetParams(): void {
		const op = this.op;
		if (op) this.values[op.id] = defaultValues(this.paramsOf(op));
		this.triedRun = false;
	}

	async run(): Promise<void> {
		const op = this.op;
		if (!op || !this.onProject || this.running) return;
		this.triedRun = true;
		if (Object.keys(this.errors).length) return;
		const params = { ...this.currentValues };
		this.running = true;
		this.runError = null;
		try {
			const before = new Set(this.app.project.summary?.objects.map((o) => o.name) ?? []);
			const res = await this.app.analysis.request<OpResult>('op.run', { op: op.id, params }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			// as Visual Dynamics works: an act's result joins the tree and is selected; a reading
			// takes the view until Back
			const added = (res.project?.objects ?? []).map((o) => o.name).filter((n) => !before.has(n));
			if (added.length) {
				const order = (res.project?.objects ?? []).map((o) => o.name);
				this.app.project.clickObject(added[0], order);
				this.closeForm();
			} else if (res.views.length) {
				this.reading = { title: op.title, views: res.views };
				this.readingId = op.id;
			} else {
				this.closeForm();
			}
			this.triedRun = false;
		} catch (err) {
			this.runError = message(err);
		} finally {
			this.running = false;
		}
	}

	/** A cell typed into in what the selection shows: the edit runs, and the table it answers with takes the view's place. */
	async editShownCell(viewIndex: number, edit: TableEdit, row: number, column: string, value: string): Promise<string | null> {
		try {
			const res = await this.app.analysis.request<OpResult>(
				'op.run',
				{ op: edit.op, params: { ...edit.params, row: edit.rows ? edit.rows[row] : row + 1, column, value } },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			if (res.project) this.app.project.apply(res.project);
			const table = res.views.find((v) => v.view === 'table');
			// a table showing a pick of its rows is shown again, since the one that comes back has them all
			if (edit.rows) void this.showSelection();
			else if (table && this.shown) this.shown = { ...this.shown, views: this.shown.views.map((v, i) => (i === viewIndex ? table : v)) };
			return null;
		} catch (err) {
			return message(err);
		}
	}
}

/** How long an operation may take: fitting or computing on a large project is not quick. */
const RUN_TIMEOUT_MS = 120_000;

// ---------------------------------------------------------------------------------------------


// ---------------------------------------------------------------------------------------------

export interface AbortNotice {
	runId: string;
	atS: number;
	reason: string;
}

class SimState {
	profiles = $state.raw<SimProfile[]>([]);
	profilesError = $state<string | null>(null);
	values = $state<ParamValues>({ profile: '', level_db: 0, speed: 1 });
	runState = $state<RunState>('idle');
	runId = $state<string | null>(null);
	display = $state.raw<DisplayEvent | null>(null);
	abort = $state<AbortNotice | null>(null);
	busy = $state(false);
	error = $state<string | null>(null);
	private stopRequested = false;

	constructor(private readonly app: AppState) {
		app.control.on('random.display', (data) => this.onDisplay(data as unknown as DisplayEvent));
	}

	/** The start form, as a descriptor so it renders with the same ParamForm as every tool. */
	get params(): ParamDescriptor[] {
		return [
			{
				name: 'profile',
				label: 'Profile',
				type: 'enum',
				options: this.profiles.map((p) => ({ value: p.id, label: p.title })),
				default: this.profiles[0]?.id
			},
			{ name: 'level_db', label: 'Level', type: 'number', unit: 'dB', default: 0, min: -24, max: 6, step: 0.5 },
			{
				name: 'speed',
				label: 'Speed',
				type: 'enum',
				options: [
					{ value: 1, label: 'Real Time' },
					{ value: 0, label: 'Fast' }
				],
				default: 1
			}
		];
	}

	get profile(): SimProfile | null {
		return this.profiles.find((p) => p.id === this.values.profile) ?? null;
	}

	get canStart(): boolean {
		return this.app.control.isOpen && this.runState !== 'running' && !this.busy && Object.keys(validateAll(this.params, this.values)).length === 0;
	}

	get canStop(): boolean {
		return this.app.control.isOpen && this.runState === 'running' && !this.busy;
	}

	async sync(): Promise<void> {
		try {
			const [{ profiles }, state] = await Promise.all([
				this.app.control.request<{ profiles: SimProfile[] }>('sim.profiles'),
				this.app.control.request<SimStateResult>('sim.state')
			]);
			this.profiles = profiles;
			this.profilesError = null;
			this.values = reconcile(this.params, this.values);
			this.runState = state.state;
			this.runId = state.run_id;
		} catch (err) {
			this.profilesError = message(err);
		}
	}

	async start(): Promise<void> {
		if (!this.canStart) return;
		this.busy = true;
		this.error = null;
		this.abort = null;
		this.stopRequested = false;
		try {
			const res = await this.app.control.request<{ run_id: string }>('sim.start', {
				profile: this.values.profile,
				level_db: this.values.level_db,
				speed: this.values.speed
			});
			this.runId = res.run_id;
			this.runState = 'running';
			this.display = null;
		} catch (err) {
			this.error = message(err);
		} finally {
			this.busy = false;
		}
	}

	async stop(): Promise<void> {
		if (!this.canStop) return;
		this.busy = true;
		this.stopRequested = true;
		try {
			const res = await this.app.control.request<{ state: RunState }>('sim.stop');
			this.runState = res.state;
		} catch (err) {
			this.error = message(err);
		} finally {
			this.busy = false;
		}
	}

	private onDisplay(d: DisplayEvent): void {
		if (this.runId && d.run_id !== this.runId && this.runState === 'running') return;
		const wasRunning = this.runState === 'running';
		this.display = d;
		this.runId = d.run_id;
		this.runState = d.state;
		// The service says how a run ended. Without an outcome, a stop the shell did not ask
		// for is treated as an abort; the controller's reason wins, then the first failing reading.
		const ended = d.outcome ? d.outcome !== 'completed' && d.outcome !== 'operator_stop' : !this.stopRequested;
		if (d.state === 'stopped' && wasRunning && ended) {
			const fail = d.views.flatMap((v) => (v.view === 'reading' ? v.rows : [])).find((r) => r.status === 'fail');
			const fromRow = fail
				? typeof fail.value === 'string' && fail.value.length > 24
					? fail.value
					: `${fail.label}: ${fail.value}${fail.unit ? ` ${fail.unit}` : ''}`
				: null;
			this.abort = {
				runId: d.run_id,
				atS: d.elapsed_s,
				reason: d.reason ?? fromRow ?? 'The control service stopped the run without a request from the shell.'
			};
		}
	}
}

// ---------------------------------------------------------------------------------------------

/** How long opening or importing a file may take before the shell stops waiting. */
const FILE_TIMEOUT_MS = 600_000;

/**
 * The mode fit under way on the project (`fit.*`): Visual Dynamics' fitting loop, one mode at a time
 * on the residual. The x range on screen is the search band Find Mode reads.
 */
export class FitState {
	state = $state.raw<FitStateView | null>(null);
	busy = $state<string | null>(null);
	error = $state<string | null>(null);
	/** The frequencies on screen, [low, high] Hz, or null for the whole range. */
	range: [number, number] | null = null;

	constructor(private readonly app: AppState) {}

	start(frf: string, shapes?: string, coherence?: string): Promise<boolean> {
		this.range = null;
		// records picked in the tree limit the fit, as in Visual Dynamics; an Edit Fit takes its set's own
		const records = shapes ? [] : (this.app.project.selection.records[frf] ?? []);
		return this.step('Starting the fit', 'fit.start', { frf, ...(shapes ? { shapes } : {}), ...(coherence ? { coherence } : {}), ...(records.length ? { records } : {}) });
	}

	find(): Promise<boolean> {
		return this.step('Finding a mode', 'fit.find', this.range ? { low: this.range[0], high: this.range[1] } : {});
	}

	move(frequency: number): Promise<boolean> {
		return this.step('Moving the cursor', 'fit.move', { frequency });
	}

	holdDamping(damping: number): Promise<boolean> {
		return this.step('Holding the damping', 'fit.damping', { damping });
	}

	fit(): Promise<boolean> {
		return this.step('Fitting the mode', 'fit.fit', {});
	}

	confirm(description?: string): Promise<boolean> {
		return this.step('Confirming the mode', 'fit.confirm', description ? { description } : {});
	}

	refine(): Promise<boolean> {
		return this.step('Refining the residues', 'fit.refine', {});
	}

	/** Take confirmed modes back, by their index in `modes`. */
	remove(modes: number[]): Promise<boolean> {
		return this.step(`Taking back mode ${modes.map((k) => k + 1).join(', ')}`, 'fit.delete', { modes });
	}

	/** Type a confirmed mode's damping (a fraction of critical) or description into its row. */
	edit(mode: number, change: { damping?: number; description?: string }): Promise<boolean> {
		return this.step(`Editing mode ${mode + 1}`, 'fit.edit', { mode, ...change });
	}

	showResidual(show: boolean): Promise<boolean> {
		return this.step(show ? 'Drawing the residual' : 'Hiding the residual', 'fit.residual', { show });
	}

	async publish(name?: string): Promise<boolean> {
		this.busy = 'Publishing';
		this.error = null;
		try {
			const res = await this.app.analysis.request<{ name: string; project: ProjectSummary; fit: FitStateView }>(
				'fit.publish',
				name ? { name } : {},
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			this.state = res.fit;
			this.app.project.apply(res.project);
			return true;
		} catch (err) {
			this.error = message(err);
			return false;
		} finally {
			this.busy = null;
		}
	}

	async end(): Promise<void> {
		this.state = null;
		this.error = null;
		try {
			await this.app.analysis.request('fit.end', {});
		} catch {
			// the fit is gone either way
		}
	}

	private async step(doing: string, method: string, params: Record<string, unknown>): Promise<boolean> {
		this.busy = doing;
		this.error = null;
		try {
			const res = await this.app.analysis.request<{ fit: FitStateView }>(method, params, { timeoutMs: RUN_TIMEOUT_MS });
			this.state = res.fit;
			return true;
		} catch (err) {
			this.error = message(err);
			return false;
		} finally {
			this.busy = null;
		}
	}
}

/**
 * The project's reports (`report.*`): Visual Dynamics' report, generated from a starter template and
 * shown as the page the core writes.
 */
export class ReportState {
	/** The report shown, and its page. */
	name = $state<string | null>(null);
	html = $state<string | null>(null);
	busy = $state<string | null>(null);
	error = $state<string | null>(null);
	/** Where the last export went, and what the page left out and the check found. */
	exported = $state<string | null>(null);
	exportedNote = $state('');
	/**
	 * What an export, a print or an issue is held on before a report leaves the project: the
	 * sections still owed and the indexed files that do not match; the person says anyway or lets
	 * it be.
	 */
	held = $state<{ act: 'export' | 'print' | 'issue'; path?: string; owed: string[]; index: { name: string; status: string }[] } | null>(null);
	/** The editor's pane for the selection, from the last `report.editor`. */
	editor = $state.raw<ReportEditor | null>(null);
	/** How far the page was scrolled, kept across the re-render an edit makes. */
	scroll = 0;

	constructor(private readonly app: AppState) {}

	async generate(template: string): Promise<void> {
		await this.act('Generating the report', async () => {
			const res = await this.app.analysis.request<{ name: string; project: ProjectSummary }>(
				'report.generate',
				{ template },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			this.app.project.apply(res.project);
			await this.load(res.name);
		});
	}

	/** Show the report `name`, in the shell's own theme. */
	async show(name: string): Promise<void> {
		await this.act(`Opening ${name}`, () => this.load(name));
	}

	async export(path: string, anyway = false): Promise<void> {
		const name = this.name;
		if (!name) return;
		this.held = null;
		await this.act('Saving the page', async () => {
			const res = await this.app.analysis.request<{ path: string | null; owed: string[]; index: { name: string; status: string }[]; note?: string }>(
				'report.export',
				{ name, path, ...(anyway ? { anyway } : {}) },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			if (res.path === null) this.held = { act: 'export', path, owed: res.owed, index: res.index };
			else {
				this.exported = res.path;
				this.exportedNote = res.note ?? '';
			}
		});
	}

	/** The held export, print or issue, done anyway. */
	async anyway(): Promise<void> {
		const held = this.held;
		if (!held) return;
		if (held.act === 'export' && held.path) await this.export(held.path, true);
		else if (held.act === 'print') await this.print(true);
		else await this.issue('begin', undefined, true);
	}

	/** The page being printed, in a frame of its own that asks for the print dialog. */
	printing = $state<string | null>(null);
	/** An issue under way: the page that says issued is printed; the PDF is named next, which issues it. */
	issuing = $state<string | null>(null);
	/** The last issue recorded. */
	issued = $state<Record<string, string> | null>(null);

	/** Print the reader's page; the print dialog saves it as a PDF. */
	async print(anyway = false): Promise<void> {
		const name = this.name;
		if (!name) return;
		this.held = null;
		await this.act('Preparing the page to print', async () => {
			const res = await this.app.analysis.request<{ html: string | null; owed: string[]; index: { name: string; status: string }[] }>(
				'report.print',
				{ name, ...(anyway ? { anyway } : {}) },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			if (res.html === null) this.held = { act: 'print', owed: res.owed, index: res.index };
			else this.printing = res.html;
		});
	}

	/** Begin, record or cancel an issue (`report.issue`). */
	async issue(step: 'begin' | 'record' | 'cancel', path?: string, anyway = false): Promise<void> {
		const name = this.name;
		if (!name) return;
		this.held = null;
		const doing = { begin: 'Preparing the page to issue', record: 'Recording the issue', cancel: 'Cancelling the issue' }[step];
		await this.act(doing, async () => {
			const res = await this.app.analysis.request<{ html?: string; status?: string; entry?: Record<string, string>; index?: { name: string; status: string }[]; project: ProjectSummary }>(
				'report.issue',
				{ name, step, ...(path ? { path } : {}), ...(anyway ? { anyway } : {}) },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			this.app.project.apply(res.project);
			if (step === 'begin' && res.index) {
				this.held = { act: 'issue', owed: [], index: res.index };
			} else if (step === 'begin') {
				this.issuing = res.status ?? 'issued';
				this.issued = null;
				this.printing = res.html ?? null;
			} else {
				this.issuing = null;
				if (res.entry) this.issued = res.entry;
				await this.load(name);
			}
		});
	}

	/** Select a block on the page (null for the report itself); its pane follows. */
	async select(at: number | null): Promise<void> {
		const name = this.name;
		if (!name || at === (this.editor?.selected ?? null)) return;
		// the page framed the block itself: the pane alone comes back
		await this.act('Selecting', async () => this.take(await this.request('report.editor', { name, selected: at, page: false })));
	}

	/** One of the editor's operations (`report.operate`); the page and pane follow. */
	async operate(op: string, params: Record<string, unknown> = {}): Promise<boolean> {
		const name = this.name;
		if (!name) return false;
		let ok = false;
		await this.act('Editing the report', async () => {
			const res = await this.request('report.operate', { name, op, selected: this.editor?.selected ?? null, ...params });
			if (res.project) this.app.project.apply(res.project);
			this.take(res);
			ok = true;
		});
		return ok;
	}

	private request(method: string, params: Record<string, unknown>): Promise<ReportEditor> {
		return this.app.analysis.request<ReportEditor>(method, { ...params, theme: theme.resolved }, { timeoutMs: RUN_TIMEOUT_MS });
	}

	private take(res: ReportEditor): void {
		// a pane alone keeps the page and the labels rendered with it
		this.editor = res.html === undefined ? { ...res, references: this.editor?.references ?? [] } : res;
		if (res.html !== undefined) this.html = res.html;
	}

	private async load(name: string): Promise<void> {
		const keep = this.name === name ? (this.editor?.selected ?? null) : null;
		if (this.name !== name) this.scroll = 0;
		const res = await this.request('report.editor', { name, selected: keep });
		this.name = name;
		this.take(res);
		this.exported = null;
	}

	private async act(doing: string, work: () => Promise<void>): Promise<void> {
		this.busy = doing;
		this.error = null;
		try {
			await work();
		} catch (err) {
			this.error = message(err);
		} finally {
			this.busy = null;
		}
	}
}

/** A unit system's length unit, and what takes it to meters. */
interface LengthUnit {
	unit: string;
	to_si: number;
}

/** Where the chosen unit system is kept between launches. */
const UNITS_KEY = 'hertzforge.units';

/**
 * The display unit system (`units.system`): every plot and reading the analysis service sends is
 * in it, as Visual Dynamics' unit selector puts its window in one. Remembered between launches and
 * told to the service each time it connects; a change draws the results on screen again.
 */
export class UnitsState {
	name = $state<string | null>(null);
	systems = $state<string[]>([]);
	/** The system's length unit, and what takes it to meters: what a solid's form is typed in. */
	length = $state.raw<LengthUnit | null>(null);

	constructor(private readonly app: AppState) {}

	private stored(): string | null {
		try {
			return localStorage.getItem(UNITS_KEY);
		} catch {
			return null;
		}
	}

	/** The service connected: tell it the system chosen before, or learn its own. */
	async sync(): Promise<void> {
		const wanted = this.stored();
		try {
			const res = await this.app.analysis.request<{ name: string; systems: string[]; length?: LengthUnit }>('units.system', {});
			this.systems = res.systems;
			if (wanted && wanted !== res.name && res.systems.includes(wanted)) {
				const set = await this.app.analysis.request<{ name: string; length?: LengthUnit }>('units.system', { name: wanted });
				this.name = set.name;
				this.length = set.length ?? null;
			} else {
				this.name = res.name;
				this.length = res.length ?? null;
			}
		} catch {
			// an older service without units: its own SI stands
		}
	}

	async choose(name: string): Promise<void> {
		if (name === this.name) return;
		const res = await this.app.analysis.request<{ name: string; length?: LengthUnit }>('units.system', { name });
		this.name = res.name;
		const before = this.length;
		this.length = res.length ?? null;
		if (before && this.length) this.app.analyze.lengthsChanged(before.to_si / this.length.to_si);
		try {
			localStorage.setItem(UNITS_KEY, res.name);
		} catch {
			// not kept between launches; this session still uses it
		}
		// what is on screen was drawn in the old system: draw it again
		if (this.app.report.name) void this.app.report.show(this.app.report.name);
	}
}

/**
 * The connection's own project (`project.*`): what it holds, and the file work under way. Opening
 * or importing switches Analyze to it.
 */
const PREFS_KEY = 'hertzforge.show.prefs';

function readPrefs(): ShowPrefs {
	const prefs: ShowPrefs = {
		flat: false,
		log_frequency: false,
		component: 'magnitude',
		projection: false,
		bars: true,
		overlay: true,
		values_dofs: 'all',
		values_form: 'shapes',
		arrows: false,
		sensors: true,
		environment: null,
		table_view: 'channels',
		tone: null,
		sine_reading: 'curves'
	};
	try {
		const kept = JSON.parse(localStorage.getItem(PREFS_KEY) ?? '{}') as Partial<ShowPrefs>;
		if (typeof kept.flat === 'boolean') prefs.flat = kept.flat;
		if (typeof kept.log_frequency === 'boolean') prefs.log_frequency = kept.log_frequency;
		for (const key of ['projection', 'bars', 'overlay', 'arrows', 'sensors'] as const) {
			if (typeof kept[key] === 'boolean') prefs[key] = kept[key];
		}
		if (kept.values_form && ['shapes', 'response', 'reference'].includes(kept.values_form)) prefs.values_form = kept.values_form;
		if (kept.component && ['magnitude', 'real', 'imag', 'phase'].includes(kept.component)) prefs.component = kept.component;
	} catch {
		// nothing kept, or no storage
	}
	return prefs;
}

export class ProjectState {
	summary = $state.raw<ProjectSummary | null>(null);
	/** What is being done, in words ("Importing modal.nc4"), or null. */
	busy = $state<string | null>(null);
	progress = $state.raw<ProjectProgress | null>(null);
	error = $state<string | null>(null);
	/** The object whose units are being declared, and its tables. */
	units = $state.raw<UnitsTables | null>(null);
	/** What the tree has selected: objects, records within them, a part of a geometry. */
	selection = $state.raw<Selection>(EMPTY);
	/** The objects whose rows are open in the tree. */
	expanded = $state.raw<Set<string>>(new Set());
	/** What each open object holds, as the service last answered. */
	children = $state.raw<Record<string, ProjectChildren>>({});
	/** The object being renamed in the tree, or null. */
	renaming = $state<string | null>(null);
	/** The photo being renamed in place: its object and index, or null. */
	renamingPart = $state<{ name: string; index: number } | null>(null);
	/** The selection whose removal waits on a second click or Enter (Delete arms it from the tree). */
	confirming = $state(false);
	/** The group being named from the action table: the member it is reached by, or null. */
	namingGroup = $state<string | null>(null);
	/** The path a browser is asking for in the top bar, and what it is for. */
	asking = $state<'open' | 'import' | 'save' | null>(null);
	/** The last record clicked in each object's grid, where a Shift extension starts. */
	private anchorRecord: Record<string, number> = {};
	private childSeq = 0;

	constructor(private readonly app: AppState) {
		app.analysis.on('project.progress', (data) => {
			if (this.busy) this.progress = data as unknown as ProjectProgress;
		});
	}

	/** A new summary from the service: the tree and Analyze's object choices follow. */
	apply(summary: ProjectSummary): void {
		this.summary = summary;
		if (this.units && !summary.objects.some((o) => o.name === this.units?.name)) this.units = null;
		const names = summary.objects.map((o) => o.name);
		this.selection = pruned(this.selection, names);
		if (this.renaming && !names.includes(this.renaming)) this.renaming = null;
		// what an open row holds may have changed: ask again, and forget rows that are gone
		const kept = new Set([...this.expanded].filter((n) => names.includes(n)));
		this.expanded = kept;
		const children: Record<string, ProjectChildren> = {};
		for (const name of kept) if (this.children[name]) children[name] = this.children[name];
		this.children = children;
		for (const name of kept) void this.loadChildren(name);
		this.app.analyze.projectChanged();
		void this.app.analyze.showSelection();
		void this.app.console.sync();
	}

	/** Open or close an object's row; opening asks what it holds. */
	toggleExpanded(name: string): void {
		const next = new Set(this.expanded);
		if (next.has(name)) next.delete(name);
		else {
			next.add(name);
			void this.loadChildren(name);
		}
		this.expanded = next;
	}

	async loadChildren(name: string): Promise<void> {
		const seq = ++this.childSeq;
		try {
			const held = await this.app.analysis.request<ProjectChildren>('project.children', { name });
			if (!this.expanded.has(name) || !this.summary?.objects.some((o) => o.name === name)) return;
			this.children = { ...this.children, [name]: held };
		} catch (err) {
			if (seq === this.childSeq) this.error = message(err);
		}
	}

	private selected(next: Selection): void {
		this.selection = next;
		this.renaming = null;
		this.renamingPart = null;
		this.confirming = false;
		this.namingGroup = null;
		this.units = null;
		if (next.objects.length === 1) this.app.analyze.followSelection(next.objects[0]);
		void this.app.analyze.showSelection();
	}

	/** An object's row clicked, with the modifiers held; `order` is the tree's rows. */
	clickObject(name: string, order: string[], mods: Modifiers = {}): void {
		this.selected(clickObject(this.selection, name, order, mods));
	}

	/** A cell of `name`'s grid clicked: its record; Shift takes the block from the last cell clicked. */
	clickRecord(name: string, index: number, mods: Modifiers = {}, grid?: number[][]): void {
		const from = this.anchorRecord[name];
		const block = mods.extend && grid && from !== undefined ? { from, to: index, grid } : undefined;
		if (!mods.extend) this.anchorRecord[name] = index;
		this.selected(clickRecord(this.selection, name, index, mods, block));
	}

	clickPart(name: string, kind: string, id: string | number): void {
		this.selected(clickPart(this.selection, name, kind, id));
	}

	clearSelection(): void {
		this.selected(EMPTY);
	}

	/** Whether `name`, or one of its records, is selected. */
	isSelected(name: string): boolean {
		return this.selection.objects.includes(name);
	}

	/**
	 * Open or import each path, as a drop or a dialog gives them: a project file opens (replacing
	 * the project), anything else is imported into it. Stops at the first refusal and says why.
	 */
	async openPaths(paths: string[]): Promise<void> {
		for (const path of paths) {
			const ok = isProjectFile(path) ? await this.open(path) : await this.importFile(path);
			if (!ok) return;
		}
	}

	open(path: string): Promise<boolean> {
		return this.call(`Opening ${baseName(path)}`, 'project.open', { path });
	}

	importFile(path: string, units?: Record<string, string>): Promise<boolean> {
		return this.call(`Importing ${baseName(path)}`, 'project.import', units ? { path, units } : { path });
	}

	save(path?: string): Promise<boolean> {
		return this.call('Saving', 'project.save', path ? { path } : {}, false);
	}

	newProject(title: string): Promise<boolean> {
		return this.call('Starting a project', 'project.new', { title });
	}

	async rename(name: string, to: string): Promise<boolean> {
		const ok = await this.call(`Renaming ${name}`, 'project.rename', { name, to }, false);
		if (ok) {
			this.selection = renamed(this.selection, name, to);
			if (this.expanded.has(name)) {
				const next = new Set(this.expanded);
				next.delete(name);
				next.add(to);
				this.expanded = next;
				void this.loadChildren(to);
			}
		}
		return ok;
	}

	remove(names: string[]): Promise<boolean> {
		return this.call(`Removing ${names.join(', ')}`, 'project.remove', { names }, false);
	}

	/**
	 * Open, Import, Save or Save As. In the desktop app these are the native dialogs; in a browser,
	 * which never gives the shell a file's path, the top bar asks for one (files.ts).
	 */
	async fileAct(what: 'open' | 'import' | 'save' | 'save-as'): Promise<void> {
		if (what === 'save' && this.summary?.file && !this.summary.file.endsWith('.vdyn')) {
			await this.save();
			return;
		}
		const asked = what === 'save-as' ? 'save' : what;
		if (hasNativeFiles()) {
			if (asked === 'save') {
				const path = await chooseSavePath(this.summary?.title ?? 'Project');
				if (path) await this.save(path);
				return;
			}
			const paths = await chooseFiles(asked);
			if (paths) await this.openPaths(paths);
			return;
		}
		this.asking = this.asking === asked ? null : asked;
	}

	/** An act of the action table's on the selection itself, by its row's id. */
	async ownAct(id: string): Promise<void> {
		if (id === 'self.open' || id === 'self.import' || id === 'self.save') {
			await this.fileAct(id === 'self.open' ? 'open' : id === 'self.import' ? 'import' : 'save');
			return;
		}
		if (id === 'self.recompute_all') {
			await this.refresh();
			return;
		}
		const picked = [...this.selection.objects];
		const one = picked.length === 1 ? picked[0] : null;
		const stale = picked.filter((n) => this.summary?.objects.find((o) => o.name === n)?.stale);
		const kinds = picked.map((n) => this.summary?.objects.find((o) => o.name === n)?.kind);
		const coherent = (this.summary?.objects ?? []).some((o) => o.kind === 'coherence' || o.kind === 'multiple_coherence');
		if (id === 'self.activate' && one) await this.activate(one);
		else if (id === 'self.units' && one) await this.toggleUnits(one);
		else if (id === 'self.recompute') await this.refresh(stale);
		else if (id === 'self.link') await this.link(picked);
		else if (id === 'self.unlink' && one) await this.unlink(one);
		else if (id === 'self.basis' && one) await this.setBasis(one);
		else if (id === 'self.name_group' && one) this.namingGroup = one;
		else if (id === 'self.copy') await this.duplicate(picked);
		else if (id === 'self.rename' && one) this.renaming = one;
		else if (id === 'self.remove') this.confirming = true;
		else if (id === 'self.fit' || id === 'self.edit_fit') {
			if (coherent) {
				this.app.analyze.fitWeighing = true;
				return;
			}
			await this.app.fit.start(picked[kinds.indexOf('frf')], id === 'self.edit_fit' ? picked[kinds.indexOf('shape_set')] : undefined);
		}
	}

	duplicate(names: string[]): Promise<boolean> {
		return this.call(`Copying ${names.join(', ')}`, 'project.duplicate', { names }, false);
	}

	/** Compute derived objects again from their sources; with none named, every stale one. */
	refresh(names: string[] = []): Promise<boolean> {
		return this.call(names.length ? `Recomputing ${names.join(', ')}` : 'Recomputing what is stale', 'project.refresh', { names });
	}

	activate(geometry: string): Promise<boolean> {
		return this.call(`Drawing on ${geometry}`, 'project.activate', { geometry }, false);
	}

	/** Link the selected objects into one group, as VD's Link does; the group merges any they are in. */
	link(names: string[]): Promise<boolean> {
		const [first, second, ...more] = names;
		return this.act(`Linking ${names.join(', ')}`, 'project.link', { first, second, more: more.join(', '), name: '' });
	}

	unlink(name: string): Promise<boolean> {
		return this.act(`Unlinking ${name}`, 'project.unlink', { object: name });
	}

	/** The objects copied from the tree (Cmd-C), pasted as copies (Cmd-V), as VD pastes its own objects. */
	copied = $state.raw<string[]>([]);

	copySelection(): boolean {
		const names = [...this.selection.objects];
		if (!names.length) return false;
		this.copied = names;
		this.error = null;
		try {
			void navigator.clipboard?.writeText(names.join('\n')).catch(() => {});
		} catch {
			// no clipboard here; the copy still lives in the shell
		}
		return true;
	}

	async paste(): Promise<boolean> {
		// files copied elsewhere since, as VD reads the clipboard: a copy here puts text on it and no file
		const paths = await pastedFiles();
		if (paths.length) {
			await this.openPaths(paths);
			return true;
		}
		const ours = this.copied.filter((n) => this.summary?.objects.some((o) => o.name === n));
		if (!ours.length) {
			this.error = this.copied.length ? 'The copied objects are not in this project' : 'Nothing copied to paste';
			return false;
		}
		return this.duplicate(ours);
	}

	/**
	 * Objects dragged onto `target` join its group and leave their own, as VD's drag moves them;
	 * dragged onto nothing (`target` null) they leave their groups.
	 */
	async moveInto(names: string[], target: string | null): Promise<boolean> {
		const groupOf = (n: string) => (this.summary?.links ?? []).find((g) => g.members.includes(n));
		const moving = names.filter((n) => n !== target && this.summary?.objects.some((o) => o.name === n));
		if (!moving.length) return false;
		if (target === null) {
			for (const n of moving) if (groupOf(n) && !(await this.unlink(n))) return false;
			return true;
		}
		for (const n of moving) {
			const home = groupOf(target);
			const own = groupOf(n);
			if (own && own !== home && !(await this.unlink(n))) return false;
		}
		const joining = moving.filter((n) => groupOf(n) !== groupOf(target) || !groupOf(n));
		return joining.length ? this.link([target, ...joining]) : true;
	}

	setBasis(name: string): Promise<boolean> {
		return this.act(`Making the group of ${name} the Basis`, 'project.set_basis', { object: name });
	}

	/** Name the group `name` is in, or clear its name with blank text. */
	nameGroup(name: string, text: string): Promise<boolean> {
		return this.act(text.trim() ? `Naming the group of ${name}` : `Unnaming the group of ${name}`, 'project.name_object_group', { object: name, name: text });
	}

	/** A DOF typed over in `name`'s grid: that channel (its quantity's) on the object and what derives from it, as VD's grid does. */
	async renameDof(name: string, old: string, next: string, quantity: string): Promise<boolean> {
		const ok = await this.act(`Renaming ${old} to ${next}`, 'project.rename_dof', { source: name, old, new: next, measured: quantity });
		if (ok) void this.loadChildren(name);
		return ok;
	}

	/** A Role cell clicked: the channel's role goes to the next one round, reference, response, monitor. */
	async cycleRole(name: string, index: number, role: string): Promise<boolean> {
		const order = ['reference', 'response', 'monitor'];
		const next = order[(order.indexOf(role) + 1) % order.length];
		const ok = await this.act(`Making ${name}'s channel ${index + 1} a ${next}`, 'project.set_channel_role', { source: name, record: index + 1, role: next });
		if (ok) void this.loadChildren(name);
		return ok;
	}

	/** A solid dragged among its siblings: the parts, or one boolean's solids, in their new order. */
	async reorderSolids(name: string, ids: number[], within: number | null): Promise<boolean> {
		const ok = await this.act(`Reordering ${name}'s solids`, 'project.reorder_solids', { geometry: name, solids: ids.join(', '), within: within ?? 0 });
		if (ok) void this.loadChildren(name);
		return ok;
	}

	/** The sub-items picked in the tree, by object: what Delete takes when any are picked. */
	get pickedParts(): Record<string, number[]> {
		const parts: Record<string, number[]> = {};
		for (const [name, picks] of Object.entries(this.selection.records)) if (picks.length) parts[name] = picks;
		return parts;
	}

	/**
	 * Remove what is selected, once confirmed: the picked sub-items when any are picked (a
	 * co-selected object is context, not a target, as in Visual Dynamics), else the objects.
	 */
	async removeSelected(): Promise<boolean> {
		const names = [...this.selection.objects];
		const parts = this.pickedParts;
		this.confirming = false;
		if (Object.keys(parts).length) return this.deleteParts(parts);
		if (!names.length) return false;
		return this.remove(names);
	}

	/** Delete sub-items by index: records, modes, channels or photos (`project.delete_parts`). */
	async deleteParts(parts: Record<string, number[]>): Promise<boolean> {
		const what = Object.entries(parts).map(([name, picks]) => `${picks.length} of ${name}`).join(', ');
		const ok = await this.call(`Deleting ${what}`, 'project.delete_parts', { parts }, false);
		if (ok) {
			const records = { ...this.selection.records };
			for (const name of Object.keys(parts)) {
				delete records[name];
				if (this.expanded.has(name)) void this.loadChildren(name);
			}
			this.selection = { ...this.selection, records };
			void this.app.analyze.showSelection();
		}
		return ok;
	}

	/** Rename one photo in place (`project.rename` with `index`). */
	async renamePart(name: string, index: number, to: string): Promise<boolean> {
		const ok = await this.call(`Renaming photo ${index + 1} of ${name}`, 'project.rename', { name, to, index }, false);
		if (ok) {
			if (this.expanded.has(name)) void this.loadChildren(name);
			void this.app.analyze.showSelection();
		}
		return ok;
	}

	/** An act on the groups, run as the operation it is; the project it answers with is applied. */
	async act(doing: string, op: string, params: Record<string, unknown>): Promise<boolean> {
		this.busy = doing;
		this.error = null;
		try {
			const res = await this.app.analysis.request<{ project?: ProjectSummary }>('op.run', { op, params }, { timeoutMs: FILE_TIMEOUT_MS });
			if (res.project) this.apply(res.project);
			return true;
		} catch (err) {
			this.error = message(err);
			return false;
		} finally {
			this.busy = null;
		}
	}

	/** Show the tables `name`'s units are declared in, or close them when they are showing. */
	async toggleUnits(name: string): Promise<void> {
		if (this.units?.name === name) {
			this.units = null;
			return;
		}
		this.error = null;
		try {
			this.units = await this.app.analysis.request<UnitsTables>('project.units', { name });
		} catch (err) {
			this.error = message(err);
		}
	}

	/** Declare units by table and row; the tables are read again to show what converted. */
	async defineUnits(name: string, units: { side: string; key: string; unit: string }[]): Promise<boolean> {
		const ok = await this.call(`Declaring units on ${name}`, 'project.define_units', { name, units }, false);
		if (ok) this.units = await this.app.analysis.request<UnitsTables>('project.units', { name });
		return ok;
	}

	private async call(doing: string, method: string, params: Record<string, unknown>, show = true): Promise<boolean> {
		this.busy = doing;
		this.progress = null;
		this.error = null;
		try {
			const res = await this.app.analysis.request<{ project: ProjectSummary }>(method, params, { timeoutMs: FILE_TIMEOUT_MS });
			this.apply(res.project);
			if (show) {
				// a study keeps the window; any other workspace goes to Signals, the recordings' module
				if (!STUDIES.has(this.app.workspace)) this.app.workspace = 'signals';
				this.app.analyze.selectProject();
			}
			return true;
		} catch (err) {
			this.error = message(err);
			return false;
		} finally {
			this.busy = null;
			this.progress = null;
		}
	}
}

// ---------------------------------------------------------------------------------------------

const CONSOLE_KEY = 'hertzforge.console.shown';

/**
 * The console: the session as Python along the bottom of the window, as Visual Dynamics' is. It
 * shows the project's journal, every verb and settings change as the line that replays it,
 * read-only, to be copied into a script; nothing is typed into it (Brandon, 2026-08-30).
 */
export class ConsoleState {
	shown = $state(false);
	lines = $state.raw<string[]>([]);
	error = $state<string | null>(null);
	private seq = 0;

	constructor(private readonly app: AppState) {
		try {
			this.shown = localStorage.getItem(CONSOLE_KEY) === '1';
		} catch {
			// closed
		}
	}

	toggle(): void {
		this.shown = !this.shown;
		try {
			localStorage.setItem(CONSOLE_KEY, this.shown ? '1' : '0');
		} catch {
			// kept for this session only
		}
		if (this.shown) void this.sync();
	}

	/** The journal as the service holds it now; asked again whenever the project changes. */
	async sync(): Promise<void> {
		if (!this.shown || !this.app.project.summary || !this.app.analysis.isOpen) return;
		const seq = ++this.seq;
		try {
			const res = await this.app.analysis.request<{ lines: string[] }>('project.journal', {});
			if (seq !== this.seq) return;
			this.lines = res.lines;
			this.error = null;
		} catch (err) {
			if (seq === this.seq) this.error = message(err);
		}
	}

	get text(): string {
		return this.lines.join('\n');
	}
}

export class AppState {
	readonly config: ShellConfig;
	readonly analysis: ServiceClient;
	readonly control: ServiceClient;

	workspace = $state<Workspace>('signals');
	paletteOpen = $state(false);
	catalog = $state.raw<Catalog | null>(null);
	catalogError = $state<string | null>(null);

	readonly analyze: AnalyzeState;
	readonly project: ProjectState;
	readonly fit: FitState;
	readonly report: ReportState;
	readonly units: UnitsState;
	/** The run log the keeper writes while a test runs, beside any workspace. */
	readonly runLog: RunLogState;
	/** The session as Python, along the bottom of every workspace. */
	readonly console: ConsoleState;
	readonly sim: SimState;
	readonly acquire: AcquireState;
	/** Acquire & Control's test types and which screen it shows. */
	readonly tests: TestTypeView;
	/** The Random run's display: its pages and plots, and the data this window asks for. */
	readonly display: DisplayState;
	/** The workspace rail is minimized, for the plots to have the screen (kept for the session). */
	railCollapsed = $state(false);
	private windows = 0;

	constructor(config: ShellConfig) {
		this.config = config;
		const analysisFactory = config.mock ? mockSocketFactory(createAnalysisServer(), 80) : undefined;
		this.analysis = new ServiceClient({ service: 'analysis', url: config.analysisUrl, socketFactory: analysisFactory });
		this.control = new ServiceClient({ service: 'control', url: config.controlUrl, socketFactory: this.controlFactory(config) });
		this.analyze = new AnalyzeState(this);
		this.project = new ProjectState(this);
		this.fit = new FitState(this);
		this.report = new ReportState(this);
		this.units = new UnitsState(this);
		this.runLog = new RunLogState(this.analysis);
		this.console = new ConsoleState(this);
		this.sim = new SimState(this);
		// A display window observes: it never takes the main window's lease or edits a setup.
		this.acquire = new AcquireState(this.control, config.popout ? { session: null, local: null, drafts: false } : {});
		this.tests = new TestTypeView(() => this.liveTest);
		this.display = new DisplayState(this.control, {
			testType: 'random',
			// The layout belongs to the setup the service holds, else to the one being edited here.
			setupTitle: () => this.acquire.held?.config.title ?? (config.popout ? null : this.acquire.setup.title),
			showing: () => config.popout || (this.workspace === 'acquire' && this.tests.screen === 'random' && this.acquire.tab === 'live'),
			page: config.page
		});
		this.railCollapsed = readSession(RAIL_KEY) === '1';
	}

	/** In mock mode, a display window shares the main window's mock service. */
	private controlFactory(config: ShellConfig) {
		if (!config.mock) return undefined;
		if (config.popout) {
			const shared = openerMock();
			if (shared) return shared;
		}
		const server = createControlServer(config.mockControl);
		shareMock(server, 110);
		return mockSocketFactory(server, 110);
	}

	toggleRail(): void {
		this.railCollapsed = !this.railCollapsed;
		writeSession(RAIL_KEY, this.railCollapsed ? '1' : null);
	}

	/**
	 * Open a display page in a window of its own, for another monitor: the same shell, showing that
	 * page, observing the run with a subscription of its own. Every window shows the run's state and
	 * keeps Stop and Abort. False when the browser refused to open it.
	 */
	openDisplayWindow(pageId: string): boolean {
		const q = new URLSearchParams(location.search);
		q.set('popout', '1');
		q.set('page', pageId);
		this.windows += 1;
		const w = window.open(`${location.pathname}?${q.toString()}#acquire`, `hertzforge-display-${Date.now()}-${this.windows}`, 'popup,width=1280,height=820');
		return w !== null;
	}

	/** A display window: the control service only, the run's state and one page. */
	startDisplayWindow(): void {
		this.workspace = 'acquire';
		$effect.root(() => {
			$effect(() => {
				if (this.control.status === 'open') void this.acquire.sync();
			});
		});
		this.display.watch();
		this.control.connect();
	}

	/** The test type whose run is live; null when none is; undefined until the control service has said. */
	get liveTest(): LiveTest {
		return liveTestOf(this.acquire, this.control.status);
	}

	/** The live run is the screen on show, so the top bar need not offer it. */
	get runOnScreen(): boolean {
		const live = this.tests.live;
		return live !== null && this.workspace === 'acquire' && this.tests.screen === live;
	}

	/** The SIM badge rule: simulated control data is on screen. */
	get simOnScreen(): boolean {
		return this.workspace === 'simulate' && this.sim.display?.source === 'SIM';
	}

	/** Where the control data on screen comes from, for the top bar's badge, or null. */
	get provenance(): { source: 'SIM' | 'HARDWARE'; cls: 'sim' | 'ni_simulated' | 'ni_real' } | null {
		if (this.workspace === 'simulate') return this.simOnScreen ? { source: 'SIM', cls: 'sim' } : null;
		// A live run is shown from every workspace (the top bar's run pill), so its source is too.
		// The test-type chooser shows no control data of its own.
		const onRandom = this.workspace === 'acquire' && this.tests.screen === 'random';
		if (onRandom || this.acquire.isLive) return this.acquire.source;
		return null;
	}

	start(): void {
		// an address from before the split (#analyze) opens the recordings' module
		const named = (hash: string) => {
			const h = hash.replace(/^#\/?/, '');
			return h === 'analyze' ? 'signals' : h;
		};
		const fromHash = named(location.hash);
		if (isWorkspace(fromHash)) this.go(fromHash);
		window.addEventListener('hashchange', () => {
			const h = named(location.hash);
			if (isWorkspace(h)) this.go(h);
		});

		$effect.root(() => {
			// Fetch the catalog whenever the analysis service (re)connects.
			$effect(() => {
				if (this.analysis.status === 'open') void this.loadCatalog();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.sim.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.acquire.sync();
			});
			$effect(() => {
				const ws = this.workspace;
				if (location.hash !== `#${ws}`) history.replaceState(null, '', `#${ws}`);
			});
		});

		this.acquire.watch();
		this.tests.watch();
		this.display.watch();
		this.analysis.connect();
		this.control.connect();
	}

	go(workspace: Workspace): void {
		const entering = workspace === 'acquire' && this.workspace !== 'acquire';
		this.workspace = workspace;
		if (entering) this.tests.open();
	}

	/** A test type's screen in Acquire & Control, if it can be chosen now. */
	openTest(id: TestTypeId): boolean {
		this.go('acquire');
		return this.tests.choose(id);
	}

	/** The live run's screen, from anywhere in the shell (the top bar's run pill). */
	showRun(): void {
		this.tests.showRun();
		this.go('acquire');
	}

	async loadCatalog(): Promise<void> {
		try {
			this.catalog = await this.analysis.request<Catalog>('catalog.list');
			this.catalogError = null;
		} catch (err) {
			this.catalogError = message(err);
		}
	}
}

const RAIL_KEY = 'hertzforge.rail.collapsed';

function readSession(key: string): string | null {
	try {
		return typeof sessionStorage === 'undefined' ? null : sessionStorage.getItem(key);
	} catch {
		return null;
	}
}

function writeSession(key: string, value: string | null): void {
	try {
		if (typeof sessionStorage === 'undefined') return;
		if (value === null) sessionStorage.removeItem(key);
		else sessionStorage.setItem(key, value);
	} catch {
		/* storage refused; the rail just does not stay minimized across reloads */
	}
}

let instance: AppState | null = null;

export function getApp(): AppState {
	instance ??= new AppState(readConfig(typeof location === 'undefined' ? '' : location.search));
	return instance;
}
