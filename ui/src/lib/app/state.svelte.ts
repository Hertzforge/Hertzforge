/**
 * Shell state: the two service clients, the catalog, and each live workspace's working state.
 * Workspace components mount and unmount as the user moves; their state lives here so nothing
 * is lost on the way back.
 */

import { DisplayState } from '$lib/acquire/display/display.svelte';
import { bestCell, pairsText, togglePick } from '$lib/viz/macBars';
import { askToReplace, chooseFiles, chooseObjectFile, chooseSavePath, discardUnsaved, hasNativeFiles, openingOrder, pastedFiles, startDragOut } from '$lib/project/files';
import { EMPTY, clickObject, clickPart, clickRecord, partForService, pruned, renamed, type Modifiers, type Selection } from '$lib/project/selection';
import { AcquireState } from '$lib/acquire/store.svelte';
import { RunOpen } from '$lib/acquire/runOpen.svelte';
import { RunFiles } from './runFiles.svelte';
import { MimoState } from '$lib/acquire/mimo/mimoStore.svelte';
import { MimoViewsState } from '$lib/acquire/mimo/views/mimoViews.svelte';
import { ShockStore } from '$lib/acquire/shock/shockStore.svelte';
import { ShockViewsState } from '$lib/acquire/shock/shockViews.svelte';
import { MimoShockStore } from '$lib/acquire/mimoshock/msStore.svelte';
import { MsViewsState } from '$lib/acquire/mimoshock/msViews.svelte';
import { MimoSineStore } from '$lib/acquire/mimosine/msineStore.svelte';
import { MsineViewsState } from '$lib/acquire/mimosine/msineViews.svelte';
import { SrsStore } from '$lib/acquire/srs/srsStore.svelte';
import { SrsViewsState } from '$lib/acquire/srs/srsViews.svelte';
import { SineStore } from '$lib/acquire/sine/sineStore.svelte';
import { SineViewsState } from '$lib/acquire/sine/sineViews.svelte';
import { MixedStore } from '$lib/acquire/mixed/mixedStore.svelte';
import { MixedViewsState } from '$lib/acquire/mixed/mixedViews.svelte';
import { floorBand } from '$lib/acquire/mixed/mixedBand';
import { TwrStore } from '$lib/acquire/twr/twrStore.svelte';
import { TwrViewsState } from '$lib/acquire/twr/twrViews.svelte';
import { ExplorerStore } from '$lib/acquire/explorer/explorerStore.svelte';
import { ExplorerViewsState } from '$lib/acquire/explorer/explorerViews.svelte';
import { RecorderStore } from '$lib/acquire/recorder/recorderStore.svelte';
import { RecorderViewsState } from '$lib/acquire/recorder/recorderViews.svelte';
import { PlaybackStore } from '$lib/acquire/playback/playbackStore.svelte';
import { PlaybackViewsState } from '$lib/acquire/playback/playbackViews.svelte';
import { ModalTestStore } from '$lib/acquire/modal_test/modalTestStore.svelte';
import { liveTestOf, TestTypeView, type LiveTest, type TestTypeId } from '$lib/acquire/testTypes.svelte';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { createControlServer } from '$lib/protocol/mock/control';
import { openerMock, shareMock } from '$lib/protocol/mock/shared';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ActionColumn, ActionRow, Actions, ExportFormat, Region, ShapeView, ProjectChildren, ShowPrefs, Shown,
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
	ReportPageData,
	RunState,
	SimProfile,
	SimStateResult,
	TableEdit,
	TableView,
	View,
	Workspace, MatrixView } from '$lib/protocol/types';
