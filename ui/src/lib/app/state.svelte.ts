/**
 * Shell state: the two service clients, the catalog, and each live workspace's working state.
 * Workspace components mount and unmount as the user moves; their state lives here so nothing
 * is lost on the way back.
 */

import { DisplayState } from '$lib/acquire/display/display.svelte';
import { EMPTY, clickObject, clickPart, clickRecord, pruned, renamed, type Modifiers, type Selection } from '$lib/project/selection';
import { AcquireState } from '$lib/acquire/store.svelte';
import { liveTestOf, TestTypeView, type LiveTest, type TestTypeId } from '$lib/acquire/testTypes.svelte';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { createControlServer } from '$lib/protocol/mock/control';
import { openerMock, shareMock } from '$lib/protocol/mock/shared';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ProjectChildren,
	Catalog,
	DatasetInfo,
	DisplayEvent,
	OpenedDataset,
	OperationDescriptor,
	OpResult,
	ParamDescriptor,
	FitStateView,
	ProjectProgress,
	UnitsTables,
	ProjectSummary,
	ReportEditor,
	RunState,
	Shape3DView,
	SimProfile,
	Pick3D,
	Region,
	SimStateResult,
	TableEdit,
	View,
	Workspace
} from '$lib/protocol/types';
import { defaultValues, hasObjectsFor, objectKinds, objectVerb, reconcile, validateAll, withObjects, withSettings, type ParamValues } from '$lib/forms/params';
import { baseName, isProjectFile } from '$lib/project/files';
import { RunLogState } from '$lib/runlog/store.svelte';
import { theme } from '$lib/theme/theme.svelte';
import { readConfig, type ShellConfig } from './config';
import { isWorkspace } from './workspaces';

function message(err: unknown): string {
	if (err instanceof RequestError) return err.message;
	if (err instanceof Error) return err.message;
	return String(err);
}

// ---------------------------------------------------------------------------------------------

export interface AnalyzeResult {
	id: number;
	opId: string;
	opTitle: string;
	datasetTitle: string;
	/** "Channel Control 1, Tolerance 3 dB" */
	paramSummary: string;
	views: View[];
	at: Date;
	/** What it ran with, so a choice made in its view can run it again. */
	params: ParamValues;
	handle: string;
	/** A rerun from the view is under way. */
	busy?: boolean;
}

/** The dataset id Analyze uses for the connection's own project. */
export const PROJECT = 'project';

class AnalyzeState {
	datasetId = $state<string | null>(null);
	opened = $state.raw<OpenedDataset | null>(null);
	opening = $state(false);
	openError = $state<string | null>(null);
	opId = $state<string | null>(null);
	/** Form values per operation id, kept while the dataset stays open. */
	values = $state<Record<string, ParamValues>>({});
	triedRun = $state(false);
	running = $state(false);
	runError = $state<string | null>(null);
	results = $state.raw<AnalyzeResult[]>([]);
	private nextResult = 1;
	private openSeq = 0;

	constructor(private readonly app: AppState) {}

	/** Every dataset some Analyze tool accepts: a new kind of data shows up here once a tool takes it. */
	get datasets(): DatasetInfo[] {
		const catalog = this.app.catalog;
		const kinds = new Set((catalog?.operations ?? []).filter((o) => o.workspace === 'analyze').map((o) => o.accepts));
		return (catalog?.datasets ?? []).filter((d) => kinds.has(d.kind));
	}

	/**
	 * Tools follow the data: only operations that accept the open dataset's kind, and on the project
	 * only those with an object to run on.
	 */
	get tools(): OperationDescriptor[] {
		const kind = this.opened?.kind;
		if (!kind) return [];
		return (this.app.catalog?.operations ?? []).filter(
			(o) => o.accepts === kind && (kind !== PROJECT || hasObjectsFor(this.paramsOf(o)))
		);
	}

	get op(): OperationDescriptor | null {
		return this.tools.find((o) => o.id === this.opId) ?? null;
	}

	get onProject(): boolean {
		return this.datasetId === PROJECT;
	}

	/** An operation's params as the form shows them: object params list the project's objects. */
	paramsOf(op: OperationDescriptor): ParamDescriptor[] {
		return op.accepts === 'project' ? withObjects(op.params, this.app.project.summary?.objects ?? []) : op.params;
	}

	get params(): ParamDescriptor[] {
		return this.op ? this.paramsOf(this.op) : [];
	}

	get currentValues(): ParamValues {
		return (this.opId && this.values[this.opId]) || {};
	}

	get errors(): Record<string, string> {
		const op = this.op;
		if (!op) return {};
		return validateAll(this.paramsOf(op), this.currentValues, this.opened?.channels ?? []);
	}

