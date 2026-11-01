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
	RandomConfigureResult,
	RandomDryRunResult,
	RandomLevelResult,
	RandomLive,
	RandomLogResult,
	RandomState,
	ReportItem,
	SessionChanged,
	SessionClaimResult,
	SessionReleaseResult,
	SessionState,
	StationList,
	StationSelected,
	ValidationReport
} from '$lib/protocol/types';
import { CONTROL_TOPICS as T } from '$lib/protocol/types';
import { isLive, runActions, primaryAction, type ActionId, type ActionState, type RunContext, type SetupStatus } from './actions';
import { byField, defaultSetup, errorCount, localErrors, mergeErrors, toConfig, type FieldErrors, type SetupDraft } from './setup';
import { bindUnbound, nextIndex, newRow, rowFindings, sameRows, tableChanges } from './table';
import { toRefusal, type Refusal } from './words';

const LEASE_KEY = 'hertzforge.acquire.lease';
const SETUP_KEY = 'hertzforge.acquire.setup.v1';
/** Most log entries kept on screen; the service keeps the full log with the run. */
const LOG_LIMIT = 500;

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

export interface AcquireOptions {
	/** Where the lease (per tab) and the setup draft (per browser) are kept; null keeps nothing. */
	session?: Storage | null;
	local?: Storage | null;
	/** How other screens see this one when it holds control. */
	name?: string;
	/** Debounce of the dry-run checks, ms. */
	checkDelayMs?: number;
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
	/** What this screen last applied, and to which run. */
	applied = $state.raw<{ key: string; runId: string | null; derived: Derived; warnings: ReportItem[]; draft: SetupDraft } | null>(null);

	// ---- run --------------------------------------------------------------------------------
	run = $state.raw<RandomState | null>(null);
	live = $state.raw<RandomLive | null>(null);
	log = $state.raw<LogEntry[]>([]);

	// ---- feedback ---------------------------------------------------------------------------
	busy = $state<string | null>(null);
	refusal = $state.raw<Refusal | null>(null);
	note = $state.raw<Note | null>(null);
	/** The service does not offer these methods, or the first read failed. */
	syncError = $state<string | null>(null);

	private tableSeq = 0;
	private setupSeq = 0;
	private tableTimer: ReturnType<typeof setTimeout> | null = null;
	private setupTimer: ReturnType<typeof setTimeout> | null = null;
	private logNext = 0;