import { defaultValues, hasObjectsFor, objectKinds, objectVerb, reconcile, rescaled, rescaledQuantities, validateAll, withDisplayLengths, withDisplayQuantities, withObjects, withSettings, type DisplayUnit, type ParamValues } from '$lib/forms/params';
import { baseName, confirmFirst, desktopRunFiles, isProjectFile, printsInBrowser } from '$lib/project/files';
import { RunLogState } from '$lib/runlog/store.svelte';
import { theme } from '$lib/theme/theme.svelte';
import { readConfig, type ShellConfig } from './config';
import { ANALYSIS, canOpen, isWorkspace, moduleOf, shownWorkspaces, STUDIES, type WorkspaceInfo } from './workspaces';

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
	/** How far the running operation has got (`op.progress`): a total of -1 is a stage that cannot count itself. */
	opProgress = $state<{ done: number; total: number } | null>(null);
	/** Cancel was asked of the running operation, which stops at its next report. */
	cancelling = $state(false);
	runError = $state<string | null>(null);
	/** What the tree's selection shows, in place of a result run by hand; null with nothing selected. */
	shown = $state.raw<Shown | null>(null);
	/** The bar's choices, remembered across launches. */
	prefs = $state<ShowPrefs>(readPrefs());
	/** The tables folded down to their headers, by what they list ('Nodes', 'Solids'), so the
	 * 3-D view beside them has the frame; remembered across launches. */
	folded = $state<string[]>(readFolded());
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
	/** A combination waiting on its last pick in the tree: an object of `kind`, or with `part`, another part of that kind. */
	picking = $state<{ verb: string; kind: string; row: ActionRow; part?: string } | null>(null);
	/** The row to run once the object it needed has joined the selection. */
	private joined: string | null = null;
	private showSeq = 0;
	private actionSeq = 0;

	constructor(private readonly app: AppState) {
		app.analysis.on('op.progress', (data) => {
			const d = data as unknown as { op: string; done: number; total: number };
			if (this.running && d.op === this.opId) this.opProgress = { done: d.done, total: d.total };
		});
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
		let params = withObjects(op.params, this.app.project.summary?.objects ?? []);
		if (op.params.some((p) => p.quantity) && !this.undefinedGeometry(op)) params = withDisplayQuantities(params, this.app.units.quantities);
		const length = this.app.units.length;
		if (!length || !op.params.some((p) => p.length)) return params;
		return withDisplayLengths(params, this.undefinedGeometry(op) ? null : length.unit);
	}

	/**
	 * Whether the geometry an operation's lengths are typed on has no units, its numbers its own: the
	 * form's geometry, or else the one the first object it names is drawn on, as the service takes them.
	 */
	private undefinedGeometry(op: OperationDescriptor): boolean {
		const summary = this.app.project.summary;
		const objects = summary?.objects ?? [];
		const values = this.values[op.id] ?? {};
		const named = 'geometry' in values ? [values.geometry] : op.params.filter((p) => p.options_from).map((p) => values[p.name]);
		for (const name of named) {
			const obj = objects.find((o) => o.name === name);
			if (!obj) continue;
			const geometry =
				obj.kind === 'geometry' ? obj : (objects.find((o) => o.kind === 'geometry' && o.groups.some((g) => obj.groups.includes(g))) ?? objects.find((o) => o.name === summary?.active_geometry));
			if (geometry) return geometry.units_defined === false;
		}
		return false;
	}

	/**
	 * The display system changed: each form holds the same lengths, and the same amount of each
	 * quantity, in the new units (`ratio` and `ratios`: the old unit in the new).
	 */
	unitsChanged(ratio: number | null, ratios: Record<string, number>): void {
		const next = { ...this.values };
		for (const op of this.app.catalog?.operations ?? []) {
			if (!next[op.id] || this.undefinedGeometry(op)) continue;
			if (ratio !== null && op.params.some((p) => p.length)) next[op.id] = rescaled(op.params, next[op.id], ratio);
			if (op.params.some((p) => p.quantity)) next[op.id] = rescaledQuantities(op.params, next[op.id], ratios);
		}
		this.values = next;
	}

	get params(): ParamDescriptor[] {
		return this.op ? this.paramsOf(this.op) : [];
	}

	/** What the form asks: not the objects the selection already filled, and of a format only those the object can be written in. */
	/** The open form's name: a solid's Add, opened on a solid already made, is its Edit. */
	get formTitle(): string {
		const title = this.op?.title ?? '';
		return this.formRow?.live ? title.replace(/^Add /, 'Edit ') : title;
	}

	get formParams(): ParamDescriptor[] {
		const row = this.formRow;
		const values = this.currentValues;
		return this.params
			.filter((p) => !p.shown_with || values[p.shown_with[0]] === p.shown_with[1])
			.filter((p) => p.name !== this.op?.commits)
			.filter((p) => !p.in_view && !(p.options_from && row?.fill && p.name in row.fill) && !(row?.live && p.name === 'solid') && !row?.omit?.includes(p.name))
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
	async showSelection(redraw = false): Promise<void> {
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
			// each selection opens as it is: Projection is asked for again, not carried over
			// and so is a loosened node match: one pair's tolerance is not the next pair's
			if (this.prefs.projection || this.prefs.match_distance !== 0) this.prefs = { ...this.prefs, projection: false, match_distance: 0 };
		}
		// a redraw of the same selection (the Averaging form typed into) offers what it did
		if (!redraw) void this.loadActions();
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
					part: partForService(selection),
					prefs: {
						flat: this.prefs.flat,
						component: this.prefs.component,
						line: this.line,
						projection: this.prefs.projection,
						match_distance: this.prefs.match_distance,
						limits: this.prefs.limits,
						shape_view: this.shapeView,
						values_dofs: this.prefs.values_dofs,
						values_form: this.prefs.values_form,
						arrows: this.prefs.arrows,
						arrow_role: this.prefs.arrow_role,
						drive: this.prefs.drive,
						sensors: this.prefs.sensors,
						environment: this.prefs.environment,
						table_view: this.prefs.table_view,
						tone: this.prefs.tone,
						sine_reading: this.prefs.sine_reading,
						averaging: this.averagingDraft
					}
				},
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			if (seq !== this.showSeq) return;
			this.shown = res;
			// two sets on two geometries: both are seen moving from the start, on the best pair;
			// it is shown, not picked, so Add Matches keeps only what was clicked
			const apart = res.views.some((v) => v.view === 'shape3d' && (v.overlay_triangles !== undefined || v.overlay_lines !== undefined));
			const compared = res.views.find((v): v is MatrixView => v.view === 'matrix' && v.selects === 'shape3d');
			if (apart && compared && !this.pair) this.pair = bestCell(compared.values, compared.columns.ticks.length);
			// the views bring the averaging's frames themselves
			this.averagingRegions = null;
			this.showError = null;
		} catch (err) {
			if (seq !== this.showSeq) return;
			this.shown = null;
			this.showError = message(err);
		}
	}

	/**
	 * What the Averaging form holds while it is open on the one time history selected, which the
	 * service draws on that history's plot (`prefs.averaging`); null otherwise.
	 */
	get averagingDraft(): Record<string, unknown> | null {
		const objects = this.app.project.selection.objects;
		const values = this.currentValues;
		if (!this.formOpen || this.opId !== 'project.set_averaging' || objects.length !== 1 || values.source !== objects[0]) return null;
		const draft: Record<string, unknown> = {};
		for (const key of ['frame_length', 'overlap', 'window', 'frames', 'start', 'detrend']) {
			if (values[key] !== undefined && values[key] !== null && values[key] !== '') draft[key] = values[key];
		}
		return draft;
	}

	/**
	 * The averaging's frames drawn over the views the selection already shows, in place of the
	 * views' own regions: asked for alone as the form is typed into (`project.averaging`), so the
	 * plot is not sent or built again. Null while the views' own regions stand.
	 */
	averagingRegions = $state.raw<Region[] | null>(null);
	private averagingKey = 'null';
	/** The form's values as last stored on the history, so a field left unchanged stores nothing. */
	private averagingStored = '';
	private averagingSeq = 0;
	private averagingTimer: ReturnType<typeof setTimeout> | null = null;

	/** The Averaging form opened, closed or typed into: once the typing rests, its frames are drawn again on the plot that is there. */
	followAveraging(): void {
		const draft = this.averagingDraft;
		const key = JSON.stringify(draft);
		if (key === this.averagingKey) return;
		const opening = this.averagingKey === 'null';
		// the form opens on what the history holds
		if (opening) this.averagingStored = JSON.stringify(this.currentValues);
		this.averagingKey = key;
		if (this.averagingTimer) clearTimeout(this.averagingTimer);
		if (draft === null) {
			// the form closed: its frames go
			this.averagingSeq++;
			if (this.shown) {
				this.averagingRegions = [];
				this.shown = { ...this.shown, note: '' };
			}
			return;
		}
		this.averagingTimer = setTimeout(() => void this.drawAveraging(), opening ? 0 : 250);
	}

	/** The frames of what the Averaging form holds, drawn over the plot that is there; the whole selection read when nothing is shown yet. */
	async drawAveraging(): Promise<void> {
		const draft = this.averagingDraft;
		const name = this.app.project.selection.objects[0];
		if (!draft || !name) return;
		if (!this.shown) return this.showSelection(true);
		const seq = ++this.averagingSeq;
		try {
			const res = await this.app.analysis.request<{ regions: Region[]; note: string }>('project.averaging', { name, averaging: draft });
			if (seq !== this.averagingSeq || !this.shown) return;
			this.averagingRegions = res.regions;
			this.shown = { ...this.shown, note: res.note };
		} catch (err) {
			if (seq === this.averagingSeq) this.runError = message(err);
		}
	}

	/** A draggable span on what the selection shows let go at new edges: its operation runs with them, and what it stored is drawn. */
	async spanOnSelection(drag: { op: string; params: Record<string, unknown>; low: string; high?: string }, x0: number, x1: number): Promise<void> {
		this.runError = null;
		// a field being typed in keeps its own text; let go of it, so it shows what the drag stores
		if (typeof document !== 'undefined' && document.activeElement instanceof HTMLInputElement) document.activeElement.blur();
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: drag.op, params: { ...drag.params, [drag.low]: x0, ...(drag.high ? { [drag.high]: x1 } : {}) } }, { timeoutMs: RUN_TIMEOUT_MS });
			// the plot is the same records: only the frames over it are drawn again, below
			if (res.project) this.app.project.apply(res.project, !this.averagingDraft);
		} catch (err) {
			this.runError = message(err);
		}
		// the form takes up what the drag stored, and its frames are drawn once: a drag that was
		// refused goes back to where it was
		this.followSource();
		if (this.averagingDraft) {
			this.averagingKey = JSON.stringify(this.averagingDraft);
			if (!this.runError) this.averagingStored = JSON.stringify(this.currentValues);
			if (this.averagingTimer) clearTimeout(this.averagingTimer);
			const refused = this.runError;
			await this.drawAveraging();
			if (refused) this.runError = refused;
		}
	}

	/** The row of the action table that runs the operation `id` on the selection, when it offers one. */
	private rowOf(id: string): ActionRow | null {
		for (const column of this.actions?.table.columns ?? []) {
			const row = column.current ? column.rows.find((r) => r.id === id) : undefined;
			if (row) return row;
		}
		return null;
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
				part: partForService(selection)
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
			// an open form follows the selection while the selection can still take it; only
			// what the selection now fills differently is filled again, so a value typed stays
			if (this.formOpen) {
				const row = current?.rows.find((r) => r.id === this.opId && r.ready);
				if (row) this.followRow(row);
				else this.closeForm();
			}
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
		if (row.instant && row.part) {
			await this.addThenEdit(row);
			return;
		}
		if (row.instant) {
			// made at once with its defaults; what it adds is picked in the tree
			this.openRow(row);
			await this.run();
			return;
		}
		if (!column.current && column.parts) {
			const part = column.parts[column.parts.length - 1];
			const picked = this.app.project.selection.part;
			const candidates = row.candidates ?? [];
			if (picked && candidates.length === 1) this.joinPart(picked.name, part, Number(candidates[0]), row);
			else this.picking = { verb: row.verb, kind: part === 'group' ? 'element_group' : part, row, part };
			return;
		}
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
		// a field the row fills is asked for nothing only when the form hides it: an object, or
		// one the row omits. A value it opens on, such as a projection's distance, is still asked
		const fill = row.fill ?? {};
		const asks = this.paramsOf(op).filter((p) => !p.in_view && !(p.name in fill && (p.options_from || row.omit?.includes(p.name))));
		// a form that applies as it goes is filled in full and opens all the same
		if (asks.length || row.live) this.formOpen = true;
		else await this.run();
		// a preview that is quick and asks nothing is shown as the form opens
		if (this.formOpen && op.commits && op.opens_previewed) await this.run(true);
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

	/** The open form's row as the actions now give it: the fields it fills differently from before are filled. */
	private followRow(row: ActionRow): void {
		const was = this.formRow?.id === row.id ? (this.formRow.fill ?? {}) : {};
		const fill = Object.fromEntries(Object.entries(row.fill ?? {}).filter(([key, text]) => was[key] !== text));
		this.pickAction({ id: row.id, fill });
		this.formRow = row;
	}

	/** Export in the desktop app: the native save window says where and in which format. */
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

	/**
	 * A shape added at once with what its row is filled with, then picked in the tree with its
	 * form open, which edits it from then on (Brandon, 2026-11-10: adding and editing are one).
	 */
	private async addThenEdit(row: ActionRow): Promise<void> {
		const p = this.app.project;
		const geometry = row.fill?.geometry;
		if (!geometry) return;
		const ids = (): number[] => {
			const held = p.children[geometry];
			return held?.kind === 'geometry' ? held.solids.map((s) => s.id) : [];
		};
		const before = new Set(ids());
		this.runError = null;
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: row.id, params: row.fill ?? {} }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) p.apply(res.project);
			await p.loadChildren(geometry);
		} catch (err) {
			this.runError = message(err);
			return;
		}
		const made = ids().find((id) => !before.has(id));
		if (made === undefined) return;
		this.joined = row.id;
		p.clickPart(geometry, 'solid', made);
	}

	/** A form that applies as it goes (`live`): what it holds now is run, and it is filled again from what the change left. */
	async applyLive(): Promise<void> {
		if (this.opId === 'project.set_averaging') {
			// the averaging is stored as it stands, and drawn on the plot that is there
			const now = JSON.stringify(this.currentValues);
			if (now === this.averagingStored) return;
			await this.run();
			if (!this.runError && !Object.keys(this.errors).length) this.averagingStored = JSON.stringify(this.currentValues);
			return;
		}
		await this.run();
		await this.loadActions();
	}

	/** The object a combination needed was chosen: it joins the selection, and the row runs once the table has caught up. */
	join(name: string, row: ActionRow): void {
		this.picking = null;
		this.joined = row.kind === 'default' ? null : row.id;
		const p = this.app.project;
		p.clickObject(name, p.summary?.objects.map((o) => o.name) ?? [], { toggle: true });
	}

	/** The part a combination of parts needed was chosen: it is picked with the others, and the row runs once the table has caught up. */
	joinPart(name: string, kind: string, id: string | number, row: ActionRow): void {
		this.picking = null;
		this.joined = row.id;
		this.app.project.clickPart(name, kind, id, { toggle: true });
	}

	/** Back from a form to the table; a preview it had drawn goes with it, since nothing ran. */
	closeForm(): void {
		this.formOpen = false;
		this.formRow = null;
		this.runError = null;
		this.triedRun = false;
		this.forgetPreview();
		this.dropPreview();
	}

	/** A row chosen in the action table: its tool, with the selection filled into its params. */
	pickAction(entry: { id: string; fill: Record<string, string> }): void {
		if (this.opId !== entry.id) this.selectOp(entry.id);
		const op = this.op;
		if (!op) return;
		const values = { ...this.currentValues };
		let changed = false;
		for (const [key, text] of Object.entries(entry.fill)) {
			const type = op.params.find((p) => p.name === key)?.type;
			const value = type === 'integer' || type === 'number' ? Number(text) : text;
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

	/** The frequency the stage that reads at the cursor's line is drawn at; null with no such stage. */
	private shownLine(): number | null {
		for (const view of this.shown?.views ?? []) {
			if (view.view === 'shape3d' && view.cursor === 'frequency') return view.modes[0]?.frequency_hz ?? null;
		}
		return null;
	}

	/**
	 * The distance two shape sets on two geometries are matched within, as the bar shows it: in the
	 * display system's length, or the model's own numbers when its geometries have no units. Null when
	 * the selection compares no such pair.
	 */
	get matchShown(): { value: number; unit: string } | null {
		const held = this.shown?.offers?.match_distance;
		if (!held) return null;
		const length = held.defined ? this.app.units.length : null;
		return { value: Number((held.value / (length?.to_si ?? 1)).toPrecision(4)), unit: length?.unit ?? '' };
	}

	/** A node match distance typed in the bar, in the units it shows it in, 0 for the default; false, and nothing asked, for one below 0. */
	setMatchDistance(typed: number): boolean {
		if (!Number.isFinite(typed) || typed < 0) return false;
		const held = this.shown?.offers?.match_distance;
		this.setPref('match_distance', typed * (held?.defined ? (this.app.units.length?.to_si ?? 1) : 1));
		return true;
	}

	/** One of the bar's choices changed: kept, and the selection read again. */
	setPref<K extends keyof ShowPrefs>(key: K, value: ShowPrefs[K]): void {
		// moving the force is the same question asked of another point: the line shown is kept,
		// where a cursor never moved would leave each force at its own strongest line
		if (key === 'drive' && this.line === null) this.line = this.shownLine();
		this.prefs = { ...this.prefs, [key]: value };
		try {
			localStorage.setItem(PREFS_KEY, JSON.stringify(this.prefs));
		} catch {
			// kept for this session only
		}
		// the shell's own choices redraw what is already here; the rest are the service's to answer
		if (key !== 'bars') void this.showSelection();
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
	/** The reading that is a preview, while it is the one drawn. */
	private previewDrawn: AnalyzeState['reading'] = null;

	/** Nothing still to come from a preview asked for. */
	private stopPreview(): void {
		if (this.previewTimer) clearTimeout(this.previewTimer);
		this.previewTimer = null;
		this.previewSeq++;
	}

	/** As `stopPreview`, and the same values typed again preview again. */
	private forgetPreview(): void {
		this.stopPreview();
		this.previewKey = '';
	}

	private dropPreview(): void {
		if (this.reading && this.reading === this.previewDrawn) this.endReading();
		this.previewDrawn = null;
	}

	/** Every field the form asks, but for what its row filled in, still holds its default. */
	private untouched(op: OperationDescriptor, values: ParamValues): boolean {
		const defaults = defaultValues(this.paramsOf(op));
		const filled = this.formRow?.fill ?? {};
		return this.formParams.every((p) => p.name in filled || values[p.name] === defaults[p.name]);
	}

	/** A form that previews, typed into: once the typing rests, what it would do is drawn, on a copy. */
	followPreview(): void {
		const open = this.formOpen;
		const op = this.op;
		const values = this.currentValues;
		if (!open || !op?.previews || !this.onProject || Object.keys(this.errors).length) {
			this.forgetPreview();
			return;
		}
		const key = `${op.id}\u0000${JSON.stringify(values)}`;
		if (key === this.previewKey) return;
		this.previewKey = key;
		this.stopPreview();
		const seq = this.previewSeq;
		this.previewTimer = setTimeout(async () => {
			try {
				const res = await this.app.analysis.request<OpResult>('op.run', { op: op.id, params: values, preview: true });
				if (seq !== this.previewSeq || this.opId !== op.id) return;
				this.reading = { title: `${this.formTitle}, preview`, views: res.views };
				this.previewDrawn = this.reading;
				this.readingId = op.id;
				this.runError = null;
			} catch (err) {
				if (seq !== this.previewSeq || this.opId !== op.id) return;
				this.dropPreview();
				// a form that waits for typing is refused as it opens: it has said nothing wrong yet
				const waiting = err instanceof RequestError && err.code === 'refused' && this.untouched(op, values);
				this.runError = waiting ? null : message(err);
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
			// a form that applies as it goes holds what the gimbal left
			if (this.formRow?.live) void this.loadActions();
			const next = res.views.find((v) => v.view === 'shape3d' && v.pick?.op === pick.op);
			if (next?.view === 'shape3d' && next.pick && this.values[pick.op]) {
				this.values = { ...this.values, [pick.op]: { ...this.values[pick.op], ...(next.pick.params as ParamValues) } };
			}
		} catch (err) {
			this.runError = message(err);
			this.redrawReading();
		}
	}

	/**
	 * The gimbal on what the selection shows (a solid picked in the tree) let go: its operation
	 * runs, and the selection is shown again, the gimbal where the solid now is.
	 */
	async dragOnSelection(act: { op: string; params: Record<string, unknown> }, patch: Record<string, unknown>): Promise<void> {
		this.runError = null;
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: act.op, params: { ...act.params, ...patch } }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			const geometry = act.params.geometry;
			if (typeof geometry === 'string') void this.app.project.loadChildren(geometry);
		} catch (err) {
			this.runError = message(err);
		}
		void this.showSelection();
	}

	/** No new view is coming: the same ones again, so a handle let go where it was refused goes back. */
	private redrawReading(): void {
		const was = this.reading;
		if (!was) return;
		this.reading = { ...was, views: was.views.map((v) => ({ ...v })) };
		if (was === this.previewDrawn) this.previewDrawn = this.reading;
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

	/**
	 * Apply the form. An operation that `commits` is sent its go-ahead param: false for Preview,
	 * which only shows what it would do, true for Apply, which does it.
	 */
	async run(preview = false): Promise<void> {
		const op = this.op;
		if (!op || !this.onProject || this.running) return;
		if (preview && !op.commits) return;
		this.triedRun = true;
		if (Object.keys(this.errors).length) return;
		const params = { ...this.currentValues };
		if (op.commits) params[op.commits] = !preview;
		this.running = true;
		this.opProgress = null;
		this.cancelling = false;
		this.runError = null;
		// what is typed now is run, so its preview must not land on top of the answer
		this.stopPreview();
		try {
			const before = new Set(this.app.project.summary?.objects.map((o) => o.name) ?? []);
			const res = await this.app.analysis.request<OpResult>('op.run', { op: op.id, params }, { timeoutMs: RUN_TIMEOUT_MS });
			// an averaging stored from the history's own plot leaves the plot as it is: only its frames are drawn again
			const sameplot = !preview && op.id === 'project.set_averaging' && !!this.shown && this.app.project.selection.objects.length === 1 && params.source === this.app.project.selection.objects[0];
			if (res.project) this.app.project.apply(res.project, !sameplot);
			// an act's result joins the tree and is selected; a reading
			// takes the view until Back
			const added = (res.project?.objects ?? []).map((o) => o.name).filter((n) => !before.has(n));
			const selected = this.app.project.selection.objects;
			const onPlot = selected.length === 1 && params.source === selected[0] && !!this.shown;
			if (added.length) {
				const order = (res.project?.objects ?? []).map((o) => o.name);
				this.app.project.clickObject(added[0], order);
				this.closeForm();
			} else if (!preview && onPlot && op.id === 'project.set_averaging') {
				// the frames are on the history's own plot: it stays, with the form, and draws what was stored
				this.followSource();
				this.averagingKey = JSON.stringify(this.averagingDraft);
				await this.drawAveraging();
			} else if (!preview && onPlot && op.id === 'project.detect_averaging' && this.rowOf('project.set_averaging')) {
				// what was found is shown where it can be seen and changed: the Averaging form, on the plot
				this.openRow(this.rowOf('project.set_averaging')!);
				this.formOpen = true;
				this.followSource();
			} else if (res.views.length) {
				this.reading = { title: preview ? `${this.formTitle}, preview` : this.formTitle, views: res.views };
				this.readingId = op.id;
			} else {
				this.closeForm();
			}
			this.triedRun = false;
		} catch (err) {
			this.runError = message(err);
		} finally {
			this.running = false;
			this.opProgress = null;
			this.cancelling = false;
		}
	}

	/** Stop the running operation at its next report of progress (`op.cancel`). */
	async cancel(): Promise<void> {
		if (!this.running || this.cancelling) return;
		this.cancelling = true;
		try {
			await this.app.analysis.request('op.cancel', {});
		} catch (err) {
			this.runError = message(err);
			this.cancelling = false;
		}
	}

	/** A field's value asked for by its request (`suggest`), with the form's values; null, and the reason said, when refused. */
	async suggest(p: ParamDescriptor): Promise<number | null> {
		if (!p.suggest) return null;
		try {
			const res = await this.app.analysis.request<{ value: number }>(p.suggest, { ...this.currentValues });
			this.runError = null;
			return res.value;
		} catch (err) {
			this.runError = message(err);
			return null;
		}
	}

	/** More rows of a long table that sent only its first ones (`view.more`). */
	async moreRows(more: { method: string; params: Record<string, unknown> }, start: number, count: number): Promise<string[][]> {
		const res = await this.app.analysis.request<{ rows: string[][] }>(more.method, { ...more.params, start, count });
		return res.rows;
	}

	/** A table folded to its header, or opened again. */
	toggleFolded(key: string): void {
		this.folded = this.folded.includes(key) ? this.folded.filter((k) => k !== key) : [...this.folded, key];
		try {
			localStorage.setItem(FOLDED_KEY, JSON.stringify(this.folded));
		} catch {
			// a private window keeps it for the session only
		}
	}

	/** Rows of a table of entities deleted (`edit.remove`): the project, the tree's counts and the tables follow. */
	async removeShownRows(remove: NonNullable<TableEdit['remove']>, ids: string[]): Promise<string | null> {
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: remove.op, params: { ...remove.params, ids: ids.join(', ') } }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			const geometry = remove.params.geometry;
			if (typeof geometry === 'string') void this.app.project.loadChildren(geometry);
			void this.showSelection();
			return null;
		} catch (err) {
			return message(err);
		}
	}

	/**
	 * A cell typed into in an action's table (a part's Edit): the edit runs, and the views it answers
	 * with take the reading's place, so the model drawn beside the table follows what was typed.
	 */
	async editReadingCell(edit: TableEdit, row: number, column: string, value: string): Promise<string | null> {
		const title = this.reading?.title ?? '';
		// a preview typed into is still the preview: closing its form takes it away
		const previewed = this.reading !== null && this.reading === this.previewDrawn;
		try {
			const res = await this.app.analysis.request<OpResult>(
				'op.run',
				{ op: edit.op, params: { ...edit.params, row: edit.rows ? edit.rows[row] : row + 1, column, value: inSi(edit, column, value) } },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			if (res.project) this.app.project.apply(res.project);
			this.reading = { title, views: res.views };
			if (previewed) this.previewDrawn = this.reading;
			const geometry = edit.params.geometry;
			if (typeof geometry === 'string') void this.app.project.loadChildren(geometry);
			return null;
		} catch (err) {
			return message(err);
		}
	}

	/** Rows of an action's table of entities deleted: the views answered take the reading's place. */
	async removeReadingRows(remove: NonNullable<TableEdit['remove']>, ids: string[]): Promise<string | null> {
		const title = this.reading?.title ?? '';
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: remove.op, params: { ...remove.params, ids: ids.join(', ') } }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			// a deletion may answer with a reading of what it kept: the drawn views and the table are what is edited
			this.reading = { title, views: res.views.filter((v) => v.view !== 'reading') };
			const geometry = remove.params.geometry;
			if (typeof geometry === 'string') void this.app.project.loadChildren(geometry);
			return null;
		} catch (err) {
			return message(err);
		}
	}

	/**
	 * One row picked in an action's table that says what a row picked does (`select`): its operation
	 * runs with the row's id, and the views answered that are not tables take the places of the ones
	 * beside the table, which stays as it is with its selection.
	 */
	async pickReadingRow(select: NonNullable<TableView['select']>, id: string): Promise<void> {
		const was = this.reading;
		if (!was) return;
		const number = Number(id);
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: select.op, params: { ...select.params, [select.param]: Number.isFinite(number) ? number : id } }, { timeoutMs: RUN_TIMEOUT_MS });
			if (this.reading !== was) return;
			const others = res.views.filter((v) => v.view !== 'table');
			let k = 0;
			this.reading = { title: was.title, views: was.views.map((v) => (v.view === 'table' ? v : (others[k++] ?? v))) };
		} catch (err) {
			this.runError = message(err);
		}
	}

	/** A cell typed into in what the selection shows: the edit runs, and the table it answers with takes the view's place. */
	async editShownCell(viewIndex: number, edit: TableEdit, row: number, column: string, value: string): Promise<string | null> {
		try {
			const res = await this.app.analysis.request<OpResult>(
				'op.run',
				{ op: edit.op, params: { ...edit.params, row: edit.rows ? edit.rows[row] : row + 1, column, value: inSi(edit, column, value) } },
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

const FOLDED_KEY = 'hertzforge.foldedTables';

function readFolded(): string[] {
	try {
		const held = JSON.parse(localStorage.getItem(FOLDED_KEY) ?? '[]');
		return Array.isArray(held) ? held.filter((k): k is string => typeof k === 'string') : [];
	} catch {
		return [];
	}
}

/** What a table lists, the part of its title before " of ": the key it is folded by. */
export function foldKey(title: string): string {
	return title.split(' of ')[0];
}

/** A value typed into a column shown in the display units, taken back to SI as the column's `to_si` says. */
export function inSi(edit: TableEdit, column: string, value: string): string {
	const change = edit.to_si?.[column];
	const number = Number(value);
	if (!change || value.trim() === '' || !Number.isFinite(number)) return value;
	return String(number * change[0] + change[1]);
}

/** The operation Delete runs for each part of a geometry it deletes, and the part in words. */
const PART_DELETES: Record<string, { op: string; param: string; words: string }> = {
	solid: { op: 'project.delete_solids', param: 'solids', words: 'solid' },
	group: { op: 'project.delete_element_groups', param: 'groups', words: 'element group' },
	mate: { op: 'project.delete_mated_surfaces', param: 'mates', words: 'mated surface' }
};

/** How long an operation may take: fitting or computing on a large project is not quick. */
const RUN_TIMEOUT_MS = 120_000;
/** How long a file dialog the service shows may stay open: it waits on the person, not on work. */
const DIALOG_TIMEOUT_MS = 3_600_000;

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
 * The mode fit under way on the project (`fit.*`): the fitting loop, one mode at a time
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
		// records picked in the tree limit the fit; an Edit Fit takes its set's own
		const records = shapes ? [] : (this.app.project.selection.records[frf] ?? []);
		return this.step('Starting the fit', 'fit.start', { frf, ...(shapes ? { shapes } : {}), ...(coherence ? { coherence } : {}), ...(records.length ? { records } : {}) });
	}

	find(): Promise<boolean> {
		return this.step('Finding a mode', 'fit.find', this.range ? { low: this.range[0], high: this.range[1] } : {});
	}

	/** `quiet` while a drag sends: no busy line flickering under the hand. */
	move(frequency: number, quiet = false): Promise<boolean> {
		return this.step('Moving the cursor', 'fit.move', { frequency }, quiet);
	}

	holdDamping(damping: number, quiet = false): Promise<boolean> {
		return this.step('Holding the damping', 'fit.damping', { damping }, quiet);
	}

	fit(quiet = false): Promise<boolean> {
		return this.step('Fitting the mode', 'fit.fit', {}, quiet);
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

	private async step(doing: string, method: string, params: Record<string, unknown>, quiet = false): Promise<boolean> {
		if (!quiet) this.busy = doing;
		this.error = null;
		try {
			const res = await this.app.analysis.request<{ fit: FitStateView; project?: ProjectSummary }>(method, params, { timeoutMs: RUN_TIMEOUT_MS });
			this.state = res.fit;
			// the fit is kept in the project as it goes: the tree shows the set as modes are confirmed
			if (res.project) this.app.project.apply(res.project);
			return true;
		} catch (err) {
			this.error = message(err);
			return false;
		} finally {
			if (!quiet) this.busy = null;
		}
	}
}

/**
 * The project's reports (`report.*`): the report, generated from a starter template and
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
	/** The report drawn by the shell's own viewers, from the last `report.views`. */
	page = $state.raw<ReportPageData | null>(null);
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

	/**
	 * Whether Print is offered: in a browser, which prints the page in place and saves it as a PDF
	 * from its dialog. The desktop app's web view does not print, so there the page would only be
	 * opened in the browser to be printed; the saved HTML file does the same, opened there by hand.
	 */
	get canPrint(): boolean {
		return !printsInBrowser();
	}

	/** Print the reader's page; the print dialog saves it as a PDF. */
	async print(anyway = false): Promise<void> {
		const name = this.name;
		if (!name || !this.canPrint) return;
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
			const res = await this.app.analysis.request<{ html?: string; opened?: string; status?: string; entry?: Record<string, string>; index?: { name: string; status: string }[]; project: ProjectSummary }>(
				'report.issue',
				{ name, step, ...(path ? { path } : {}), ...(anyway ? { anyway } : {}), ...(step === 'begin' && printsInBrowser() ? { browser: true } : {}) },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			this.app.project.apply(res.project);
			if (step === 'begin' && res.index) {
				this.held = { act: 'issue', owed: [], index: res.index };
			} else if (step === 'begin') {
				this.issuing = res.status ?? 'issued';
				this.issued = null;
				this.printing = res.opened ? null : (res.html ?? null);
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
			if (res.html !== undefined) await this.draw(name);
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
		await this.draw(name);
		this.exported = null;
	}

	/** The report `name` as the shell's own page draws it, in this connection's units. */
	private async draw(name: string): Promise<void> {
		this.page = await this.app.analysis.request<ReportPageData>('report.views', { name }, { timeoutMs: RUN_TIMEOUT_MS });
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
type LengthUnit = DisplayUnit;

/** What `units.system` answers. */
interface UnitsAnswer {
	name: string;
	systems: string[];
	length?: LengthUnit;
	quantities?: Record<string, DisplayUnit>;
}

/** Where the chosen unit system is kept between launches. */
const UNITS_KEY = 'hertzforge.units';

/**
 * The display unit system (`units.system`): every plot and reading the analysis service sends is
 * in it. Remembered between launches and
 * told to the service each time it connects; a change draws the results on screen again.
 */
export class UnitsState {
	name = $state<string | null>(null);
	systems = $state<string[]>([]);
	/** The system's length unit, and what takes it to meters: what a solid's form is typed in. */
	length = $state.raw<LengthUnit | null>(null);
	/** The system's unit for each quantity a param is typed in (`force/length`), and what takes it to SI. */
	quantities = $state.raw<Record<string, DisplayUnit>>({});

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
			const res = await this.app.analysis.request<UnitsAnswer>('units.system', {});
			this.systems = res.systems;
			const set = wanted && wanted !== res.name && res.systems.includes(wanted) ? await this.app.analysis.request<UnitsAnswer>('units.system', { name: wanted }) : res;
			this.name = set.name;
			this.length = set.length ?? null;
			this.quantities = set.quantities ?? {};
		} catch {
			// an older service without units: its own SI stands
		}
	}

	async choose(name: string): Promise<void> {
		if (name === this.name) return;
		const res = await this.app.analysis.request<UnitsAnswer>('units.system', { name });
		this.name = res.name;
		const before = this.length;
		const beforeQuantities = this.quantities;
		this.length = res.length ?? null;
		this.quantities = res.quantities ?? {};
		const ratios: Record<string, number> = {};
		for (const [q, now] of Object.entries(this.quantities)) if (beforeQuantities[q]) ratios[q] = beforeQuantities[q].to_si / now.to_si;
		this.app.analyze.unitsChanged(before && this.length ? before.to_si / this.length.to_si : null, ratios);
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
		component: 'magnitude',
		projection: false,
		match_distance: 0,
		limits: true,
		bars: true,
		values_dofs: 'all',
		values_form: 'shapes',
		arrows: false,
		arrow_role: 'all',
		drive: null,
		sensors: true,
		environment: null,
		table_view: 'channels',
		tone: null,
		sine_reading: 'curves'
	};
	try {
		const kept = JSON.parse(localStorage.getItem(PREFS_KEY) ?? '{}') as Partial<ShowPrefs>;
		if (typeof kept.flat === 'boolean') prefs.flat = kept.flat;
		// Projection is not kept: left on, two sets on two geometries open drawn on one of them
		for (const key of ['bars', 'arrows', 'sensors'] as const) {
			if (typeof kept[key] === 'boolean') prefs[key] = kept[key];
		}
		if (kept.values_form && ['shapes', 'response', 'reference'].includes(kept.values_form)) prefs.values_form = kept.values_form;
		if (kept.arrow_role && ['all', 'control', 'reference', 'response', 'monitor'].includes(kept.arrow_role)) prefs.arrow_role = kept.arrow_role;
		if (kept.component && ['magnitude', 'real', 'imag', 'phase'].includes(kept.component)) prefs.component = kept.component;
	} catch {
		// nothing kept, or no storage
	}
	return prefs;
}

/** What is renamed on its own row under an object in the tree. */
export type SubName = 'group' | 'solid' | 'mate' | 'mode' | 'channel';
const SUB_WORDS = { group: 'element group', solid: 'solid', mate: 'mated surfaces', mode: 'mode' } as const;

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
	/** The sub-item being renamed in place: its object, what it is and its id (a mode's number, a channel's row), or null. */
	renamingSub = $state<{ name: string; kind: SubName; id: number } | null>(null);
	/** The selection whose removal waits on a second click or Enter (Delete arms it from the tree). */
	confirming = $state(false);
	/** The group being named from the action table: the member it is reached by, or null. */
	namingGroup = $state<string | null>(null);
	/** The path a browser is asking for in the top bar, and what it is for. */
	asking = $state<'open' | 'import' | 'save' | null>(null);
	/** The analysis service said it shows no file dialogs (`files.choose`): the top bar asks for a path. */
	private noDialogs = false;
	/** The last record clicked in each object's grid, where a Shift extension starts. */
	private anchorRecord: Record<string, number> = {};
	private childSeq = 0;

	constructor(private readonly app: AppState) {
		app.analysis.on('project.progress', (data) => {
			if (this.busy) this.progress = data as unknown as ProjectProgress;
		});
	}

	/** A new summary from the service: the tree and Analyze's object choices follow. */
	apply(summary: ProjectSummary, show = true): void {
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
		// a change that leaves what is shown as it was (an averaging stored) does not read it again
		if (show) void this.app.analyze.showSelection();
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
		this.renamingSub = null;
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

	clickPart(name: string, kind: string, id: string | number, mods: Modifiers = {}, order: (string | number)[] = []): void {
		this.selected(clickPart(this.selection, name, kind, id, mods, order));
	}

	clearSelection(): void {
		this.selected(EMPTY);
	}

	/** Whether `name`, or one of its records, is selected. */
	isSelected(name: string): boolean {
		return this.selection.objects.includes(name);
	}

	/**
	 * Open or import each path, as a drop, a paste or File > Open gives them: the first project file
	 * opens (replacing the project, after asking when that loses work), and everything else is
	 * imported into it. Stops at the first refusal and says why.
	 */
	async openPaths(paths: string[]): Promise<void> {
		const { opens, imports } = openingOrder(paths);
		if (opens && !(await this.open(opens))) return;
		await this.importPaths(imports);
	}

	/** Import each path into the open project, a project file's objects among them. */
	async importPaths(paths: string[]): Promise<void> {
		for (const path of paths) {
			if (!(await this.importFile(path))) return;
		}
	}

	/** Open a project file in place of the project; false when the person keeps working instead. */
	async open(path: string): Promise<boolean> {
		if (this.summary?.modified && !(await askToReplace(this.summary.title, baseName(path)))) return false;
		return this.call(`Opening ${baseName(path)}`, 'project.open', { path });
	}

	/**
	 * A control run's folder into the analysis side: opened as the project when the project holds
	 * nothing yet, otherwise imported into it. The service converts the folder; the objects it
	 * brought are selected in the tree. Resolves with their names; a refusal is passed on.
	 */
	async takeRun(path: string): Promise<string[]> {
		const s = this.summary;
		const fresh = !s || (!s.file && s.objects.length === 0);
		this.busy = `Opening ${baseName(path)}`;
		this.progress = null;
		this.error = null;
		try {
			let added: string[];
			if (fresh) {
				const res = await this.app.analysis.request<{ project: ProjectSummary }>('project.open', { path }, { timeoutMs: FILE_TIMEOUT_MS });
				this.apply(res.project);
				added = res.project.objects.map((o) => o.name);
			} else {
				const res = await this.app.analysis.request<{ added: string[]; project: ProjectSummary }>('project.import', { path }, { timeoutMs: FILE_TIMEOUT_MS });
				this.apply(res.project);
				added = res.added;
			}
			const names = added.filter((n) => this.summary?.objects.some((o) => o.name === n));
			this.selected({ objects: names, records: {}, part: null, anchor: names[0] ?? null });
			return names;
		} finally {
			this.busy = null;
			this.progress = null;
		}
	}

	importFile(path: string, units?: Record<string, string>): Promise<boolean> {
		return this.call(`Importing ${baseName(path)}`, 'project.import', units ? { path, units } : { path });
	}

	save(path?: string): Promise<boolean> {
		return this.call('Saving', 'project.save', path ? { path } : {}, false);
	}

	/** An empty project in place of this one, after asking when this one has unsaved changes. */
	async newProject(): Promise<boolean> {
		if (!(await this.mayDiscard())) return false;
		return this.call('Starting a project', 'project.new', {}, false);
	}

	/** The service connected: every connection starts with an empty project, which the shell takes up. */
	async connected(): Promise<void> {
		if (this.summary) return;
		try {
			const res = await this.app.analysis.request<{ project: ProjectSummary }>('project.new', {});
			if (!this.summary) this.apply(res.project);
		} catch {
			// the first open or import says what is wrong
		}
	}

	private async mayDiscard(): Promise<boolean> {
		return !this.summary?.modified || (await discardUnsaved(this.summary.title));
	}

	/** The project's own name typed over on its row. */
	async retitle(title: string): Promise<boolean> {
		return this.call('Renaming the project', 'project.retitle', { title }, false);
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
	 * which never gives the shell a file's path, the analysis service shows the system's dialog on its
	 * own machine (`hertzforge` with no command), and where it shows none the top bar asks for a path.
	 */
	async fileAct(what: 'new' | 'open' | 'import' | 'save' | 'save-as'): Promise<void> {
		if (what === 'new') {
			await this.newProject();
			return;
		}
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
			if (paths) await (asked === 'import' ? this.importPaths(paths) : this.openPaths(paths));
			return;
		}
		// a browser never gives the shell a file's path: the service on this machine shows the
		// system's dialog instead, and where it shows none the top bar asks for a typed path
		const chosen = await this.dialogPaths(asked, asked === 'save' ? `${this.summary?.title ?? 'Project'}.hforge` : '');
		if (chosen !== null) {
			if (!chosen.length) return;
			if (asked === 'save') await this.save(chosen[0]);
			else await (asked === 'import' ? this.importPaths(chosen) : this.openPaths(chosen));
			return;
		}
		this.asking = this.asking === asked ? null : asked;
	}

	/**
	 * The paths chosen in the system's dialog the analysis service shows (`files.choose`): none when
	 * cancelled, null when the service shows none, which is remembered so the next act goes straight
	 * to the typed path.
	 */
	private async dialogPaths(purpose: 'open' | 'import' | 'save', name: string): Promise<string[] | null> {
		if (this.noDialogs) return null;
		try {
			const res = await this.app.analysis.request<{ paths: string[] }>('files.choose', name ? { purpose, name } : { purpose }, { timeoutMs: DIALOG_TIMEOUT_MS });
			return res.paths;
		} catch (err) {
			if (err instanceof RequestError && (err.code === 'refused' || err.code === 'unknown_method')) this.noDialogs = true;
			return null;
		}
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
		if (id === 'self.activate' && one) await this.activate(one);
		else if (id === 'self.units' && one) await this.toggleUnits(one);
		else if (id === 'self.recompute') await this.refresh(stale);
		else if (id === 'self.link') await this.link(picked);
		else if (id === 'self.unlink' && one) await this.unlink(one);
		else if (id === 'self.basis' && one) await this.setBasis(one);
		else if (id === 'self.name_group' && one) this.namingGroup = one;
		else if (id === 'self.copy') await this.duplicate(picked);
		else if (id === 'self.rename' && one) this.renaming = one;
		else if (id === 'self.fit' || id === 'self.edit_fit') {
			// weighted by the coherence the service chooses, with nothing asked (Brandon, 2026-11-14)
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

	/** Link the selected objects into one group; the group merges any they are in. */
	link(names: string[]): Promise<boolean> {
		const [first, second, ...more] = names;
		return this.act(`Linking ${names.join(', ')}`, 'project.link', { first, second, more: more.join(', '), name: '' });
	}

	unlink(name: string): Promise<boolean> {
		return this.act(`Unlinking ${name}`, 'project.unlink', { object: name });
	}

	/** The objects copied from the tree (Cmd-C), pasted as copies (Cmd-V). */
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
		// files copied elsewhere since: a copy here puts text on it and no file
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
	 * The project, or objects, dragged out of the window (Brandon, 2026-11-11: dragging the project
	 * out saves it): the service writes a copy, and the system's drag of it starts if the button
	 * is still held once it is written, so a drag let go meanwhile drops nothing. The project
	 * keeps its own file and its unsaved changes. False when nothing was dragged.
	 */
	async dragOut(what: { project: true } | { names: string[] }, stillHeld: () => boolean): Promise<boolean> {
		const label = 'project' in what ? `${this.summary?.title ?? 'Project'}.hforge` : what.names.length === 1 ? `${what.names[0]}.hforge` : `${what.names.length} objects`;
		try {
			const res = await this.app.analysis.request<{ paths: string[] }>('project.drag_out', what, { timeoutMs: RUN_TIMEOUT_MS });
			if (!stillHeld()) return false;
			return await startDragOut(res.paths, label);
		} catch (err) {
			this.error = `Dragging out ${label}: ${message(err)}`;
			return false;
		}
	}

	/**
	 * Objects dragged onto `target` join its group and leave their own;
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

	/** A DOF typed over in `name`'s grid: that channel (its quantity's) on the object and what derives
	 * from it. */
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

	/** How many parts an open object holds, as its row lists them; null when it is not open. */
	partCount(name: string): number | null {
		const kids = this.children[name];
		if (!kids) return null;
		switch (kids.kind) {
			case 'grid':
				return new Set(kids.cells.flat().filter((i) => i >= 0)).size;
			case 'modes':
				return kids.modes.length;
			case 'channels':
				return kids.channels.length;
			case 'photos':
				return kids.photos.length;
			case 'matches':
				return kids.matches.length;
			default:
				return null;
		}
	}

	/**
	 * The picked parts as the confirmation says them: "3 of A", or "all of B" when every part of
	 * B is picked, which deletes B itself (Brandon, 2026-11-13).
	 */
	partsSaid(parts: Record<string, number[]>): string {
		return Object.entries(parts)
			.map(([name, picks]) => (picks.length === this.partCount(name) ? `all of ${name}` : `${picks.length} of ${name}`))
			.join(', ');
	}

	/**
	 * What Delete takes with a part of a geometry picked: the solids, element groups or mated
	 * surfaces picked, never the geometry. Null for a part Delete leaves alone (the nodes, a
	 * family: their rows are deleted in their table).
	 */
	get partDeletion(): { name: string; op: string; param: string; ids: (string | number)[]; words: string } | null {
		const part = this.selection.part;
		const how = part ? PART_DELETES[part.kind] : undefined;
		if (!part || !how) return null;
		return { name: part.name, ...how, ids: part.ids ?? [part.id] };
	}

	/**
	 * Remove what is selected, once confirmed: the picked sub-items when any are picked (a
	 * co-selected object is context, not a target), else the objects.
	 */
	async removeSelected(): Promise<boolean> {
		const names = [...this.selection.objects];
		const parts = this.pickedParts;
		this.confirming = false;
		if (this.selection.part) {
			const gone = this.partDeletion;
			if (!gone) return false;
			const ok = await this.act(`Deleting ${gone.ids.length} ${gone.words}${gone.ids.length === 1 ? '' : 's'} of ${gone.name}`, gone.op, { geometry: gone.name, [gone.param]: gone.ids.join(', ') });
			if (ok) {
				this.selected({ objects: [gone.name], records: {}, part: null, anchor: gone.name });
				void this.loadChildren(gone.name);
			}
			return ok;
		}
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

	/** Rename one element group, solid or set of mated surfaces of a geometry, or describe one mode of a shape set, in place (`project.rename` with its kind). */
	async renameSub(name: string, kind: Exclude<SubName, 'channel'>, id: number, to: string): Promise<boolean> {
		const ok = await this.call(`Renaming ${SUB_WORDS[kind]} ${id} of ${name}`, 'project.rename', { name, to, [kind]: id }, false);
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
				// any other workspace goes to the analysis
				if (!STUDIES.has(this.app.workspace)) this.app.workspace = ANALYSIS;
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
 * The console: the session as Python along the bottom of the window. It
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
	/** The last workspace shown other than Report: the template Generate Report starts from. */
	lastModule = $state<Workspace | null>(null);
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
	/** A finished Random run's way to its data on the analysis side. */
	readonly runOpen: RunOpen;
	/** Where runs are kept, and Show Folder, Copy Folder Path and Delete Run for any run named. */
	readonly runFiles: RunFiles;
	/** A finished classical shock run's way to its data: the Shock workspace. */
	readonly shockRunOpen: RunOpen;
	/** A finished SRS shock run's way to its data: the Shock workspace. */
	readonly srsRunOpen: RunOpen;
	/** A finished MIMO Shock run's way to its data: the Shock workspace. */
	readonly mimoShockRunOpen: RunOpen;
	/** A finished MIMO Sine run's way to its data: the Sine workspace. */
	readonly mimoSineRunOpen: RunOpen;
	/** A finished Sine run's way to its data on the analysis side. */
	readonly sineRunOpen: RunOpen;
	/** A finished Mixed Mode run's way to its data on the analysis side. */
	readonly mixedRunOpen: RunOpen;
	/** A finished Explorer run's way to its data: its time histories, in the Signals workspace. */
	readonly explorerRunOpen: RunOpen;
	/** A finished MIMO Random run's way to its data: the Random workspace. */
	readonly mimoRunOpen: RunOpen;
	/** A finished TWR run's way to its data: the Transient Analysis workspace. */
	readonly twrRunOpen: RunOpen;
	readonly recorderRunOpen: RunOpen;
	/** A finished Playback run's way to its data: its inputs and drives, in the Signals workspace. */
	readonly playbackRunOpen: RunOpen;
	/** A finished Modal Test's way to its FRFs: the modal fit, in the Modal workspace. */
	readonly modalTestRunOpen: RunOpen;
	/** MIMO Random's setup and run; the station and table are the Acquire store's. */
	readonly mimo: MimoState;
	/** Acquire & Control's test types and which screen it shows. */
	readonly tests: TestTypeView;
	/** The Random run's display: its pages and plots, and the data this window asks for. */
	readonly display: DisplayState;
	/** The MIMO Random run's views: its pages and this window's subscription. */
	readonly mimoViews: MimoViewsState;
	/** Classical shock's setup and run; the station and table are the Acquire store's. */
	readonly shock: ShockStore;
	/** The classical shock run's pages and this window's subscription. */
	readonly shockViews: ShockViewsState;
	/** MIMO Shock's setup and run; the station and table are the Acquire store's. */
	readonly mimoShock: MimoShockStore;
	/** The MIMO Shock run's pages and this window's subscription. */
	readonly mimoShockViews: MsViewsState;
	/** MIMO Sine's setup and run; the station and table are the Acquire store's. */
	readonly mimoSine: MimoSineStore;
	/** The MIMO Sine run's pages and this window's subscription. */
	readonly mimoSineViews: MsineViewsState;
	/** SRS shock's setup and run; the station and table are the Acquire store's. */
	readonly srs: SrsStore;
	/** The SRS shock run's pages and this window's subscription. */
	readonly srsViews: SrsViewsState;
	/** Sine's setup and run; the station and table are the Acquire store's. */
	readonly sine: SineStore;
	/** The Sine run's pages and this window's subscription. */
	readonly sineViews: SineViewsState;
	/** Mixed Mode's setup and run; the station and table are the Acquire store's. */
	readonly mixed: MixedStore;
	/** The Mixed Mode run's pages and this window's subscription. */
	readonly mixedViews: MixedViewsState;
	/** Time waveform replication's setup and run; the station and table are the Acquire store's. */
	readonly twr: TwrStore;
	/** The TWR run's pages and this window's subscription. */
	readonly twrViews: TwrViewsState;
	/** Explorer's setup and run; the station and table are the Acquire store's. */
	readonly explorer: ExplorerStore;
	/** The Explorer run's pages and this window's subscription. */
	readonly explorerViews: ExplorerViewsState;
	/** The Recorder's setup and run; the station and table are the Acquire store's. */
	readonly recorder: RecorderStore;
	/** The Recorder run's pages and this window's subscription. */
	readonly recorderViews: RecorderViewsState;
	/** Playback's setup, drive file and run; the station and table are the Acquire store's. */
	readonly playback: PlaybackStore;
	/** The Playback run's pages, drawn from its live frames. */
	readonly playbackViews: PlaybackViewsState;
	/** The Modal Test's setup, its points, its hits and its live frame; the station and table (with each row's DOF) are the Acquire store's. */
	readonly modalTest: ModalTestStore;
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
		this.acquire = new AcquireState(this.control, config.popout ? { session: null, local: null, drafts: false } : { tables: this.analysis, onProject: (summary) => this.project.apply(summary) });
		const runHost = {
			bring: (folder: string) => this.project.takeRun(folder),
			show: (workspace: Workspace) => {
				this.go(workspace);
				this.analyze.selectProject();
			}
		};
		this.runOpen = new RunOpen(runHost);
		this.runFiles = new RunFiles({
			request: <R,>(method: string, params: Record<string, unknown>) => this.analysis.request<R>(method, params),
			control: (method: string, params: Record<string, unknown>) => this.control.request(method, params),
			desktop: config.mock ? null : desktopRunFiles(),
			confirm: confirmFirst
		});
		this.shockRunOpen = new RunOpen(runHost, 'classical_shock');
		this.srsRunOpen = new RunOpen(runHost, 'srs');
		this.mimoShockRunOpen = new RunOpen(runHost, 'mimo_shock');
		this.mimoSineRunOpen = new RunOpen(runHost, 'mimo_sine');
		this.sineRunOpen = new RunOpen(runHost, 'sine');
		this.mixedRunOpen = new RunOpen(runHost, 'mixed');
		this.explorerRunOpen = new RunOpen(runHost, 'explorer');
		this.mimoRunOpen = new RunOpen(runHost, 'mimo_random');
		this.twrRunOpen = new RunOpen(runHost, 'twr');
		this.recorderRunOpen = new RunOpen(runHost, 'recorder');
		this.playbackRunOpen = new RunOpen(runHost, 'playback');
		this.modalTestRunOpen = new RunOpen(runHost, 'modal_test');
		this.mimo = new MimoState(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.shock = new ShockStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.mimoShock = new MimoShockStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.mimoSine = new MimoSineStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.srs = new SrsStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.sine = new SineStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.mixed = new MixedStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.twr = new TwrStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.explorer = new ExplorerStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.recorder = new RecorderStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.modalTest = new ModalTestStore(this.acquire, config.popout ? { local: null, drafts: false } : {});
		this.playback = new PlaybackStore(this.acquire, config.popout ? { local: null, drafts: false } : { analysis: this.analysis, objects: () => this.project.summary?.objects ?? [] });
		this.tests = new TestTypeView(() => this.liveTest);
		this.display = new DisplayState(this.control, {
			// The layout belongs to the setup the service holds, else to the one being edited here.
			setupTitle: () => this.acquire.held?.config.title ?? (config.popout ? null : this.acquire.setup.title),
			showing: () => config.popout || (this.workspace === 'acquire' && this.tests.screen === 'random' && this.acquire.tab === 'live'),
			page: config.page
		});
		const mimoPopout = config.popout && config.popoutTest === 'mimo_random';
		this.mimoViews = new MimoViewsState(this.control, {
			setupTitle: () => this.mimo.status?.title ?? (config.popout ? null : this.mimo.setup.title),
			showing: () => mimoPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'mimo_random' && this.mimo.step === 'run'),
			page: mimoPopout ? config.page : null,
			windowIndex: mimoPopout ? 1 : 0
		});
		const shockPopout = config.popout && config.popoutTest === 'shock';
		this.shockViews = new ShockViewsState(
			this.control,
			{
				setupTitle: () => this.shock.status?.title ?? (config.popout ? null : this.shock.setup.title),
				showing: () => shockPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'shock' && this.shock.step === 'run'),
				page: shockPopout ? config.page : null
			},
			this.shock
		);
		const mimoShockPopout = config.popout && config.popoutTest === 'mimo_shock';
		this.mimoShockViews = new MsViewsState(
			this.control,
			{
				setupTitle: () => this.mimoShock.status?.title ?? (config.popout ? null : this.mimoShock.setup.title),
				showing: () => mimoShockPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'mimo_shock' && this.mimoShock.step === 'run'),
				page: mimoShockPopout ? config.page : null
			},
			this.mimoShock
		);
		const mimoSinePopout = config.popout && config.popoutTest === 'mimo_sine';
		this.mimoSineViews = new MsineViewsState(this.control, {
			setupTitle: () => this.mimoSine.status?.title ?? (config.popout ? null : this.mimoSine.setup.title),
			showing: () => mimoSinePopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'mimo_sine' && this.mimoSine.step === 'run'),
			page: mimoSinePopout ? config.page : null
		});
		const srsPopout = config.popout && config.popoutTest === 'srs';
		this.srsViews = new SrsViewsState(
			this.control,
			{
				setupTitle: () => this.srs.status?.title ?? (config.popout ? null : this.srs.setup.title),
				showing: () => srsPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'srs' && this.srs.step === 'run'),
				page: srsPopout ? config.page : null
			},
			this.srs
		);
		const sinePopout = config.popout && config.popoutTest === 'sine';
		this.sineViews = new SineViewsState(this.control, {
			setupTitle: () => this.sine.status?.title ?? (config.popout ? null : this.sine.setup.title),
			showing: () => sinePopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'sine' && this.sine.step === 'run'),
			page: sinePopout ? config.page : null
		});
		const mixedPopout = config.popout && config.popoutTest === 'mixed';
		this.mixedViews = new MixedViewsState(this.control, {
			setupTitle: () => this.mixed.status?.title ?? (config.popout ? null : this.mixed.setup.title),
			showing: () => mixedPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'mixed' && this.mixed.step === 'run'),
			page: mixedPopout ? config.page : null,
			runId: () => this.mixed.status?.run_id ?? null,
			band: () => floorBand(this.mixed.runConfig)
		});
		const twrPopout = config.popout && config.popoutTest === 'twr';
		this.twrViews = new TwrViewsState(this.control, {
			setupTitle: () => this.twr.status?.title ?? (config.popout ? null : this.twr.setup.title),
			showing: () => twrPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'twr' && this.twr.step === 'run'),
			page: twrPopout ? config.page : null
		});
		const explorerPopout = config.popout && config.popoutTest === 'explorer';
		this.explorerViews = new ExplorerViewsState(this.control, {
			setupTitle: () => this.explorer.status?.title ?? (config.popout ? null : this.explorer.setup.title),
			showing: () => explorerPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'explorer' && this.explorer.step === 'run'),
			page: explorerPopout ? config.page : null
		});
		const recorderPopout = config.popout && config.popoutTest === 'recorder';
		this.recorderViews = new RecorderViewsState(this.control, {
			setupTitle: () => this.recorder.status?.title ?? (config.popout ? null : this.recorder.setup.title),
			showing: () => recorderPopout || (!config.popout && this.workspace === 'acquire' && this.tests.screen === 'recorder' && this.recorder.step === 'run'),
			page: recorderPopout ? config.page : null
		});
		this.playbackViews = new PlaybackViewsState(this.control, {
			setupTitle: () => this.playback.now?.title ?? (config.popout ? null : this.playback.setup.title),
			showing: () => !config.popout && this.workspace === 'acquire' && this.tests.screen === 'playback' && this.playback.step === 'run',
			runId: () => this.playback.now?.run_id ?? null
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
	openDisplayWindow(pageId: string, test: 'random' | 'mimo_random' | 'shock' | 'srs' | 'sine' | 'mixed' | 'twr' | 'explorer' | 'recorder' | 'mimo_shock' | 'mimo_sine' = 'random'): boolean {
		const q = new URLSearchParams(location.search);
		q.set('popout', '1');
		q.set('page', pageId);
		if (test !== 'random') q.set('test', test);
		else q.delete('test');
		this.windows += 1;
		const w = window.open(`${location.pathname}?${q.toString()}#acquire`, `hertzforge-display-${Date.now()}-${this.windows}`, 'popup,width=1280,height=820');
		return w !== null;
	}

	/** A display window: the control service only, the run's state and one page. */
	startDisplayWindow(): void {
		this.workspace = 'acquire';
		const mimo = this.config.popoutTest === 'mimo_random';
		const shock = this.config.popoutTest === 'shock';
		const srs = this.config.popoutTest === 'srs';
		const sine = this.config.popoutTest === 'sine';
		const mixed = this.config.popoutTest === 'mixed';
		const twr = this.config.popoutTest === 'twr';
		const explorer = this.config.popoutTest === 'explorer';
		const recorder = this.config.popoutTest === 'recorder';
		const mimoShock = this.config.popoutTest === 'mimo_shock';
		const mimoSine = this.config.popoutTest === 'mimo_sine';
		$effect.root(() => {
			$effect(() => {
				if (this.control.status === 'open') void this.acquire.sync();
			});
			if (mimo) {
				$effect(() => {
					if (this.control.status === 'open') void this.mimo.sync();
				});
			}
			if (shock) {
				$effect(() => {
					if (this.control.status === 'open') void this.shock.sync();
				});
			}
			if (srs) {
				$effect(() => {
					if (this.control.status === 'open') void this.srs.sync();
				});
			}
			if (sine) {
				$effect(() => {
					if (this.control.status === 'open') void this.sine.sync();
				});
			}
			if (mixed) {
				$effect(() => {
					if (this.control.status === 'open') void this.mixed.sync();
				});
			}
			if (twr) {
				$effect(() => {
					if (this.control.status === 'open') void this.twr.sync();
				});
			}
			if (explorer) {
				$effect(() => {
					if (this.control.status === 'open') void this.explorer.sync();
				});
			}
			if (recorder) {
				$effect(() => {
					if (this.control.status === 'open') void this.recorder.sync();
				});
			}
			if (mimoShock) {
				$effect(() => {
					if (this.control.status === 'open') void this.mimoShock.sync();
				});
			}
			if (mimoSine) {
				$effect(() => {
					if (this.control.status === 'open') void this.mimoSine.sync();
				});
			}
		});
		if (mimo) this.mimoViews.watch();
		else if (shock) this.shockViews.watch();
		else if (srs) this.srsViews.watch();
		else if (sine) this.sineViews.watch();
		else if (mixed) this.mixedViews.watch();
		else if (twr) this.twrViews.watch();
		else if (explorer) this.explorerViews.watch();
		else if (recorder) this.recorderViews.watch();
		else if (mimoShock) this.mimoShockViews.watch();
		else if (mimoSine) this.mimoSineViews.watch();
		else this.display.watch();
		if (this.config.control) this.control.connect();
	}

	/** The test type whose run is live; null when none is; undefined until the control service has said. */
	get liveTest(): LiveTest {
		return liveTestOf(this.acquire, this.control.status, this.mimo, this.shock, this.srs, this.sine, this.twr, this.explorer, this.recorder, this.mixed, this.mimoShock, this.playback, this.mimoSine, this.modalTest);
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
		const onMimo = this.workspace === 'acquire' && this.tests.screen === 'mimo_random';
		if (onMimo || this.mimo.isLive) return this.mimo.source;
		const onShock = this.workspace === 'acquire' && this.tests.screen === 'shock';
		if (onShock || this.shock.isLive) return this.shock.source;
		const onSrs = this.workspace === 'acquire' && this.tests.screen === 'srs';
		if (onSrs || this.srs.isLive) return this.srs.source;
		const onSine = this.workspace === 'acquire' && this.tests.screen === 'sine';
		if (onSine || this.sine.isLive) return this.sine.source;
		const onMixed = this.workspace === 'acquire' && this.tests.screen === 'mixed';
		if (onMixed || this.mixed.isLive) return this.mixed.source;
		const onTwr = this.workspace === 'acquire' && this.tests.screen === 'twr';
		if (onTwr || this.twr.isLive) return this.twr.source;
		const onExplorer = this.workspace === 'acquire' && this.tests.screen === 'explorer';
		if (onExplorer || this.explorer.isLive) return this.explorer.source;
		const onRecorder = this.workspace === 'acquire' && this.tests.screen === 'recorder';
		if (onRecorder || this.recorder.isLive) return this.recorder.source;
		const onMimoShock = this.workspace === 'acquire' && this.tests.screen === 'mimo_shock';
		if (onMimoShock || this.mimoShock.isLive) return this.mimoShock.source;
		const onMimoSine = this.workspace === 'acquire' && this.tests.screen === 'mimo_sine';
		if (onMimoSine || this.mimoSine.isLive) return this.mimoSine.source;
		const onPlayback = this.workspace === 'acquire' && this.tests.screen === 'playback';
		if (onPlayback || this.playback.isLive) return this.playback.source;
		const onModal = this.workspace === 'acquire' && this.tests.screen === 'modal_test';
		if (onModal || this.modalTest.isLive) return this.modalTest.source;
		return null;
	}

	start(): void {
		// #analyze and #model are addresses the analysis had; any of its earlier ids opens it (`go`)
		const named = (hash: string) => {
			const h = hash.replace(/^#\/?/, '');
			return h === 'analyze' ? 'signals' : h === 'model' ? 'geometry' : h;
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
				if (this.control.status === 'open') void this.mimo.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.shock.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.srs.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.sine.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.mixed.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.twr.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.explorer.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.recorder.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.mimoShock.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.mimoSine.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.playback.sync();
			});
			$effect(() => {
				if (this.control.status === 'open') void this.modalTest.sync();
			});
			$effect(() => {
				const ws = this.workspace;
				if (location.hash !== `#${ws}`) history.replaceState(null, '', `#${ws}`);
			});
		});

		this.acquire.watch();
		this.mimo.watch();
		this.shock.watch();
		this.srs.watch();
		this.sine.watch();
		this.mixed.watch();
		this.twr.watch();
		this.explorer.watch();
		this.recorder.watch();
		this.mimoShock.watch();
		this.mimoSine.watch();
		this.playback.watch();
		this.modalTest.watch();
		this.tests.watch();
		this.display.watch();
		this.mimoViews.watch();
		this.shockViews.watch();
		this.srsViews.watch();
		this.sineViews.watch();
		this.mixedViews.watch();
		this.twrViews.watch();
		this.explorerViews.watch();
		this.recorderViews.watch();
		this.mimoShockViews.watch();
		this.mimoSineViews.watch();
		this.playbackViews.watch();
		this.analysis.connect();
		if (this.config.control) this.control.connect();
	}

	/** The rail's workspaces: without a control service, the analysis and the report alone. */
	get workspaces(): WorkspaceInfo[] {
		return shownWorkspaces(this.config.control);
	}

	go(workspace: Workspace): void {
		if (!canOpen(workspace, this.config.control)) workspace = ANALYSIS;
		const entering = workspace === 'acquire' && this.workspace !== 'acquire';
		this.workspace = moduleOf(workspace);
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
