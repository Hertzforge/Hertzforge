/**
 * Modal Test's working state, against the control service's `modal_test.*` methods
 * (schema/PROTOCOL.md, "Modal Test").
 *
 * The station, the channel table and the controller seat are the service's and shared by every
 * test type: they stay in the Acquire store, and this one reads them from there. The DOFs are the
 * table's own node, direction and analysis role columns, edited through the table's methods and
 * sent with it. What is the Modal Test's own is here: the setup draft (kept across reloads), the
 * dry run of the draft with the service's check, the test's state with its counts and hit tiles,
 * its live frame, its log, and the verbs and keys. The control service judges every hit, keeps
 * every count and holds every protection; this store sends the operator's choices and shows what
 * comes back.
 */

import { untrack } from 'svelte';
import { regresses } from '../runOrder';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelRow, RunRecordRef } from '$lib/protocol/types';
import {
	isModalRun,
	MODAL_TEST_TOPICS,
	type ModalCallResult,
	type ModalHitKind,
	type ModalTestCheck,
	type ModalTestConfig,
	type ModalTestConfigResult,
	type ModalTestConfigureResult,
	type ModalMode,
	type ModalTestDryRunResult,
	type ModalTestInvalidDetail,
	type ModalTestLive,
	type ModalTestLogEntry,
	type ModalTestLogEvent,
	type ModalTestLogResult,
	type ModalTestRun,
	type ModalTestState
} from '$lib/protocol/modal_test';
import type { ActionId, ActionState, RunHead, SetupStatus } from '../actions';
import { canonical } from '../mimo/mimoSetup';
import { errorCount, type FieldErrors } from '../setup';
import type { AcquireState } from '../store.svelte';
import type { RowEdit } from '../table';
import { toRefusal, type Refusal } from '../words';
import { modalKeyAction, type KeyLike, type ModalKeyAction } from './modalTestKeys';
import { defaultModalSetup, fromModalConfig, setModalMode, itemsByKey, localErrors, mergeModalSetup, MODAL_SECTIONS, modalRows, modalSectionOf, toModalConfig, type ModalDraft, type ModalSection } from './modalTestSetup';
import { modalBarActions, modalIsLive, modalMeasureVerbs, modalPrimary, type ModalVerb, type ModalVerbContext } from './modalTestVerbs';
import { chipState, MODAL_CHIP_WORDS, MODAL_OUTCOME_WORDS, MODAL_PHASE_WORDS } from './modalTestWords';

const SETUP_KEY = 'hertzforge.acquire.modal_test.setup.v1';
/** The setup this screen applied, by run: `{runId, key}`. */
const HELD_KEY = 'hertzforge.acquire.modal_test.held.v1';
const LOG_LIMIT = 500;

