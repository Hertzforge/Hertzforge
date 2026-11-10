/**
 * Acquire & Control: the Random run's working state, against the control service's hardware and
 * Random methods (PROTOCOL.md, "Hardware and Random Control").
 *
 * The service holds the truth: the controller seat, the station, the channel table (by
 * revision) and the run. This store mirrors it from the methods and the events, keeps the
 * operator's edits (station choice, channel table, setup) as drafts beside it, checks the drafts
 * with the service's dry runs, and sends them only when the operator says so. A refusal is kept
 * by its reason, for the screen to show in words.
 *
 * Nothing here draws or computes a measurement. Every number on screen comes from the service.
 */

import { regresses } from './runOrder';
import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type {
	ChannelRole,
	ChannelRow,
	ChannelsSetResult,
	ChannelTable,
	Derived,
	LadderMode,
	LogEntry,
	NiDevice,
	RandomConfig,
	RandomConfigResult,
	RandomConfigureResult,
	RandomDryRunResult,
	RandomLevelResult,
	RandomLiveHead,
	RandomLogEvent,
	RandomLogResult,
	RandomState,
	ReportItem,
	SessionChanged,
	SessionClaimResult,
	SessionReleaseResult,
	SessionState,
	SpectrumView,
	StationList,
	StationSelected,
	ValidationReport
} from '$lib/protocol/types';
import { CONTROL_TOPICS as T } from '$lib/protocol/types';
import { isLive, runActions, primaryAction, type ActionId, type ActionState, type RunContext, type SetupStatus } from './actions';
import { byField, configKey, defaultSetup, errorCount, fromConfig, localErrors, mergeErrors, toConfig, type FieldErrors, type SetupDraft } from './setup';
import { bindUnbound, nextIndex, newRow, rowFindings, sameRows, tableChanges } from './table';
import { toRefusal, type Refusal } from './words';

const LEASE_KEY = 'hertzforge.acquire.lease';
const SETUP_KEY = 'hertzforge.acquire.setup.v1';
/** The key of the service setup the editor's draft started from (applied here, or read back). */
const BASE_KEY = 'hertzforge.acquire.setup.base.v1';
/** Most log entries kept on screen; the service keeps the full log with the run. */
const LOG_LIMIT = 500;

export type AcquireTab = 'station' | 'test' | 'control' | 'live';

export interface Note {
	tone: 'info' | 'pass' | 'warn';
	text: string;
}

/** The service changed the table while this screen had edits of its own. */
export interface TableChange {
	revision: number;
	changes: string[];
	server: ChannelRow[];
}

/** What the service holds for the present run, read back with `random.config`. */
export interface HeldSetup {
	runId: string;
	config: RandomConfig;
	derived: Derived;
	/** `[reference]`: the profile and its bands on the requested grid, as the service drew them. */
	views: SpectrumView[];
	/** The configuration as the editor would hold it, and its key. */
	draft: SetupDraft;
	key: string;
}

export interface AcquireOptions {
	/** Where the lease (per tab) and the setup draft (per browser) are kept; null keeps nothing. */
	session?: Storage | null;
	local?: Storage | null;
	/** How other screens see this one when it holds control. */
	name?: string;
	/** Debounce of the dry-run checks, ms. */
	checkDelayMs?: number;
	/** Keep and check setup drafts (false for a window that only watches the run). */
	drafts?: boolean;
}

function storageOf(kind: 'sessionStorage' | 'localStorage'): Storage | null {
	try {
		return typeof globalThis[kind] === 'undefined' ? null : globalThis[kind];
	} catch {
		return null;
	}
}

export function clientName(): string {
	const p = typeof navigator === 'undefined' ? '' : navigator.platform || navigator.userAgent || '';
	const os = /Mac/i.test(p) ? 'macOS' : /Win/i.test(p) ? 'Windows' : /Linux/i.test(p) ? 'Linux' : 'this computer';
	return `Hertzforge on ${os}`;
}

function read(storage: Storage | null, key: string): string | null {
	try {
		return storage?.getItem(key) ?? null;
	} catch {
		return null;
	}
}

function write(storage: Storage | null, key: string, value: string | null) {
	try {
		if (!storage) return;
		if (value === null) storage.removeItem(key);
		else storage.setItem(key, value);
	} catch {
		/* storage refused; the draft just does not persist */
	}
}