	/** Work on the connection's own project, as a dataset whose tools take its objects. */
	selectProject(): void {
		const summary = this.app.project.summary;
		if (!summary) return;
		++this.openSeq;
		this.datasetId = PROJECT;
		this.opening = false;
		this.openError = null;
		this.projectChanged();
		if (!this.op) this.selectOp(this.tools[0]?.id ?? null);
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

	/** The project's objects changed: its title, and every form's object choices, follow. */
	projectChanged(): void {
		const summary = this.app.project.summary;
		if (!this.onProject) return;
		if (!summary) {
			this.datasetId = null;
			this.opened = null;
			return;
		}
		this.opened = { handle: '', kind: 'project', title: summary.title, summary: {}, channels: [] };
		const next = { ...this.values };
		for (const op of this.tools) next[op.id] = reconcile(this.paramsOf(op), this.values[op.id]);
		this.values = next;
		if (!this.op) this.selectOp(this.tools[0]?.id ?? null);
	}

	async selectDataset(id: string): Promise<void> {
		if (id === PROJECT) return this.selectProject();
		if (this.datasetId === id && this.opened) return;
		const seq = ++this.openSeq;
		this.datasetId = id;
		this.opening = true;
		this.openError = null;
		try {
			const opened = await this.app.analysis.request<OpenedDataset>('dataset.open', { id });
			if (seq !== this.openSeq) return;
			this.opened = opened;
			// Re-fit every remembered form to the new dataset's channels.
			const next: Record<string, ParamValues> = {};
			for (const op of this.tools) next[op.id] = reconcile(op.params, this.values[op.id], opened.channels);
			this.values = next;
			if (!this.op) this.selectOp(this.tools[0]?.id ?? null);
		} catch (err) {
			if (seq !== this.openSeq) return;
			this.opened = null;
			this.openError = message(err);
		} finally {
			if (seq === this.openSeq) this.opening = false;
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
		if (op && !this.values[op.id]) this.values[op.id] = defaultValues(this.paramsOf(op), this.opened?.channels ?? []);
	}

	resetParams(): void {
		const op = this.op;
		if (op) this.values[op.id] = defaultValues(this.paramsOf(op), this.opened?.channels ?? []);
		this.triedRun = false;
	}

	async run(): Promise<void> {
		const op = this.op;
		const opened = this.opened;
		if (!op || !opened || this.running) return;
		this.triedRun = true;
		if (Object.keys(this.errors).length) return;
		const params = { ...this.currentValues };
		const shown = this.paramsOf(op);
		this.running = true;
		this.runError = null;
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', target(op, opened.handle, params), { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			this.results = [
				{
					id: this.nextResult++,
					opId: op.id,
					opTitle: op.title,
					datasetTitle: opened.title,
					paramSummary: summarize(shown, params, opened.channels),
					views: res.views,
					at: new Date(),
					params,
					handle: opened.handle
				},
				...this.results
			];
			this.triedRun = false;
		} catch (err) {
			this.runError = message(err);
		} finally {
			this.running = false;
		}
	}