	constructor(control: ServiceClient, options: AcquireOptions = {}) {
		this.control = control;
		this.name = options.name ?? clientName();
		this.sessionStore = options.session === undefined ? storageOf('sessionStorage') : options.session;
		this.localStore = options.local === undefined ? storageOf('localStorage') : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 250;
		this.lease = read(this.sessionStore, LEASE_KEY);
		this.setup = loadSetup(this.localStore);

		control.on(T.session, (d) => this.onSession(d as unknown as SessionChanged));
		control.on(T.station, (d) => this.onStations(d as unknown as StationList));
		control.on(T.channels, (d) => this.onTable(d as unknown as ChannelTable));
		control.on(T.state, (d) => this.onState(d as unknown as RandomState));
		control.on(T.live, (d) => this.onLive(d as unknown as RandomLive));
		control.on(T.log, (d) => this.onLog((d as unknown as { entries: LogEntry[] }).entries));
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
	readonly setupKey = $derived(JSON.stringify(this.config));
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

	/** The derived numbers for the draft, from the service; the applied ones while it checks. */
	get derived(): Derived | null {
		return this.setupCheck?.derived ?? (this.setupStatus === 'applied' ? (this.applied?.derived ?? null) : null);
	}

	get setupStatus(): SetupStatus {
		const run = this.run;
		if (!run || run.state === 'unconfigured') return 'none';
		if (this.applied && this.applied.runId === run.run_id) return this.applied.key === this.setupKey ? 'applied' : 'changed';
		return 'foreign';
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

	/** The live frame for the present run, or null. */
	get frame(): RandomLive | null {
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
			const [session, stations, table, run, log] = await Promise.all([
				c.request<SessionState>('session.state'),
				c.request<StationList>('station.list'),
				c.request<ChannelTable>('channels.get'),
				c.request<RandomState>('random.state'),
				c.request<RandomLogResult>('random.log', { since: 0 })
			]);
			this.syncError = null;
			this.setSession(session);
			this.onStations(stations, true);
			this.adoptTable(table);
			this.onState(run);
			this.log = log.entries.slice(-LOG_LIMIT);
			this.logNext = log.next;
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		if (this.lease && this.role !== 'controller') await this.resume();
		this.scheduleTableCheck(0);
		this.scheduleSetupCheck(0);
	}

	// =========================================================================================
	// Session

	private setSession(s: SessionState) {
		this.session = s;
		this.orphanDeadline = s.orphan_s === null ? null : Date.now() + s.orphan_s * 1000;
	}

	private onSession(d: SessionChanged) {
		if (this.session) this.session = { ...this.session, controller: d.controller, orphan_s: d.orphan_s };
		this.orphanDeadline = d.orphan_s === null ? null : Date.now() + d.orphan_s * 1000;
		// The event says who holds the seat, not whether it is this screen: ask.
		void this.control
			.request<SessionState>('session.state')
			.then((s) => {
				const was = this.role;
				this.setSession(s);
				if (was === 'controller' && s.role !== 'controller') {
					this.setLease(null);
					this.note = { tone: 'warn', text: 'This screen no longer holds control.' };
				}
			})
			.catch(() => {});
	}

	private setLease(lease: string | null) {
		this.lease = lease;
		write(this.sessionStore, LEASE_KEY, lease);
	}

	async claim(): Promise<boolean> {
		const res = await this.act<SessionClaimResult>('Take Control', 'session.claim', { name: this.name, ...(this.lease ? { lease: this.lease } : {}) });
		if (!res) return false;
		this.setLease(res.lease);
		this.session = { ...(this.session ?? { controller: null, orphan_s: null, heartbeat_s: res.heartbeat_s, timeout_s: res.timeout_s }), role: 'controller' };
		this.note = res.resumed ? { tone: 'pass', text: 'Control resumed.' } : null;
		await this.refreshSession();
		return true;
	}

	/** After a drop: present the lease. The seat comes back if it was kept for this screen. */
	private async resume(): Promise<void> {
		try {
			const res = await this.control.request<SessionClaimResult>('session.claim', { name: this.name, lease: this.lease });
			this.setLease(res.lease);
			this.note = res.resumed
				? { tone: 'pass', text: 'The connection dropped and came back. This screen still holds control.' }
				: { tone: 'warn', text: 'The connection dropped for longer than the service keeps a seat. This screen has taken control again.' };
		} catch (err) {
			this.setLease(null);
			const r = toRefusal('Resume Control', err);
			this.note = { tone: 'warn', text: `The connection dropped and control was not resumed. ${r.message}` };
		}
		await this.refreshSession();
	}

	async release(): Promise<void> {
		const res = await this.act<SessionReleaseResult>('Release Control', 'session.release');
		if (!res) return;
		this.setLease(null);
		this.note = null;
		await this.refreshSession();
	}

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
		try {
			const res = await this.control.request<RandomDryRunResult>('random.configure', { config: this.config, dry_run: true });
			if (seq === this.setupSeq) this.setupCheck = res;
			return res;
		} catch {
			return null;
		} finally {
			if (seq === this.setupSeq) this.checking = false;
		}
	}

	resetSetup() {
		this.setup = defaultSetup();
	}

	/** Put the setup back to what this screen last applied. */
	revertSetup() {
		if (this.applied) this.setup = structuredClone(this.applied.draft);
	}

	/** Send the setup (`random.configure`). It replaces what the service held. */
	async apply(): Promise<boolean> {
		const config = this.config;
		const key = this.setupKey;
		const draft = $state.snapshot(this.setup) as SetupDraft;
		const res = await this.act<RandomConfigureResult>('Apply Setup', 'random.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				this.setupCheck = { errors: (r.detail.errors as ReportItem[]) ?? [], warnings: (r.detail.warnings as ReportItem[]) ?? [], derived: null };
			}
			if (r?.reason === 'table_invalid' && r.detail) {
				this.tableCheck = { errors: (r.detail.errors as ReportItem[]) ?? [], warnings: (r.detail.warnings as ReportItem[]) ?? [] };
			}
			return false;
		}
		this.applied = { key, runId: res.state.run_id, derived: res.derived, warnings: res.warnings, draft };
		this.onState(res.state);
		return true;
	}

	// =========================================================================================
	// Run

	private onState(s: RandomState) {
		const prev = this.run;
		this.run = s;
		// A new run starts with an empty display; a finished run keeps its last frame.
		if (prev && prev.run_id !== s.run_id) this.live = null;
	}

	private onLive(l: RandomLive) {
		if (this.run && l.run_id !== this.run.run_id) return;
		this.live = l;
	}

	private onLog(entries: LogEntry[]) {
		if (!entries.length) return;
		// The log starts again at every configure and reset: its sequence goes back to 0.
		const restart = entries[0].seq < this.logNext;
		const kept = restart ? [] : this.log;
		this.log = [...kept, ...entries].slice(-LOG_LIMIT);
		this.logNext = entries[entries.length - 1].seq + 1;
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
		// The new run's state event can arrive before this answer: compare with the run before.
		const before = this.run?.run_id ?? null;
		const res = await this.act<RandomState>('New Run', 'random.reset');
		if (!res) return false;
		if (this.applied && this.applied.runId === before) this.applied = { ...this.applied, runId: res.run_id };
		this.onState(res);
		return true;
	}

	async clear(): Promise<boolean> {
		const res = await this.act<RandomState>('Clear', 'random.clear');
		if (!res) return false;
		this.applied = null;
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