function storageOf(name: 'localStorage'): Storage | null {
	try {
		return typeof window === 'undefined' ? null : window[name];
	} catch {
		return null;
	}
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

const nanOut = (_k: string, v: unknown) => (typeof v === 'number' && Number.isNaN(v) ? '__NaN__' : v);
const nanIn = (_k: string, v: unknown) => (v === '__NaN__' ? NaN : v);

export function saveModalSetup(d: ModalDraft): string {
	return JSON.stringify(d, nanOut);
}

export function loadModalSetup(raw: string | null): ModalDraft {
	if (!raw) return defaultModalSetup();
	try {
		return mergeModalSetup(JSON.parse(raw, nanIn) as Partial<ModalDraft>);
	} catch {
		return defaultModalSetup();
	}
}

export interface ModalOptions {
	local?: Storage | null;
	/** Delay before a dry run after an edit, ms. */
	checkDelayMs?: number;
	/** A display window observes: it keeps no draft and checks nothing. */
	drafts?: boolean;
}

/** The Measure verbs' method names on the service. */
const VERB_METHOD: Record<Exclude<ModalVerb, 'end'>, string> = {
	arm: 'modal_test.arm',
	disarm: 'modal_test.disarm',
	accept: 'modal_test.accept',
	reject: 'modal_test.reject',
	undo: 'modal_test.undo',
	clear: 'modal_test.clear_point',
	next: 'modal_test.next',
	previous: 'modal_test.previous',
	target: 'modal_test.target_from_hit',
	drive_start: 'modal_test.drive_start',
	drive_stop: 'modal_test.drive_stop'
};

const VERB_WORDS: Record<ModalVerb, string> = {
	arm: 'Arm',
	disarm: 'Disarm',
	accept: 'Accept',
	reject: 'Reject',
	undo: 'Undo',
	clear: 'Clear Point',
	next: 'Next Point',
	previous: 'Previous Point',
	target: 'Use This Hit as the Target',
	drive_start: 'Start Driving',
	drive_stop: 'Stop Driving',
	end: 'End Test'
};

/** A key as the person pressed it, in words: Space, Left arrow, R. */
function keyName(k: string): string {
	if (k === ' ' || k === 'Spacebar') return 'Space';
	if (k === 'ArrowLeft') return 'Left arrow';
	if (k === 'ArrowRight') return 'Right arrow';
	return k.length === 1 ? k.toUpperCase() : k;
}

export class ModalTestStore {
	readonly control: ServiceClient;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;

	// ---- the operator's draft --------------------------------------------------------------
	setup = $state<ModalDraft>(defaultModalSetup());
	section = $state<string>('station');
	step = $state<'setup' | 'run'>('setup');
	/** Which response and reference the FRF plots show: a view choice, not part of the setup. */
	shown = $state({ response: 0, reference: 0 });
	/** Said when a mode change cleared the roving plan, until the next mode change or a draft adopted. */
	rovingNote = $state<string | null>(null);

	// ---- from the service ------------------------------------------------------------------
	status = $state.raw<ModalTestState | null>(null);
	live = $state.raw<ModalTestLive | null>(null);
	served = $state.raw<ModalTestConfigResult | null>(null);
	check = $state.raw<ModalTestDryRunResult | null>(null);
	checkedKey = $state('');
	/** The table revision the check was made against. */
	checkedBase = $state(-1);
	checking = $state(false);
	log = $state.raw<ModalTestLogEntry[]>([]);
	held = $state.raw<{ runId: string | null; key: string } | null>(null);

	// ---- feedback --------------------------------------------------------------------------
	busy = $state<string | null>(null);
	/** The last Measure verb the service refused, in its words, beside the hit. */
	verbRefusal = $state.raw<Refusal | null>(null);
	#refusal = $state.raw<Refusal | null>(null);
	syncError = $state<string | null>(null);

	private checkSeq = 0;
	/** State events seen: a reply's state is older than any event that came after the call went out. */
	private events = 0;
	private logRun: string | null = null;
	private checkTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(
		readonly acq: AcquireState,
		options: ModalOptions = {}
	) {
		this.control = acq.control;
		this.localStore = options.local === undefined ? storageOf('localStorage') : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 300;
		this.drafts = options.drafts ?? true;
		this.setup = loadModalSetup(read(this.localStore, SETUP_KEY));
		const held = read(this.localStore, HELD_KEY);
		if (held) {
			try {
				this.held = JSON.parse(held);
			} catch {
				this.held = null;
			}
		}
		this.control.on(MODAL_TEST_TOPICS.state, (d) => {
			this.events++;
			this.onStatus(d as unknown as ModalTestState);
		});
		this.control.on(MODAL_TEST_TOPICS.live, (d) => this.onFrame(d as unknown as ModalTestLive));
		this.control.on(MODAL_TEST_TOPICS.log, (d) => {
			const e = d as unknown as ModalTestLogEvent;
			const id = this.now?.run_id;
			if (id && e.run_id !== id) return;
			const have = new Set(this.log.map((x) => x.seq));
			this.log = [...this.log, ...e.entries.filter((x) => !have.has(x.seq))].slice(-LOG_LIMIT);
		});
	}

	// =========================================================================================
	// Derived state

	get connected(): boolean {
		return this.acq.connected;
	}

	get isController(): boolean {
		return this.acq.isController;
	}

	get rows(): ChannelRow[] {
		return this.acq.rows;
	}

	/** The test the service holds, or null when it holds none. */
	get now(): ModalTestRun | null {
		return isModalRun(this.status) ? this.status : null;
	}

	get isLive(): boolean {
		return modalIsLive(this.now);
	}

	get locked(): boolean {
		return this.isLive || this.acq.isLive;
	}

	/** The rows a test records: impact mode opens the measured rows only; shaker mode every enabled input and the drives. */
	get recordChannels(): number {
		const r = modalRows(this.rows);
		if (this.setup.mode === 'impact') return r.references.length + r.responses.length;
		return r.references.length + r.responses.length + r.monitors.length + r.unassigned.length + this.setup.shaker.drives.length;
	}

	get recordRateHz(): number {
		return this.checkOf?.fs_hz ?? this.setup.fs_hz;
	}

	/** The station is the simulator: the simulated hits are offered. */
	get onSimulator(): boolean {
		const n = this.now;
		if (n) return n.station_class === 'sim';
		return this.acq.source?.cls === 'sim';
	}

	readonly wire = $derived.by((): ModalTestConfig => toModalConfig(this.setup));
	readonly key = $derived(canonical(this.wire));
	readonly local = $derived.by((): FieldErrors => localErrors(this.setup));

	/**
	 * What a dry run is checked against: the draft and the table revision the service holds. The
	 * service checks the DOFs in its own table, so a check made before the table changed is not shown.
	 */
	get checkKey(): string {
		return `${this.key}#${this.acq.base}`;
	}

	/** The dry run for the draft and the applied table as they stand, or null (also while the table has edits not applied). */
	get current(): ModalTestDryRunResult | null {
		return this.check && !this.acq.tableDirty && this.checkedKey === this.key && this.checkedBase === this.acq.base ? this.check : null;
	}

	/** The service's check: the draft's dry run, else the test's own once applied. */
	get checkOf(): ModalTestCheck | null {
		return this.current?.check ?? (this.setupStatus === 'applied' || this.isLive ? (this.served?.check ?? null) : null);
	}

	/** The check the test runs with: the one the service holds for it. */
	get runCheck(): ModalTestCheck | null {
		return this.served?.check ?? null;
	}

	get fieldErrors(): FieldErrors {
		const fromCheck = this.current ? itemsByKey(this.current.errors) : {};
		const out: FieldErrors = {};
		for (const map of [this.local, fromCheck]) for (const [k, v] of Object.entries(map)) for (const m of v) if (!(out[k] ??= []).includes(m)) out[k].push(m);
		return out;
	}

	get fieldWarnings(): FieldErrors {
		return this.current ? itemsByKey(this.current.warnings) : {};
	}

	get counts(): Record<string, { errors: number; warnings: number }> {
		const out = Object.fromEntries(MODAL_SECTIONS.map((s) => [s.id, { errors: 0, warnings: 0 }])) as Record<ModalSection, { errors: number; warnings: number }>;
		for (const [k, v] of Object.entries(this.fieldErrors)) out[modalSectionOf(k)].errors += v.length;
		for (const [k, v] of Object.entries(this.fieldWarnings)) out[modalSectionOf(k)].warnings += v.length;
		out.station.errors += this.acq.tableErrorCount;
		return out;
	}

	get setupErrorCount(): number {
		return Object.values(this.counts).reduce((n, c) => n + c.errors, 0);
	}

	get setupStatus(): SetupStatus {
		const s = this.status;
		if (!s || s.state === 'none') return 'none';
		const held = this.held;
		if (!held || held.runId !== s.run_id) return 'unread';
		return held.key === this.key ? 'applied' : 'changed';
	}

	get pending(): string | null {
		if (this.acq.stationDirty) return 'Use the chosen station, or put the choice back, first.';
		return null;
	}

	private get verbContext(): ModalVerbContext {
		const n = this.now;
		return {
			connected: this.connected,
			controller: this.isController,
			run: n,
			setup: this.setupStatus,
			setupErrors: this.setupErrorCount,
			busy: this.busy,
			pending: this.pending
		};
	}

	get actions(): Record<ActionId, ActionState> {
		return modalBarActions(this.verbContext);
	}

	get primary(): ActionId | null {
		return modalPrimary(this.verbContext);
	}

	/** The Measure screen's verbs and why each waits. */
	get verbs(): Record<ModalVerb, ActionState> {
		return modalMeasureVerbs(this.verbContext);
	}

	// ---- the run as the shared run bar and notices read it ---------------------------------

	get record(): RunRecordRef | null {
		const r = this.now?.record;
		return r ? { folder: r.folder, streams: r.streams ? 'on' : 'off', problem: r.problem } : null;
	}

	get run(): RunHead | null {
		const s = this.now;
		if (!s) return this.status ? { run_id: null, state: 'unconfigured', phase: 'idle', title: null, mode: null, levels_db: null, level_db: null, at_level_s: null, remaining_at_level_s: null, end: null, record: null } : null;
		return {
			run_id: s.run_id,
			state: chipState(s.state) as RunHead['state'],
			phase: s.state,
			title: s.title,
			mode: null,
			levels_db: null,
			level_db: null,
			at_level_s: null,
			remaining_at_level_s: null,
			end: s.end ? ({ outcome: s.end.outcome, message: s.end.message, at_s: s.end.at_s, level_db: null, trip: null } as unknown as RunHead['end']) : null,
			record: this.record
		};
	}

	/** No level in dB: the bar shows no level readout. */
	readonly frame = null;
	readonly levelHidden = true;

	readonly outcomeWords: Record<string, string> = MODAL_OUTCOME_WORDS;
	readonly stateWords: Record<string, string> = MODAL_CHIP_WORDS;
	readonly phaseWords: Record<string, string> = MODAL_PHASE_WORDS;

	get elapsed(): number | null {
		return null;
	}

	get source(): { source: 'SIM' | 'HARDWARE'; cls: 'sim' | 'ni_simulated' | 'ni_real' } | null {
		const s = this.now;
		if (s) return { source: s.source, cls: s.station_class };
		return this.acq.source;
	}

	get session() {
		return this.acq.session;
	}

	get orphanDeadline() {
		return this.acq.orphanDeadline;
	}

	get refusal(): Refusal | null {
		return this.#refusal ?? this.acq.refusal;
	}

	set refusal(r: Refusal | null) {
		this.#refusal = r;
	}

	get note() {
		return this.acq.note;
	}

	claim() {
		return this.acq.claim();
	}

	release() {
		return this.acq.release();
	}

	dismissRefusal() {
		this.#refusal = null;
		this.acq.dismissRefusal();
	}

	dismissNote() {
		this.acq.dismissNote();
	}

	// =========================================================================================
	// Wiring

	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				const text = saveModalSetup($state.snapshot(this.setup) as ModalDraft);
				untrack(() => write(this.localStore, SETUP_KEY, text));
			});
			$effect(() => {
				const key = this.checkKey;
				untrack(() => {
					if (this.drafts && key) this.scheduleCheck();
				});
			});
		});
	}

	async sync(): Promise<void> {
		const c = this.control;
		try {
			const [status, served] = await Promise.all([c.request<ModalTestState>('modal_test.state'), c.request<ModalTestConfigResult>('modal_test.config')]);
			this.syncError = null;
			this.served = served;
			this.onStatus(status);
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		if (this.drafts) this.scheduleCheck(0);
	}

	private onStatus(s: ModalTestState) {
		const was = this.status;
		if (regresses(was as never, s as never)) return;
		this.status = s;
		if (this.live && (!isModalRun(s) || this.live.run_id !== s.run_id || s.state === 'configured')) this.live = null;
		const id = isModalRun(s) ? s.run_id : null;
		if (id !== this.logRun) void this.loadLog(id);
		if ((isModalRun(was) ? was.run_id : null) !== id && id) void this.readServed();
		if (modalIsLive(isModalRun(s) ? s : null) && !modalIsLive(isModalRun(was) ? was : null) && this.step === 'setup') this.step = 'run';
	}

	private onFrame(l: ModalTestLive) {
		const s = this.now;
		if (s?.run_id && l.run_id !== s.run_id) return;
		this.live = l;
	}

	async loadLog(runId: string | null): Promise<void> {
		if (runId !== this.logRun) this.log = [];
		this.logRun = runId;
		if (!runId) return;
		try {
			const res = await this.control.request<ModalTestLogResult>('modal_test.log', { since: 0 });
			if (this.logRun !== runId) return;
			const have = new Map(this.log.map((e) => [e.seq, e]));
			for (const e of res.entries) have.set(e.seq, e);
			this.log = [...have.values()].sort((a, b) => a.seq - b.seq).slice(-LOG_LIMIT);
		} catch {
			/* the entries come with each log event */
		}
	}

	private async readServed(): Promise<void> {
		try {
			this.served = await this.control.request<ModalTestConfigResult>('modal_test.config');
		} catch {
			/* kept as it was */
		}
	}

	/** Start the draft from the service's configuration (the one the test holds). */
	adoptServed(): boolean {
		const c = this.served?.config;
		if (!c || this.locked) return false;
		this.setup = fromModalConfig(c);
		this.rovingNote = null;
		return true;
	}

	/** Change the mode; a roving plan the new mode cannot take is cleared, with a note saying so. */
	chooseMode(mode: ModalMode): void {
		if (this.locked || this.setup.mode === mode) return;
		this.rovingNote = setModalMode(this.setup, mode, this.rows);
	}

	// =========================================================================================
	// The DOFs: the channel table's own columns, through the table's edit

	/** Set a row's node, direction or analysis role, at its position in the table shown. */
	editDof(position: number, edit: Extract<RowEdit, { field: 'node' | 'direction' | 'analysis_role' }>): boolean {
		if (this.locked) return false;
		return this.acq.editRows([position], edit).changed > 0;
	}

	// =========================================================================================
	// The dry run

	scheduleCheck(delay = this.checkDelayMs) {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	/** The service's verdict on the draft, with its check (`dry_run`: needs no control). */
	async checkSetup(): Promise<ModalTestDryRunResult | null> {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = null;
		if (!this.connected || errorCount(this.local) > 0) return null;
		const config = this.wire;
		const key = this.key;
		const base = this.acq.base;
		const seq = ++this.checkSeq;
		this.checking = true;
		try {
			const res = await this.control.request<ModalTestDryRunResult>('modal_test.configure', { config, dry_run: true });
			if (seq === this.checkSeq) {
				this.check = res;
				this.checkedKey = key;
				this.checkedBase = base;
			}
			return res;
		} catch (err) {
			const r = toRefusal('Check the setup', err);
			if (seq === this.checkSeq && r.reason === 'bad_params') {
				const field = typeof r.detail?.param === 'string' ? r.detail.param : 'setup';
				this.check = { errors: [{ reason: 'out_of_range', field, message: r.message } as never], warnings: [], check: null };
				this.checkedKey = key;
				this.checkedBase = base;
			}
			return null;
		} finally {
			if (seq === this.checkSeq) this.checking = false;
		}
	}

	// =========================================================================================
	// The run

	private setHeld(h: { runId: string | null; key: string } | null) {
		this.held = h;
		write(this.localStore, HELD_KEY, h ? JSON.stringify(h) : null);
	}

	/** Send the setup. A table with edits (the DOFs among them) is applied first: the plan is made from it. */
	async apply(): Promise<boolean> {
		if (!this.actions.apply.enabled) return false;
		if (this.acq.tableDirty) {
			const ok = await this.acq.applyTable();
			if (!ok) return false;
		}
		const config = this.wire;
		const key = this.key;
		const res = await this.act<ModalTestConfigureResult>('Apply Setup', 'modal_test.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				const d = r.detail as unknown as ModalTestInvalidDetail;
				this.check = { errors: d.errors ?? [], warnings: d.warnings ?? [], check: d.check ?? null };
				this.checkedKey = key;
				this.checkedBase = this.acq.base;
			}
			return false;
		}
		this.status = res.state;
		this.live = null;
		this.setHeld({ runId: res.state.run_id, key });
		this.served = { run_id: res.state.run_id, config, check: res.check };
		void this.loadLog(res.state.run_id);
		void this.readServed();
		this.acq.note = { tone: 'pass', text: 'The service holds this setup.' };
		return true;
	}

	/** The station opens and the test is ready at its first point. */
	async start(): Promise<boolean> {
		const seen = this.events;
		const res = await this.act<ModalTestState>('Start', 'modal_test.start');
		if (res) {
			if (this.events === seen) this.onStatus(res);
			this.step = 'run';
		} else void this.sync();
		return res !== null;
	}

	arm(): Promise<boolean> {
		return Promise.resolve(false);
	}

	proceed(): Promise<boolean> {
		return Promise.resolve(false);
	}

	setMode(): Promise<boolean> {
		return Promise.resolve(false);
	}

	level(): Promise<null> {
		return Promise.resolve(null);
	}

	goTo(): Promise<null> {
		return Promise.resolve(null);
	}

	/** The controlled end (shaker mode: the shared stop ramp); the run bar's Stop and End Test. */
	async stop(): Promise<boolean> {
		const seen = this.events;
		const res = await this.act<ModalTestState>('End Test', 'modal_test.end', {}, false);
		if (res && this.events === seen) this.onStatus(res);
		return res !== null;
	}

	async abort(): Promise<boolean> {
		const seen = this.events;
		const res = await this.act<ModalTestState>('Abort', 'modal_test.abort', {}, false);
		if (res && this.events === seen) this.onStatus(res);
		return res !== null;
	}

	/** A new test from the same configuration. */
	async reset(): Promise<boolean> {
		const seen = this.events;
		const res = await this.act<ModalTestState>('New Run', 'modal_test.reset');
		if (!res) return false;
		if (this.events === seen) this.onStatus(res);
		this.live = null;
		if (this.held && this.setupStatus !== 'changed' && isModalRun(res)) this.setHeld({ runId: res.run_id, key: this.key });
		return true;
	}

	// =========================================================================================
	// The Measure screen's verbs and keys

	/** A Measure verb: the service answers with the counts and the state, which replace the ones shown. */
	async verb(v: ModalVerb, params: Record<string, unknown> = {}): Promise<boolean> {
		if (v === 'end') return this.stop();
		const gate = this.verbs[v];
		if (!gate.enabled) return false;
		this.verbRefusal = null;
		const seen = this.events;
		try {
			const res = await this.control.request<ModalCallResult>(VERB_METHOD[v], params);
			if (res?.state && this.events === seen) this.onStatus(res.state);
			return true;
		} catch (err) {
			this.verbRefusal = toRefusal(VERB_WORDS[v], err);
			return false;
		}
	}

	/** Move to a point of the plan, by its index. */
	async select(point: number): Promise<boolean> {
		if (!this.isLive || !this.isController) return false;
		this.verbRefusal = null;
		const seen = this.events;
		try {
			const res = await this.control.request<ModalCallResult>('modal_test.select', { point });
			if (res?.state && this.events === seen) this.onStatus(res.state);
			return true;
		} catch (err) {
			this.verbRefusal = toRefusal('Select the Point', err);
			return false;
		}
	}

	/** On the simulator: the kind of the next simulated hit. */
	async simHit(kind: ModalHitKind): Promise<boolean> {
		if (!this.onSimulator || !this.isLive) return false;
		this.verbRefusal = null;
		try {
			await this.control.request('modal_test.sim_hit', { kind });
			return true;
		} catch (err) {
			this.verbRefusal = toRefusal('Simulated Hit', err);
			return false;
		}
	}

	/** The verb a key asks for, as the Measure screen runs it: Space arms or disarms. */
	keyVerb(a: ModalKeyAction): ModalVerb | null {
		if (a === 'arm') {
			const s = this.now?.state;
			if (this.now?.mode !== 'impact') return null;
			return s === 'armed' || s === 'arming' ? 'disarm' : 'arm';
		}
		return a;
	}

	/**
	 * A key press on the Measure screen: its verb runs when it is one of the screen's keys and the
	 * verb applies now; when it does not (a hit waits, or the service has not yet answered the key
	 * before), the key is refused in words where the screen shows refusals, never dropped unseen.
	 * Returns the verb asked for (so the caller can stop the browser's own use of the key), or null
	 * when the key was not the screen's.
	 */
	onKey(e: KeyLike): ModalVerb | null {
		if (this.step !== 'run') return null;
		const a = modalKeyAction(e);
		if (!a) return null;
		const v = this.keyVerb(a);
		if (!v) return null;
		const gate = this.verbs[v];
		if (!gate.enabled) {
			const message = `${keyName(e.key)} was not taken: ${gate.reason ?? 'not now.'}`;
			this.verbRefusal = { verb: VERB_WORDS[v], reason: 'key_not_taken', code: 'refused', title: 'Key Not Taken', message, facts: [], detail: undefined };
			return v;
		}
		void this.verb(v);
		return v;
	}

	private async act<R>(verb: string, method: string, params: Record<string, unknown> = {}, exclusive = true): Promise<R | null> {
		if (exclusive) this.busy = verb;
		this.#refusal = null;
		try {
			return await this.control.request<R>(method, params);
		} catch (err) {
			this.#refusal = toRefusal(verb, err);
			return null;
		} finally {
			if (exclusive && this.busy === verb) this.busy = null;
		}
	}
}