/** JSON that keeps NaN (a field the operator has emptied) through a reload. */
const nanOut = (_k: string, v: unknown) => (typeof v === 'number' && Number.isNaN(v) ? '__NaN__' : v);
const nanIn = (_k: string, v: unknown) => (v === '__NaN__' ? NaN : v);

function loadSetup(storage: Storage | null): SetupDraft {
	const raw = read(storage, SETUP_KEY);
	const base = defaultSetup();
	if (!raw) return base;
	try {
		const saved = JSON.parse(raw, nanIn) as Partial<SetupDraft>;
		const merged = { ...base, ...saved } as SetupDraft;
		merged.control = { ...base.control, ...(saved.control ?? {}) };
		merged.control.pretest = { ...base.control.pretest, ...(saved.control?.pretest ?? {}) };
		merged.feasibility = { ...base.feasibility, ...(saved.feasibility ?? {}) };
		if (!Array.isArray(merged.breakpoints) || !Array.isArray(merged.schedule?.levels)) return base;
		return merged;
	} catch {
		return base;
	}
}

export class AcquireState {
	readonly control: ServiceClient;
	readonly name: string;
	private readonly sessionStore: Storage | null;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;

	// ---- session ----------------------------------------------------------------------------
	session = $state.raw<SessionState | null>(null);
	/** When the unattended run will be stopped (ms since the epoch), or null. */
	orphanDeadline = $state<number | null>(null);
	lease = $state<string | null>(null);

	// ---- station ----------------------------------------------------------------------------
	stations = $state.raw<StationList | null>(null);
	/** The station the operator is choosing in the panel, before it is sent. */
	stationDraft = $state<{ kind: 'sim' | 'ni'; devices: string[] }>({ kind: 'sim', devices: [] });
	scanning = $state(false);

	// ---- channel table ----------------------------------------------------------------------
	server = $state.raw<ChannelTable | null>(null);
	/** The operator's working copy; `base` is the revision it started from. */
	rows = $state<ChannelRow[]>([]);
	base = $state(0);
	baseRows = $state.raw<ChannelRow[]>([]);
	tableCheck = $state.raw<ValidationReport | null>(null);
	tableChange = $state.raw<TableChange | null>(null);

	// ---- setup ------------------------------------------------------------------------------
	setup = $state<SetupDraft>(defaultSetup());
	setupCheck = $state.raw<RandomDryRunResult | null>(null);
	checking = $state(false);
	/** The setup the service holds for the present run, whichever screen applied it. */
	held = $state.raw<HeldSetup | null>(null);
	/**
	 * The key of the service setup the draft started from. While the draft still equals it, the
	 * operator has changed nothing here, and a setup the service takes from another screen is
	 * loaded into the editor; once they differ, the operator's changes are kept.
	 */
	baseKey = $state('');

	// ---- run --------------------------------------------------------------------------------
	run = $state.raw<RandomState | null>(null);
	/** The run's numbers from the last live frame (`random.live`, or this screen's `random.data`). */
	live = $state.raw<RandomLiveHead | null>(null);
	log = $state.raw<LogEntry[]>([]);

	// ---- view -------------------------------------------------------------------------------
	/** The workspace's open section; kept here so it survives moving between workspaces. */
	tab = $state<AcquireTab>('station');

	// ---- feedback ---------------------------------------------------------------------------
	busy = $state<string | null>(null);
	refusal = $state.raw<Refusal | null>(null);
	#note = $state.raw<Note | null>(null);
	private noteTimer: ReturnType<typeof setTimeout> | null = null;

	/** A short note about what just happened. Good news fades after a few seconds; a warning stays. */
	get note(): Note | null {
		return this.#note;
	}