	/**
	 * Run a result's operation again with `patch` over the params it ran with, as a choice made in
	 * its view asks (an `in_view` param), and show the new views in the same card.
	 */
	async rerun(id: number, patch: ParamValues): Promise<void> {
		const result = this.results.find((r) => r.id === id);
		const op = this.app.catalog?.operations.find((o) => o.id === result?.opId);
		if (!result || !op || result.busy) return;
		const params = { ...result.params, ...patch };
		const replace = (changes: Partial<AnalyzeResult>) =>
			(this.results = this.results.map((r) => (r.id === id ? { ...r, ...changes } : r)));
		replace({ busy: true });
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', target(op, result.handle, params), { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			replace({ views: res.views, params, paramSummary: summarize(this.paramsOf(op), params, this.opened?.channels ?? []), busy: false });
		} catch (err) {
			replace({ busy: false });
			this.runError = message(err);
		}
	}

	/**
	 * Set one cell of a result's editable table (`TableEdit`): its operation runs, and the first table
	 * it answers with takes that view's place. The refusal, when there is one, comes back as words.
	 */
	async editCell(id: number, viewIndex: number, edit: TableEdit, row: number, column: string, value: string): Promise<string | null> {
		try {
			const res = await this.app.analysis.request<OpResult>(
				'op.run',
				{ op: edit.op, params: { ...edit.params, row: row + 1, column, value } },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			if (res.project) this.app.project.apply(res.project);
			const table = res.views.find((v) => v.view === 'table');
			if (table) {
				this.results = this.results.map((r) =>
					r.id === id ? { ...r, views: r.views.map((v, i) => (i === viewIndex ? table : v)) } : r
				);
			}
			return null;
		} catch (err) {
			return message(err);
		}
	}

	/** A span dragged on a result's plot: its operation runs with the new edges, and its views take the result's place. */
	async dragSpan(id: number, drag: NonNullable<Region['drag']>, x0: number, x1: number): Promise<void> {
		const result = this.results.find((r) => r.id === id);
		if (!result || result.busy) return;
		const replace = (changes: Partial<AnalyzeResult>) =>
			(this.results = this.results.map((r) => (r.id === id ? { ...r, ...changes } : r)));
		replace({ busy: true });
		try {
			const params = { ...drag.params, [drag.low]: x0, ...(drag.high ? { [drag.high]: x1 } : {}) };
			const res = await this.app.analysis.request<OpResult>('op.run', { op: drag.op, params }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			replace({ views: res.views, busy: false });
		} catch (err) {
			replace({ busy: false });
			this.runError = message(err);
		}
	}

	/** A click in a result's 3-D view: the view's pick operation runs with what was clicked, and its views take the result's place. */
	async pickIn(id: number, pick: Pick3D, patch: Record<string, unknown>): Promise<void> {
		const result = this.results.find((r) => r.id === id);
		if (!result || result.busy) return;
		const replace = (changes: Partial<AnalyzeResult>) =>
			(this.results = this.results.map((r) => (r.id === id ? { ...r, ...changes } : r)));
		replace({ busy: true });
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: pick.op, params: { ...pick.params, ...patch } }, { timeoutMs: RUN_TIMEOUT_MS });
			if (res.project) this.app.project.apply(res.project);
			replace({ views: res.views, busy: false });
		} catch (err) {
			replace({ busy: false });
			this.runError = message(err);
		}
	}

	removeResult(id: number): void {
		this.results = this.results.filter((r) => r.id !== id);
	}

	clearResults(): void {
		this.results = [];
	}
}

/** How long an operation may take: fitting or computing on a large project is not quick. */
const RUN_TIMEOUT_MS = 120_000;

/** `op.run`'s params: a project operation names no handle, since it runs on the connection's project. */
function target(op: OperationDescriptor, handle: string, params: ParamValues): Record<string, unknown> {
	return op.accepts === 'project' ? { op: op.id, params } : { op: op.id, handle, params };
}

function summarize(params: ParamDescriptor[], values: ParamValues, channels: { value: string | number; label: string }[]): string {
	return params
		.filter((p) => !p.in_view)
		.map((p) => {
			const v = values[p.name];
			let text: string;
			if (p.type === 'enum') {
				const opts = p.options_from === 'channels' ? channels : (p.options ?? []);
				text = opts.find((o) => o.value === v)?.label ?? String(v);
			} else if (p.type === 'boolean') text = v ? 'on' : 'off';
			else if (p.type === 'string') text = v ? `"${v}"` : 'none';
			else text = `${v}${p.unit ? ` ${p.unit}` : ''}`;
			return `${p.label}: ${text}`;
		})
		.join(' \u00b7 ');
}

// ---------------------------------------------------------------------------------------------

class ModalState {
	datasetId = $state<string | null>(null);
	loading = $state(false);
	error = $state<string | null>(null);
	view = $state.raw<Shape3DView | null>(null);
	mode = $state(0);
	private seq = 0;

	constructor(private readonly app: AppState) {}

	get datasets(): DatasetInfo[] {
		return (this.app.catalog?.datasets ?? []).filter((d) => d.kind === 'modal_model');
	}

	get modesOp(): OperationDescriptor | null {
		return this.app.catalog?.operations.find((o) => o.id === 'modal.modes') ?? null;
	}

	/** Open the model and run `modal.modes` with its defaults. */
	async select(id: string): Promise<void> {
		if (this.datasetId === id && (this.view || this.loading)) return;
		const seq = ++this.seq;
		this.datasetId = id;
		this.loading = true;
		this.error = null;
		try {
			const opened = await this.app.analysis.request<OpenedDataset>('dataset.open', { id });
			const op = this.modesOp;
			if (!op) throw new Error('The analysis service does not offer modal.modes.');
			const res = await this.app.analysis.request<OpResult>(
				'op.run',
				{ op: op.id, handle: opened.handle, params: defaultValues(op.params, opened.channels) },
				{ timeoutMs: 30_000 }
			);
			if (seq !== this.seq) return;
			const shape = res.views.find((v): v is Shape3DView => v.view === 'shape3d') ?? null;
			if (!shape) throw new Error('modal.modes returned no shape3d view.');
			this.view = shape;
			this.mode = 0;
		} catch (err) {
			if (seq !== this.seq) return;
			this.view = null;
			this.error = message(err);
		} finally {
			if (seq === this.seq) this.loading = false;
		}
	}

