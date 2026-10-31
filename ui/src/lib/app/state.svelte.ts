/**
 * Shell state: the two service clients, the catalog, and each live workspace's working state.
 * Workspace components mount and unmount as the user moves; their state lives here so nothing
 * is lost on the way back.
 */

import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type {
	Catalog,
	DatasetInfo,
	DisplayEvent,
	OpenedDataset,
	OperationDescriptor,
	OpResult,
	ParamDescriptor,
	RunState,
	Shape3DView,
	SimProfile,
	SimStateResult,
	View,
	Workspace
} from '$lib/protocol/types';
import { defaultValues, reconcile, validateAll, type ParamValues } from '$lib/forms/params';
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
}

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

	get datasets(): DatasetInfo[] {
		return (this.app.catalog?.datasets ?? []).filter((d) => d.kind === 'random_run');
	}

	/** Tools follow the data: only operations that accept the open dataset's kind. */
	get tools(): OperationDescriptor[] {
		const kind = this.opened?.kind;
		if (!kind) return [];
		return (this.app.catalog?.operations ?? []).filter((o) => o.accepts === kind);
	}

	get op(): OperationDescriptor | null {
		return this.tools.find((o) => o.id === this.opId) ?? null;
	}

	get currentValues(): ParamValues {
		return (this.opId && this.values[this.opId]) || {};
	}

	get errors(): Record<string, string> {
		const op = this.op;
		if (!op) return {};
		return validateAll(op.params, this.currentValues, this.opened?.channels ?? []);
	}

	async selectDataset(id: string): Promise<void> {
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
		if (op && !this.values[op.id]) this.values[op.id] = defaultValues(op.params, this.opened?.channels ?? []);
	}

	resetParams(): void {
		const op = this.op;
		if (op) this.values[op.id] = defaultValues(op.params, this.opened?.channels ?? []);
		this.triedRun = false;
	}

	async run(): Promise<void> {
		const op = this.op;
		const opened = this.opened;
		if (!op || !opened || this.running) return;
		this.triedRun = true;
		if (Object.keys(this.errors).length) return;
		const params = { ...this.currentValues };
		this.running = true;
		this.runError = null;
		try {
			const res = await this.app.analysis.request<OpResult>('op.run', { op: op.id, handle: opened.handle, params }, { timeoutMs: 30_000 });
			this.results = [
				{
					id: this.nextResult++,
					opId: op.id,
					opTitle: op.title,
					datasetTitle: opened.title,
					paramSummary: summarize(op.params, params, opened.channels),
					views: res.views,
					at: new Date()
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

	removeResult(id: number): void {
		this.results = this.results.filter((r) => r.id !== id);
	}

	clearResults(): void {
		this.results = [];
	}
}

function summarize(params: ParamDescriptor[], values: ParamValues, channels: { value: string | number; label: string }[]): string {
	return params
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

export class AppState {
	readonly config: ShellConfig;
	readonly analysis: ServiceClient;
	readonly control: ServiceClient;

	workspace = $state<Workspace>('analyze');
	paletteOpen = $state(false);
	catalog = $state.raw<Catalog | null>(null);
	catalogError = $state<string | null>(null);

	readonly analyze: AnalyzeState;
	readonly modal: ModalState;
	readonly sim: SimState;

	constructor(config: ShellConfig) {
		this.config = config;
		const analysisFactory = config.mock ? mockSocketFactory(createAnalysisServer(), 80) : undefined;
		const controlFactory = config.mock ? mockSocketFactory(createControlServer(), 110) : undefined;
		this.analysis = new ServiceClient({ service: 'analysis', url: config.analysisUrl, socketFactory: analysisFactory });
		this.control = new ServiceClient({ service: 'control', url: config.controlUrl, socketFactory: controlFactory });
		this.analyze = new AnalyzeState(this);
		this.modal = new ModalState(this);
		this.sim = new SimState(this);
	}

	/** The SIM badge rule: simulated control data is on screen. */
	get simOnScreen(): boolean {
		return this.workspace === 'simulate' && this.sim.display?.source === 'SIM';
	}

	start(): void {
		const fromHash = location.hash.replace(/^#\/?/, '');
		if (isWorkspace(fromHash)) this.workspace = fromHash;
		window.addEventListener('hashchange', () => {
			const h = location.hash.replace(/^#\/?/, '');
			if (isWorkspace(h)) this.workspace = h;
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
				const ws = this.workspace;
				if (location.hash !== `#${ws}`) history.replaceState(null, '', `#${ws}`);
			});
		});

		this.analysis.connect();
		this.control.connect();
	}

	go(workspace: Workspace): void {
		this.workspace = workspace;
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

let instance: AppState | null = null;

export function getApp(): AppState {
	instance ??= new AppState(readConfig(typeof location === 'undefined' ? '' : location.search));
	return instance;
}