	set note(n: Note | null) {
		this.#note = n;
		if (this.noteTimer) clearTimeout(this.noteTimer);
		this.noteTimer = n && n.tone !== 'warn' ? setTimeout(() => (this.#note = null), 6000) : null;
	}
	/** The service does not offer these methods, or the first read failed. */
	syncError = $state<string | null>(null);

	private tableSeq = 0;
	private setupSeq = 0;
	private tableTimer: ReturnType<typeof setTimeout> | null = null;
	private setupTimer: ReturnType<typeof setTimeout> | null = null;
	private logNext = 0;
	/** The run the log on screen belongs to. */
	private logRun: string | null = null;
	/** The run whose setup is being read back, so one read is in flight per run. */
	private heldReading: string | null = null;
	/** The key of the setup the last dry run checked. */
	private checkedKey: string | null = null;

	constructor(control: ServiceClient, options: AcquireOptions = {}) {
		this.control = control;
		this.name = options.name ?? clientName();
		this.sessionStore = options.session === undefined ? storageOf('sessionStorage') : options.session;
		this.localStore = options.local === undefined ? storageOf('localStorage') : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 250;
		this.drafts = options.drafts ?? true;
		this.lease = read(this.sessionStore, LEASE_KEY);
		this.setup = loadSetup(this.localStore);
		this.baseKey = read(this.localStore, BASE_KEY) ?? configKey(toConfig(defaultSetup()));

		control.on(T.session, (d) => this.onSession(d as unknown as SessionChanged));
		control.on(T.station, (d) => this.onStations(d as unknown as StationList));
		control.on(T.channels, (d) => this.onTable(d as unknown as ChannelTable));
		control.on(T.state, (d) => this.onState(d as unknown as RandomState));
		control.on(T.live, (d) => this.onLive(d as unknown as RandomLiveHead));
		control.on(T.data, (d) => this.onLive(d as unknown as RandomLiveHead));
		control.on(T.log, (d) => this.onLog(d as unknown as RandomLogEvent));
	}

	// =========================================================================================
	// Derived state

	get connected(): boolean {
		return this.control.isOpen;
	}

	get role(): 'controller' | 'observer' {
		return this.session?.role ?? 'observer';
	}

	get isController(): boolean {
		return this.role === 'controller';
	}

	get selected(): StationSelected | null {
		return this.stations?.selected ?? null;
	}

	get niDevices(): NiDevice[] {
		return this.stations?.stations[1]?.devices ?? [];
	}

	get stationDirty(): boolean {
		const sel = this.selected;
		if (!sel) return false;
		if (this.stationDraft.kind !== sel.kind) return true;
		if (sel.kind === 'sim') return false;
		const a = [...this.stationDraft.devices].sort();
		const b = [...sel.devices].sort();
		return a.length !== b.length || a.some((x, i) => x !== b[i]);
	}

	readonly tableDirty = $derived(!sameRows($state.snapshot(this.rows) as ChannelRow[], this.baseRows));

	/** The report for what the operator sees: the dry run of the draft, else the service's. */
	get tableReport(): ValidationReport | null {
		return this.tableCheck ?? this.server?.report ?? null;
	}

	readonly findings = $derived(rowFindings(this.tableCheck ?? this.server?.report ?? null));

	readonly config = $derived(toConfig(this.setup));
	readonly setupKey = $derived(configKey(this.config));
	readonly local = $derived(localErrors(this.setup));

	/** Setup findings by field: the editor's own and the service's dry run. */
	get fieldErrors(): FieldErrors {
		return mergeErrors(this.local, byField(this.setupCheck?.errors ?? []));
	}

	get fieldWarnings(): FieldErrors {
		return byField(this.setupCheck?.warnings ?? []);
	}

	/** Errors that block applying: the editor's, the setup dry run's, and the service's table. */
	get setupErrorCount(): number {
		return errorCount(this.local) + (this.setupCheck?.errors.length ?? 0);
	}

	get tableErrorCount(): number {
		return this.tableReport?.errors.length ?? 0;
	}

	/**
	 * The derived numbers, from the service: while the editor holds the service's setup, the ones
	 * the service holds with it; otherwise its dry run of the draft.
	 */
	get derived(): Derived | null {
		if (this.setupStatus === 'applied' && this.held) return this.held.derived;
		return this.setupCheck?.derived ?? null;
	}

	/**
	 * The profile and its bands on the analysis lines, as the service draws them (never computed
	 * here): the service's own while the editor holds its setup, otherwise the dry run's of the
	 * draft. Null while the draft has errors.
	 */
	get reference(): SpectrumView | null {
		if (this.setupStatus === 'applied' && this.held?.views[0]) return this.held.views[0];
		return this.setupCheck?.views?.[0] ?? null;
	}

	get setupStatus(): SetupStatus {
		const run = this.run;
		if (!run || run.state === 'unconfigured') return 'none';
		const held = this.held;
		if (!held || held.runId !== run.run_id) return 'unread';
		return held.key === this.setupKey ? 'applied' : 'changed';
	}

	get isLive(): boolean {
		return isLive(this.run);
	}

	/** Setup, station and table are locked while a run is live. */
	get locked(): boolean {
		return this.isLive;
	}

	/** Why Apply, Arm and Start wait on this screen, or null. */
	get pending(): string | null {
		if (this.stationDirty) return 'Use the chosen station, or put the choice back, first.';
		if (this.tableDirty) return 'Apply the channel table edits, or discard them, first.';
		return null;
	}

	get context(): RunContext {
		return {
			connected: this.connected,
			role: this.role,
			run: this.run,
			setup: this.setupStatus,
			setupErrors: this.setupErrorCount,
			busy: this.busy,
			pending: this.pending
		};
	}

	get actions(): Record<ActionId, ActionState> {
		return runActions(this.context);
	}

	get primary(): ActionId | null {
		return primaryAction(this.context);
	}

	/** The live frame's numbers for the present run, or null. */
	get frame(): RandomLiveHead | null {
		const live = this.live;
		return live && this.run && live.run_id === this.run.run_id ? live : null;
	}

	/** Seconds since start: the live frame's while one streams, else the state's. */
	get elapsed(): number | null {
		return this.frame?.elapsed_s ?? this.run?.elapsed_s ?? null;
	}

	/** Where the data on screen comes from: the run's, else the selected station's. */
	get source(): { source: 'SIM' | 'HARDWARE'; cls: 'sim' | 'ni_simulated' | 'ni_real' } | null {
		const f = this.frame;
		if (f) return { source: f.source, cls: f.class };
		const run = this.run;
		if (run?.source && run.station) return { source: run.source, cls: run.station.class };
		const sel = this.selected;
		return sel ? { source: sel.source, cls: sel.class } : null;
	}

	// =========================================================================================
	// Wiring

	/** Re-check drafts as they change and keep the setup across reloads. Returns the stopper. */
	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				void JSON.stringify($state.snapshot(this.rows));
				untrack(() => this.scheduleTableCheck());
			});
			$effect(() => {
				void this.setupKey;
				const snapshot = $state.snapshot(this.setup);
				untrack(() => {
					this.scheduleSetupCheck();
					write(this.localStore, SETUP_KEY, JSON.stringify(snapshot, nanOut));
				});
			});
		});
	}

	/** Read everything the service holds; resume control if this screen held it before a drop. */
	async sync(): Promise<void> {
		const c = this.control;
		try {
			const [session, stations, table, run, held, log] = await Promise.all([
				c.request<SessionState>('session.state'),
				c.request<StationList>('station.list'),
				c.request<ChannelTable>('channels.get'),
				c.request<RandomState>('random.state'),
				c.request<RandomConfigResult>('random.config'),
				c.request<RandomLogResult>('random.log', { since: 0 })
			]);
			this.syncError = null;
			this.setSession(session);
			// After a reconnect, the operator's unsent station choice and table edits are kept;
			// a table that moved meanwhile is shown as a change, never merged.
			this.onStations(stations);
			if (!this.server || !this.tableDirty) this.adoptTable(table);
			else this.onTable(table);
			this.onHeld(held, run);
			this.onState(run);
			this.log = log.entries.slice(-LOG_LIMIT);
			this.logNext = log.next;
			this.logRun = run.run_id;
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		if (this.lease && this.role !== 'controller') await this.resume();
		if (!this.drafts) return;
		this.scheduleTableCheck(0);
		this.scheduleSetupCheck(0);
	}

	// =========================================================================================
	// Session

	private setSession(s: SessionState) {
		this.session = s;
		this.orphanDeadline = s.orphan_s === null ? null : Date.now() + s.orphan_s * 1000;
	}

	/** Each screen gets its own copy of the event, with its own role in it. */
	private onSession(d: SessionChanged) {
		const was = this.role;
		if (this.session) this.session = { ...this.session, role: d.role, controller: d.controller, orphan_s: d.orphan_s };
		this.orphanDeadline = d.orphan_s === null ? null : Date.now() + d.orphan_s * 1000;
		// A screen that let go on purpose dropped its lease first; any other loss is news.
		if (was === 'controller' && d.role !== 'controller' && this.lease !== null) {
			this.setLease(null);
			this.note = { tone: 'warn', text: 'This screen no longer holds control.' };
		}
	}

	private setLease(lease: string | null) {
		this.lease = lease;
		write(this.sessionStore, LEASE_KEY, lease);
	}

	async claim(): Promise<boolean> {
		const res = await this.act<SessionClaimResult>('Take Control', 'session.claim', { name: this.name, ...(this.lease ? { lease: this.lease } : {}) });
		if (!res) return false;
		this.setLease(res.lease);
		this.holdSeat(res);
		this.note = res.resumed ? { tone: 'pass', text: 'Control resumed.' } : null;
		return true;
	}

	/** This screen holds the seat; who holds it and the orphan clock come with `session.changed`. */
	private holdSeat(res: SessionClaimResult) {
		this.session = { ...(this.session ?? { controller: null, orphan_s: null }), heartbeat_s: res.heartbeat_s, timeout_s: res.timeout_s, role: 'controller' };
	}

	/** After a drop: present the lease. The seat comes back if it was kept for this screen. */
	private async resume(): Promise<void> {
		try {
			const res = await this.control.request<SessionClaimResult>('session.claim', { name: this.name, lease: this.lease });
			this.setLease(res.lease);
			this.holdSeat(res);
			this.note = res.resumed
				? { tone: 'pass', text: 'The connection dropped and came back. This screen still holds control.' }
				: { tone: 'warn', text: 'The connection dropped for longer than the service keeps a seat. This screen has taken control again.' };
		} catch (err) {
			this.setLease(null);
			const r = toRefusal('Resume Control', err);
			this.note = { tone: 'warn', text: `The connection dropped and control was not resumed. ${r.message}` };
		}
	}

	async release(): Promise<void> {
		// Drop the lease first: the seat's change event can arrive before this answer, and a
		// screen without a lease let go of control on purpose.
		const lease = this.lease;
		this.setLease(null);
		const res = await this.act<SessionReleaseResult>('Release Control', 'session.release');
		if (!res) {
			this.setLease(lease);
			return;
		}
		if (this.session) this.session = { ...this.session, role: res.role, orphan_s: res.orphan_s };
		this.note = null;
	}

	/** After a refusal for want of control: the screen's view of the seat was out of date. */
	private async refreshSession() {
		try {
			this.setSession(await this.control.request<SessionState>('session.state'));
		} catch {
			/* the next event or sync brings it */
		}
	}

	// =========================================================================================
	// Stations

	private onStations(list: StationList, force = false) {
		const before = this.stations?.selected;
		this.stations = list;
		const followed =
			force ||
			!before ||
			(this.stationDraft.kind === before.kind && JSON.stringify([...this.stationDraft.devices].sort()) === JSON.stringify([...before.devices].sort()));
		if (followed) this.stationDraft = { kind: list.selected.kind, devices: [...list.selected.devices] };
		this.scheduleSetupCheck();
	}

	chooseStation(kind: 'sim' | 'ni') {
		const sel = this.selected;
		this.stationDraft = { kind, devices: kind === 'ni' ? (sel?.kind === 'ni' ? [...sel.devices] : [...this.stationDraft.devices]) : [] };
	}

	toggleDevice(id: string, on?: boolean) {
		const has = this.stationDraft.devices.includes(id);
		const want = on ?? !has;
		if (want && !has) this.stationDraft.devices = [...this.stationDraft.devices, id];
		if (!want && has) this.stationDraft.devices = this.stationDraft.devices.filter((d) => d !== id);
	}

	revertStation() {
		const sel = this.selected;
		if (sel) this.stationDraft = { kind: sel.kind, devices: [...sel.devices] };
	}

	async rescan(): Promise<void> {
		this.scanning = true;
		try {
			const list = await this.act<StationList>('Rescan', 'station.list', { rescan: true });
			if (list) this.onStations(list);
		} finally {
			this.scanning = false;
		}
	}

	async useStation(): Promise<boolean> {
		const d = this.stationDraft;
		const params = d.kind === 'sim' ? { kind: 'sim' } : { kind: 'ni', devices: [...d.devices] };
		const res = await this.act<{ selected: StationSelected }>('Use Station', 'station.select', params);
		if (!res) return false;
		if (this.stations) this.stations = { ...this.stations, selected: res.selected };
		this.stationDraft = { kind: res.selected.kind, devices: [...res.selected.devices] };
		return true;
	}

	// =========================================================================================
	// Channel table

	private adoptTable(t: ChannelTable) {
		this.server = t;
		this.base = t.revision;
		this.baseRows = structuredClone(t.channels);
		this.rows = structuredClone(t.channels);
		this.tableChange = null;
		this.tableCheck = null;
	}

	private onTable(t: ChannelTable) {
		const prev = this.server;
		this.server = t;
		if (!this.tableDirty || (prev && sameRows($state.snapshot(this.rows) as ChannelRow[], t.channels))) {
			this.adoptTable(t);
		} else if (t.revision !== this.base) {
			// Never merged silently: the table carries the abort limits.
			this.tableChange = { revision: t.revision, changes: tableChanges(this.baseRows, t.channels), server: structuredClone(t.channels) };
		}
		this.scheduleTableCheck();
		this.scheduleSetupCheck();
	}

	addRow(role: ChannelRole) {
		this.rows.push(newRow(nextIndex(this.rows), role));
	}

	removeRow(position: number) {
		this.rows.splice(position, 1);
	}

	/** Bind the unbound rows to the selected devices' free channels, in order. */
	bindRows(): number {
		const sel = this.selected;
		if (!sel || sel.kind !== 'ni') return 0;
		const { rows, bound } = bindUnbound($state.snapshot(this.rows) as ChannelRow[], this.niDevices, sel.devices);
		this.rows = rows;
		this.note = { tone: bound ? 'info' : 'warn', text: bound ? `Bound ${bound} ${bound === 1 ? 'row' : 'rows'} to free channels. Check them, then apply the table.` : 'No free channel of the right direction was left to bind.' };
		return bound;
	}

	discardTable() {
		if (this.server) this.adoptTable(this.server);
	}

	/** Drop this screen's edits and load the service's table (after it changed underneath). */
	takeServerTable() {
		if (this.server) this.adoptTable(this.server);
	}

	/** Keep this screen's edits, now based on the service's present revision. The operator has seen what changed. */
	keepEditsOnServer() {
		const change = this.tableChange;
		if (!change) return;
		this.base = change.revision;
		this.baseRows = structuredClone(change.server);
		this.tableChange = null;
		this.scheduleTableCheck(0);
	}

	async applyTable(): Promise<boolean> {
		const rows = $state.snapshot(this.rows) as ChannelRow[];
		const res = await this.act<ChannelsSetResult>('Apply Table', 'channels.set', { revision: this.base, channels: rows });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'stale_revision') await this.showServerChange();
			if (r?.reason === 'table_invalid' && r.detail) this.tableCheck = { errors: (r.detail.errors as ReportItem[]) ?? [], warnings: (r.detail.warnings as ReportItem[]) ?? [] };
			return false;
		}
		this.base = res.revision;
		this.baseRows = rows;
		this.server = { revision: res.revision, channels: rows, report: res.report };
		this.tableCheck = res.report;
		this.tableChange = null;
		this.note = { tone: 'pass', text: `Channel table applied (revision ${res.revision}).` };
		return true;
	}

	/** Read the table again and show what changed; never retried on the operator's behalf. */
	private async showServerChange() {
		try {
			const t = await this.control.request<ChannelTable>('channels.get');
			this.server = t;
			this.tableChange = { revision: t.revision, changes: tableChanges(this.baseRows, t.channels), server: structuredClone(t.channels) };
		} catch {
			/* the refusal is already on screen */
		}
	}

	scheduleTableCheck(delay = this.checkDelayMs) {
		if (this.tableTimer) clearTimeout(this.tableTimer);
		this.tableTimer = setTimeout(() => void this.checkTable(), delay);
	}

	/** The service's verdict on the draft table (`dry_run`: no control needed, nothing written). */
	async checkTable(): Promise<ValidationReport | null> {
		if (this.tableTimer) clearTimeout(this.tableTimer);
		this.tableTimer = null;
		if (!this.connected || !this.server) return null;
		const seq = ++this.tableSeq;
		try {
			const res = await this.control.request<ChannelsSetResult>('channels.set', { revision: this.base, channels: $state.snapshot(this.rows), dry_run: true });
			if (seq === this.tableSeq) this.tableCheck = res.report;
			return res.report;
		} catch {
			return null;
		}
	}

	// =========================================================================================
	// Setup

	scheduleSetupCheck(delay = this.checkDelayMs) {
		if (this.setupTimer) clearTimeout(this.setupTimer);
		this.setupTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	/** The service's verdict on the draft setup and its derived numbers (`dry_run`). */
	async checkSetup(): Promise<RandomDryRunResult | null> {
		if (this.setupTimer) clearTimeout(this.setupTimer);
		this.setupTimer = null;
		if (!this.connected) return null;
		const seq = ++this.setupSeq;
		if (errorCount(this.local) > 0) {
			this.setupCheck = null;
			return null;
		}
		this.checking = true;
		const key = this.setupKey;
		try {
			const res = await this.control.request<RandomDryRunResult>('random.configure', { config: this.config, dry_run: true });
			if (seq === this.setupSeq) {
				this.setupCheck = res;
				this.checkedKey = key;
			}
			return res;
		} catch {
			return null;
		} finally {
			if (seq === this.setupSeq) this.checking = false;
		}
	}

	/** Put the editor back to the setup the service holds. */
	revertSetup() {
		const held = this.held;
		if (!held) return;
		this.setup = structuredClone(held.draft);
		this.setBase(held.key);
	}

	private setBase(key: string) {
		this.baseKey = key;
		write(this.localStore, BASE_KEY, key);
	}

	/** Send the setup (`random.configure`). It replaces what the service held. */
	async apply(): Promise<boolean> {
		const config = this.config;
		const key = this.setupKey;
		const draft = $state.snapshot(this.setup) as SetupDraft;
		const views = this.checkedKey === key ? (this.setupCheck?.views ?? []) : [];
		const res = await this.act<RandomConfigureResult>('Apply Setup', 'random.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				this.setupCheck = { errors: (r.detail.errors as ReportItem[]) ?? [], warnings: (r.detail.warnings as ReportItem[]) ?? [], derived: null, views: [] };
				this.checkedKey = key;
			}
			if (r?.reason === 'table_invalid' && r.detail) {
				this.tableCheck = { errors: (r.detail.errors as ReportItem[]) ?? [], warnings: (r.detail.warnings as ReportItem[]) ?? [] };
			}
			return false;
		}
		const runId = res.state.run_id;
		this.setBase(key);
		// What was sent is what the service holds; its own copy is read back to replace this one.
		if (runId) this.held = { runId, config, derived: res.derived, views, draft, key };
		this.onState(res.state);
		if (runId) void this.readHeld(runId, true);
		return true;
	}

	/** Read back what the service holds for a run (`random.config`). One read per run at a time. */
	private async readHeld(runId: string, again = false): Promise<void> {
		if (this.heldReading === runId || (!again && this.held?.runId === runId)) return;
		this.heldReading = runId;
		try {
			this.onHeld(await this.control.request<RandomConfigResult>('random.config'), this.run);
		} catch {
			/* the status stays unread; the next state event or sync reads it again */
		} finally {
			if (this.heldReading === runId) this.heldReading = null;
		}
	}

	/**
	 * The service's setup for the present run. The editor takes it when the operator has changed
	 * nothing here since the setup it last started from (so a second screen, or a screen that was
	 * reloaded, shows what the run uses); an operator's own changes are never replaced.
	 */
	private onHeld(r: RandomConfigResult, run: RandomState | null) {
		if (!r.run_id || !r.config || !r.derived) {
			this.held = null;
			return;
		}
		// An answer about a run that has since been replaced: that run's own read follows.
		if (run && run.run_id !== r.run_id) return;
		const draft = fromConfig(r.config);
		const key = configKey(toConfig(draft));
		this.held = { runId: r.run_id, config: r.config, derived: r.derived, views: r.views, draft, key };
		if (key === this.setupKey) {
			this.setBase(key);
		} else if (this.setupKey === this.baseKey) {
			this.setup = structuredClone(draft);
			this.setBase(key);
		}
	}

	// =========================================================================================
	// Run

	private onState(s: RandomState) {
		const prev = this.run;
		// A reply older than the run's own word that it finished is not applied.
		if (regresses(prev, s)) return;
		this.run = s;
		if (!s.run_id || s.state === 'unconfigured') this.held = null;
		else if (this.held?.runId !== s.run_id) void this.readHeld(s.run_id);
		// A new run starts with an empty display; a finished run keeps its last frame.
		if (prev && prev.run_id !== s.run_id) this.live = null;
		// When a run starts driving, the live view comes forward once.
		const driving = (x: RandomState | null) => !!x && (x.state === 'running' || x.state === 'review' || x.state === 'stopping');
		if (driving(s) && (!driving(prev) || prev?.run_id !== s.run_id)) this.tab = 'live';
	}

	private onLive(l: RandomLiveHead) {
		if (this.run && l.run_id !== this.run.run_id) return;
		// Only the numbers: a screen's plots read their own data from the display.
		const { views: _v, plots: _p, ...head } = l as RandomLiveHead & { views?: unknown; plots?: unknown };
		this.live = head;
	}

	/** Each run has its own log, started at configure and at every new run. */
	private onLog(e: RandomLogEvent) {
		const restart = e.run_id !== this.logRun;
		this.logRun = e.run_id;
		// An entry the first read already brought is not shown twice.
		const fresh = restart ? e.entries : e.entries.filter((x) => x.seq >= this.logNext);
		if (restart) {
			this.log = [];
			this.logNext = 0;
		}
		if (!fresh.length) return;
		this.log = [...this.log, ...fresh].slice(-LOG_LIMIT);
		this.logNext = fresh[fresh.length - 1].seq + 1;
	}

	async arm(overrideAck?: string): Promise<boolean> {
		const res = await this.act<RandomState>(overrideAck === undefined ? 'Arm' : 'Arm Past Feasibility', 'random.arm', overrideAck === undefined ? {} : { override_ack: overrideAck });
		if (res) this.onState(res);
		return !!res;
	}

	async start(): Promise<boolean> {
		const res = await this.act<RandomState>('Start', 'random.start');
		if (res) this.onState(res);
		return !!res;
	}

	async proceed(): Promise<boolean> {
		const res = await this.act<RandomState>('Proceed', 'random.proceed');
		if (res) this.onState(res);
		return !!res;
	}

	async level(deltaDb: number): Promise<number | null> {
		const res = await this.act<RandomLevelResult>(deltaDb > 0 ? 'Level Up' : 'Level Down', 'random.level', { delta_db: deltaDb });
		return res?.level_db ?? null;
	}

	async goTo(db: number): Promise<number | null> {
		const res = await this.act<RandomLevelResult>('Go to Level', 'random.level', { to_db: db });
		return res?.level_db ?? null;
	}

	async setMode(mode: LadderMode): Promise<boolean> {
		const res = await this.act<{ mode: LadderMode }>(mode === 'manual' ? 'Manual Ladder' : 'Automatic Ladder', 'random.mode', { mode });
		return !!res;
	}

	/** The controlled stop. Any connected screen may send it. Never waits on another request. */
	async stop(): Promise<boolean> {
		const res = await this.act<RandomState>('Stop', 'random.stop', {}, false);
		if (res) this.onState(res);
		return !!res;
	}

	/** Ends the run within one control loop, without the ramp. Any connected screen may send it. */
	async abort(): Promise<boolean> {
		const res = await this.act<RandomState>('Abort', 'random.abort', {}, false);
		if (res) this.onState(res);
		return !!res;
	}

	async reset(): Promise<boolean> {
		// A new run from the same setup: what the service held carries over to it.
		const held = this.held;
		const res = await this.act<RandomState>('New Run', 'random.reset');
		if (!res) return false;
		if (held && res.run_id && this.held === held) this.held = { ...held, runId: res.run_id };
		this.onState(res);
		return true;
	}

	async clear(): Promise<boolean> {
		const res = await this.act<RandomState>('Clear', 'random.clear');
		if (!res) return false;
		this.onState(res);
		return true;
	}

	dismissRefusal() {
		this.refusal = null;
	}

	dismissNote() {
		this.note = null;
	}

	/**
	 * One request with its refusal kept by reason. `exclusive` requests set `busy`; Stop and Abort
	 * are never held back by another request in flight, and never mark the screen busy.
	 */
	private async act<R>(verb: string, method: string, params: Record<string, unknown> = {}, exclusive = true): Promise<R | null> {
		if (exclusive) this.busy = verb;
		this.refusal = null;
		try {
			return await this.control.request<R>(method, params);
		} catch (err) {
			this.refusal = toRefusal(verb, err);
			if (this.refusal.reason === 'not_controller') void this.refreshSession();
			return null;
		} finally {
			if (exclusive && this.busy === verb) this.busy = null;
		}
	}
}