	/** The project's shape set `shapes` on its active geometry (`project.mode_shapes`). */
	async selectShapes(shapes: string): Promise<void> {
		const geometry = this.app.project.summary?.active_geometry;
		if (!geometry) return;
		const seq = ++this.seq;
		this.datasetId = `project:${shapes}`;
		this.loading = true;
		this.error = null;
		try {
			const res = await this.app.analysis.request<OpResult>(
				'op.run',
				{ op: 'project.mode_shapes', params: { shapes, geometry } },
				{ timeoutMs: 30_000 }
			);
			if (seq !== this.seq) return;
			const shape = res.views.find((v): v is Shape3DView => v.view === 'shape3d') ?? null;
			if (!shape) throw new Error('project.mode_shapes returned no shape3d view.');
			this.view = shape;
			this.mode = 0;
		} catch (err) {
			if (seq !== this.seq) return;
			this.view = null;
			this.error = message(err);
		} finally {
			if (seq === this.seq) this.loading = false;
		}
	}
}

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
		return this.step('Starting the fit', 'fit.start', { frf, ...(shapes ? { shapes } : {}), ...(coherence ? { coherence } : {}) });
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
	/** Where the last export went. */
	exported = $state<string | null>(null);
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

	async export(path: string): Promise<void> {
		const name = this.name;
		if (!name) return;
		await this.act('Saving the page', async () => {
			const res = await this.app.analysis.request<{ path: string }>('report.export', { name, path }, { timeoutMs: RUN_TIMEOUT_MS });
			this.exported = res.path;
		});
	}

	/** The page being printed, in a frame of its own that asks for the print dialog. */
	printing = $state<string | null>(null);
	/** An issue under way: the report is marked issued and its page printed; the PDF is named next. */
	issuing = $state<string | null>(null);
	/** The last issue recorded. */
	issued = $state<Record<string, string> | null>(null);

	/** Print the reader's page; the print dialog saves it as a PDF. */
	async print(): Promise<void> {
		const name = this.name;
		if (!name) return;
		await this.act('Preparing the page to print', async () => {
			const res = await this.app.analysis.request<{ html: string }>('report.print', { name }, { timeoutMs: RUN_TIMEOUT_MS });
			this.printing = res.html;
		});
	}

	/** Begin, record or cancel an issue (`report.issue`). */
	async issue(step: 'begin' | 'record' | 'cancel', path?: string): Promise<void> {
		const name = this.name;
		if (!name) return;
		const doing = { begin: 'Marking the report issued', record: 'Recording the issue', cancel: 'Putting the report back' }[step];
		await this.act(doing, async () => {
			const res = await this.app.analysis.request<{ html?: string; status?: string; entry?: Record<string, string>; project: ProjectSummary }>(
				'report.issue',
				{ name, step, ...(path ? { path } : {}) },
				{ timeoutMs: RUN_TIMEOUT_MS }
			);
			this.app.project.apply(res.project);
			if (step === 'begin') {
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
			const res = await this.app.analysis.request<{ name: string; systems: string[] }>('units.system', {});
			this.systems = res.systems;
			if (wanted && wanted !== res.name && res.systems.includes(wanted)) {
				const set = await this.app.analysis.request<{ name: string }>('units.system', { name: wanted });
				this.name = set.name;
			} else this.name = res.name;
		} catch {
			// an older service without units: its own SI stands
		}
	}

	async choose(name: string): Promise<void> {
		if (name === this.name) return;
		const res = await this.app.analysis.request<{ name: string }>('units.system', { name });
		this.name = res.name;
		try {
			localStorage.setItem(UNITS_KEY, res.name);
		} catch {
			// not kept between launches; this session still uses it
		}
		// what is on screen was drawn in the old system: draw it again
		for (const r of this.app.analyze.results) void this.app.analyze.rerun(r.id, {});
		if (this.app.report.name) void this.app.report.show(this.app.report.name);
	}
}

/**
 * The connection's own project (`project.*`): what it holds, and the file work under way. Opening
 * or importing switches Analyze to it.
 */
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
		if (next.objects.length === 1) this.app.analyze.followSelection(next.objects[0]);
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
				// the workspaces with the project panel keep the window; any other goes to Analyze
				if (this.app.workspace !== 'model' && this.app.workspace !== 'specify') this.app.workspace = 'analyze';
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

export class AppState {
	readonly config: ShellConfig;
	readonly analysis: ServiceClient;
	readonly control: ServiceClient;

	workspace = $state<Workspace>('analyze');
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
	readonly modal: ModalState;
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
		this.modal = new ModalState(this);
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
		const fromHash = location.hash.replace(/^#\/?/, '');
		if (isWorkspace(fromHash)) this.go(fromHash);
		window.addEventListener('hashchange', () => {
			const h = location.hash.replace(/^#\/?/, '');
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
