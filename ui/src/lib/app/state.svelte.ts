/**
 * Shell state: the two service clients, the catalog, and each live workspace's working state.
 * Workspace components mount and unmount as the user moves; their state lives here so nothing
 * is lost on the way back.
 */

import { DisplayState } from '$lib/acquire/display/display.svelte';
import { AcquireState } from '$lib/acquire/store.svelte';
import { liveTestOf, TestTypeView, type LiveTest, type TestTypeId } from '$lib/acquire/testTypes.svelte';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { createControlServer } from '$lib/protocol/mock/control';
import { openerMock, shareMock } from '$lib/protocol/mock/shared';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type {
	Catalog,
	DatasetInfo,
	DisplayEvent,
	OpenedDataset,
	OperationDescriptor,
	OpResult,
	ParamDescriptor,
	FitStateView,
	ProjectProgress,
	ProjectSummary,
	RunState,
	Shape3DView,
	SimProfile,
	SimStateResult,
	View,
	Workspace
} from '$lib/protocol/types';
import { defaultValues, hasObjectsFor, reconcile, validateAll, withObjects, withSettings, type ParamValues } from '$lib/forms/params';
import { baseName, isProjectFile } from '$lib/project/files';
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
 * The connection's own project (`project.*`): what it holds, and the file work under way. Opening
 * or importing switches Analyze to it.
 */
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

	start(frf: string, shapes?: string): Promise<boolean> {
		this.range = null;
		return this.step('Starting the fit', 'fit.start', shapes ? { frf, shapes } : { frf });
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

class ProjectState {
	summary = $state.raw<ProjectSummary | null>(null);
	/** What is being done, in words ("Importing modal.nc4"), or null. */
	busy = $state<string | null>(null);
	progress = $state.raw<ProjectProgress | null>(null);
	error = $state<string | null>(null);

	constructor(private readonly app: AppState) {
		app.analysis.on('project.progress', (data) => {
			if (this.busy) this.progress = data as unknown as ProjectProgress;
		});
	}

	/** A new summary from the service: the tree and Analyze's object choices follow. */
	apply(summary: ProjectSummary): void {
		this.summary = summary;
		this.app.analyze.projectChanged();
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

	rename(name: string, to: string): Promise<boolean> {
		return this.call(`Renaming ${name}`, 'project.rename', { name, to }, false);
	}

	remove(names: string[]): Promise<boolean> {
		return this.call(`Removing ${names.join(', ')}`, 'project.remove', { names }, false);
	}

	activate(geometry: string): Promise<boolean> {
		return this.call(`Drawing on ${geometry}`, 'project.activate', { geometry }, false);
	}

	private async call(doing: string, method: string, params: Record<string, unknown>, show = true): Promise<boolean> {
		this.busy = doing;
		this.progress = null;
		this.error = null;
		try {
			const res = await this.app.analysis.request<{ project: ProjectSummary }>(method, params, { timeoutMs: FILE_TIMEOUT_MS });
			this.apply(res.project);
			if (show) {
				this.app.workspace = 'analyze';
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
