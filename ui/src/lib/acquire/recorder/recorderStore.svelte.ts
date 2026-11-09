/**
 * The Recorder's working state, against the control service's `recorder.*` methods
 * (schema/PROTOCOL.md, "Recorder").
 *
 * The station, the channel table and the controller seat are the service's and shared by every
 * test type: they stay in the Acquire store, and this one reads them from there. What is the
 * Recorder's own is here: the setup draft (kept across reloads), the dry run of the draft, the
 * run's state and its live numbers, the run's recordings and marks, the service's answer to the
 * last verb, and the verbs. As everywhere in Acquire, the service holds the truth and computes
 * every number; this store keeps the operator's edits beside it and sends them when the operator
 * says so. A verb's answer is shown as the service gave it (the station frame a recording began
 * at, the pre-window it holds), never what was asked.
 */

import { regresses } from '../runOrder';
import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelRow } from '$lib/protocol/types';
import {
	RECORDER_TOPICS,
	type RecorderArmResult,
	type RecorderConfig,
	type RecorderConfigResult,
	type RecorderConfigureResult,
	type RecorderCutResult,
	type RecorderDerived,
	type RecorderDryRunResult,
	type RecorderLiveHead,
	type RecorderLogEntry,
	type RecorderLogEvent,
	type RecorderLogResult,
	type RecorderMark,
	type RecorderMarksResult,
	type RecorderRecordResult,
	type RecorderRecording,
	type RecorderRecordingsResult,
	type RecorderState
} from '$lib/protocol/recorder';
import type { ActionId, ActionState, RunHead, SetupStatus } from '../actions';
import { canonical } from '../mimo/mimoSetup';
import { errorCount, type FieldErrors } from '../setup';
import type { AcquireState } from '../store.svelte';
import { toRefusal, type Refusal } from '../words';
import {
	defaultRecorderSetup,
	fromRecorderConfig,
	localErrors,
	mergeRecorderSetup,
	RECORDER_SECTIONS,
	recorderByKey,
	recorderSectionOf,
	toRecorderConfig,
	type RecorderDraft,
	type RecorderSection
} from './recorderSetup';
import { recorderBarActions, recorderIsLive, recorderPrimary, recorderVerbs, type RecorderVerbContext, type RecorderVerbId } from './recorderVerbs';
import {
	RECORDER_OUTCOME_WORDS,
	RECORDER_PHASE_WORDS,
	RECORDER_STATE_WORDS,
	recorderModeFact,
	recorderSpaceFacts,
	recorderStateFact,
	secs,
	type RecorderCardSetup
} from './recorderWords';

const SETUP_KEY = 'hertzforge.acquire.recorder.setup.v1';
/** The setup this screen applied, by run: `{runId, key}`. */
const HELD_KEY = 'hertzforge.acquire.recorder.held.v1';
/** Most log entries kept on screen; the service keeps the full log with the run. */
const LOG_LIMIT = 500;
/** The log's words for the run folder at start. */
const FOLDER_LINE = /^records in (.+)$/;

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

/** JSON that keeps NaN (a field the operator has emptied) through a reload. */
const nanOut = (_k: string, v: unknown) => (typeof v === 'number' && Number.isNaN(v) ? '__NaN__' : v);
const nanIn = (_k: string, v: unknown) => (v === '__NaN__' ? NaN : v);

export function saveRecorderSetup(d: RecorderDraft): string {
	return JSON.stringify(d, nanOut);
}

export function loadRecorderSetup(raw: string | null): RecorderDraft {
	if (!raw) return defaultRecorderSetup();
	try {
		return mergeRecorderSetup(JSON.parse(raw, nanIn) as Partial<RecorderDraft>);
	} catch {
		return defaultRecorderSetup();
	}
}

export interface RecorderOptions {
	local?: Storage | null;
	/** Delay before a dry run after an edit, ms. */
	checkDelayMs?: number;
	/** A display window observes: it keeps no draft and checks nothing. */
	drafts?: boolean;
}

/** The service's answer to the last Recorder verb this screen sent, as the run view shows it. */
export interface RecorderAnswer {
	verb: RecorderVerbId;
	result: RecorderArmResult | RecorderRecordResult | RecorderCutResult | RecorderState;
	words: string;
}

export class RecorderStore {
	readonly control: ServiceClient;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;

	// ---- the operator's draft --------------------------------------------------------------
	setup = $state<RecorderDraft>(defaultRecorderSetup());
	section = $state<RecorderSection>('trigger');
	/** Where the operator is: the setup, or the run. */
	step = $state<'setup' | 'run'>('setup');

	// ---- from the service ------------------------------------------------------------------
	status = $state.raw<RecorderState | null>(null);
	/**
	 * The run's numbers from the last frame: `recorder.live` on the default subscription, or this
	 * screen's own `recorder.data` once it has subscribed to plots (the service then sends no
	 * `recorder.live`).
	 */
	live = $state.raw<RecorderLiveHead | null>(null);
	/** What the service holds: the configuration of the present run. */
	served = $state.raw<RecorderConfigResult | null>(null);
	/** The dry run of the draft's setup, and the key of the setup it checked. */
	check = $state.raw<RecorderDryRunResult | null>(null);
	checkedKey = $state('');
	checking = $state(false);
	log = $state.raw<RecorderLogEntry[]>([]);
	/** The run's recordings as the service lists them: the saved ones, then the one in progress. */
	recordings = $state.raw<RecorderRecording[]>([]);
	/** The run's marks, in the order they were made. */
	marks = $state.raw<RecorderMark[]>([]);
	/** The setup this screen applied: the run it made and the setup's key. */
	held = $state.raw<{ runId: string | null; key: string } | null>(null);
	/** The service's answer to the last Recorder verb this screen sent, for this run. */
	answer = $state.raw<RecorderAnswer | null>(null);

	// ---- feedback --------------------------------------------------------------------------
	busy = $state<string | null>(null);
	#refusal = $state.raw<Refusal | null>(null);
	syncError = $state<string | null>(null);

	/** Which came last for this run: the live frame (true) or the state. */
	private liveNewer = $state(false);
	private checkSeq = 0;
	private logRun: string | null = null;
	private checkTimer: ReturnType<typeof setTimeout> | null = null;
	/** The run whose lists are held, and whether a read of them is on its way or wanted again. */
	private listRun: string | null = null;
	private listing = false;
	private listAgain = false;

	constructor(
		readonly acq: AcquireState,
		options: RecorderOptions = {}
	) {
		this.control = acq.control;
		this.localStore = options.local === undefined ? storageOf('localStorage') : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 300;
		this.drafts = options.drafts ?? true;
		this.setup = loadRecorderSetup(read(this.localStore, SETUP_KEY));
		const held = read(this.localStore, HELD_KEY);
		if (held) {
			try {
				this.held = JSON.parse(held);
			} catch {
				this.held = null;
			}
		}
		this.control.on(RECORDER_TOPICS.state, (d) => this.onStatus(d as unknown as RecorderState));
		const onFrame = (d: Record<string, unknown>) => this.onFrame(d as unknown as RecorderLiveHead);
		this.control.on(RECORDER_TOPICS.live, onFrame);
		this.control.on(RECORDER_TOPICS.data, onFrame);
		this.control.on(RECORDER_TOPICS.log, (d) => {
			const e = d as unknown as RecorderLogEvent;
			if (this.status?.run_id && e.run_id !== this.status.run_id) return;
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

	get isLive(): boolean {
		return recorderIsLive(this.status);
	}

	/** Setup, station and table are locked while a run is live. */
	get locked(): boolean {
		return this.isLive || this.acq.isLive;
	}

	readonly wire = $derived.by((): RecorderConfig => toRecorderConfig(this.setup));
	readonly key = $derived(canonical(this.wire));
	readonly local = $derived.by((): FieldErrors => localErrors(this.setup));

	/** Findings by key: the editor's own and the dry run's for the draft as it stands. */
	get fieldErrors(): FieldErrors {
		const fromCheck = this.check && this.checkedKey === this.key ? recorderByKey(this.check.errors) : {};
		const out: FieldErrors = {};
		for (const map of [this.local, fromCheck]) for (const [k, v] of Object.entries(map)) for (const m of v) if (!(out[k] ??= []).includes(m)) out[k].push(m);
		return out;
	}

	get fieldWarnings(): FieldErrors {
		return this.check && this.checkedKey === this.key ? recorderByKey(this.check.warnings) : {};
	}

	/** Errors and warnings per section. */
	get counts(): Record<RecorderSection, { errors: number; warnings: number }> {
		const out = Object.fromEntries(RECORDER_SECTIONS.map((s) => [s.id, { errors: 0, warnings: 0 }])) as Record<RecorderSection, { errors: number; warnings: number }>;
		for (const [k, v] of Object.entries(this.fieldErrors)) out[recorderSectionOf(k)].errors += v.length;
		for (const [k, v] of Object.entries(this.fieldWarnings)) out[recorderSectionOf(k)].warnings += v.length;
		out.station.errors += this.acq.tableErrorCount;
		return out;
	}

	get setupErrorCount(): number {
		return Object.values(this.counts).reduce((n, c) => n + c.errors, 0);
	}

	get setupStatus(): SetupStatus {
		const s = this.status;
		if (!s || s.state === 'unconfigured') return 'none';
		const held = this.held;
		if (!held || held.runId !== s.run_id) return 'unread';
		return held.key === this.key ? 'applied' : 'changed';
	}

	/** Why Apply and Start wait, in words, or null. */
	get pending(): string | null {
		if (this.acq.stationDirty) return 'Use the chosen station, or put the choice back, first.';
		return null;
	}

	/** The configuration the run holds, as the service has it. */
	get runConfig(): RecorderConfig | null {
		return this.served?.config ?? null;
	}

	/** What the service derived for the draft as it stands (the dry run), else for the run it holds. */
	get derived(): RecorderDerived | null {
		if (this.check && this.checkedKey === this.key && this.check.derived) return this.check.derived;
		return this.served?.derived ?? null;
	}

	/**
	 * The latest numbers: the state, with this run's live frame laid over it when the frame came
	 * after it. A state that came after the frame (a change of phase, a stop) is the run's word
	 * until the next frame.
	 */
	get now(): RecorderState | null {
		const s = this.status;
		const l = this.live;
		if (!s || !l || l.run_id !== s.run_id || !this.liveNewer) return s;
		return {
			...s,
			state: l.state,
			phase: l.phase,
			t_s: l.t_s ?? s.t_s,
			station_frames: l.station_frames ?? s.station_frames,
			grid: s.grid ? { ...s.grid, requested_fs_hz: l.grid.requested_fs_hz, fs_hz: l.grid.fs_hz, group_delay_frames: l.grid.group_delay_frames } : s.grid,
			trigger: l.trigger,
			recording: l.recording,
			recordings: l.recordings,
			marks: l.marks,
			disk: l.disk,
			health: l.health
		};
	}

	/** The last frame's numbers, when they are this run's. */
	get frameOfRun(): RecorderLiveHead | null {
		const s = this.status;
		const l = this.live;
		return s && l && l.run_id === s.run_id ? l : null;
	}

	/** The run folder, as the run's log names it at start. */
	get folder(): string | null {
		for (const e of this.log) {
			const m = FOLDER_LINE.exec(e.text);
			if (m) return m[1].trim();
		}
		return null;
	}

	/** The last recording saved, or null. */
	get lastSaved(): RecorderRecording | null {
		for (let i = this.recordings.length - 1; i >= 0; i--) if (this.recordings[i].complete) return this.recordings[i];
		return null;
	}

	/** The trigger's row, level and slope, from the configuration the run holds, for the card's words. */
	get cardSetup(): RecorderCardSetup | null {
		const t = this.runConfig?.trigger;
		if (!t) return null;
		const index = t.row ?? null;
		const unit = index === null ? '' : (this.frameOfRun?.rows.find((r) => r.index === index)?.unit ?? this.rows.find((r) => r.index === index)?.units ?? '');
		return { row: index === null ? null : { index, unit }, level: t.level ?? null, slope: t.slope ?? 'rising' };
	}

	private get verbContext(): RecorderVerbContext {
		const n = this.now;
		return {
			connected: this.connected,
			controller: this.isController,
			run: n ? { state: n.state, phase: n.phase, mode: n.mode, left: n.trigger?.left ?? null } : null,
			setup: this.setupStatus,
			setupErrors: this.setupErrorCount,
			busy: this.busy,
			pending: this.pending
		};
	}

	/** The run bar's verbs, by the shared bar's names. */
	get actions(): Record<ActionId, ActionState> {
		return recorderBarActions(this.verbContext);
	}

	get primary(): ActionId | null {
		return recorderPrimary(this.verbContext);
	}

	/** The Recorder's own verbs, with why each waits. */
	get verbs(): Record<RecorderVerbId, ActionState> {
		return recorderVerbs(this.verbContext);
	}

	// ---- the run as the shared run bar and notices read it ---------------------------------

	get run(): RunHead | null {
		const s = this.now;
		if (!s) return null;
		return {
			run_id: s.run_id,
			// The bar reads Random's states; the Recorder's own (`starting`) comes with its words.
			state: s.state as RunHead['state'],
			phase: s.phase,
			title: s.title,
			mode: null,
			levels_db: null,
			level_db: null,
			at_level_s: null,
			remaining_at_level_s: null,
			end: s.end as unknown as RunHead['end']
		};
	}

	/** No level in dB: the bar's level readout is not shown, and the Recorder's numbers take its place. */
	readonly frame = null;
	readonly levelHidden = true;

	/** The run bar's words for the Recorder's outcomes, states and phases. */
	readonly outcomeWords: Record<string, string> = RECORDER_OUTCOME_WORDS;
	readonly stateWords: Record<string, string> = RECORDER_STATE_WORDS;
	readonly phaseWords: Record<string, string> = RECORDER_PHASE_WORDS;

	/** Station time since start. */
	get elapsed(): number | null {
		return this.frameOfRun?.t_s ?? this.status?.t_s ?? null;
	}

	get source(): { source: 'SIM' | 'HARDWARE'; cls: 'sim' | 'ni_simulated' | 'ni_real' } | null {
		const l = this.frameOfRun;
		if (l) return { source: l.source, cls: l.class };
		const s = this.status;
		if (s?.source && s.station) return { source: s.source, cls: s.station.class };
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

	/** Keep the draft across reloads, and re-check it as it changes. Returns the stopper. */
	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				const text = saveRecorderSetup($state.snapshot(this.setup) as RecorderDraft);
				untrack(() => write(this.localStore, SETUP_KEY, text));
			});
			$effect(() => {
				const key = this.key;
				untrack(() => {
					if (this.drafts && key && key !== this.checkedKey) this.scheduleCheck();
				});
			});
		});
	}

	/** Read what the service holds: the run and its configuration. */
	async sync(): Promise<void> {
		const c = this.control;
		try {
			const [status, served] = await Promise.all([c.request<RecorderState>('recorder.state'), c.request<RecorderConfigResult>('recorder.config')]);
			this.syncError = null;
			this.served = served;
			this.onStatus(status);
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		if (this.drafts) this.scheduleCheck(0);
	}

	private onStatus(s: RecorderState) {
		const was = this.status;
		// A reply older than the run's own word that it finished is not applied.
		if (regresses(was, s)) return;
		this.status = s;
		// The state is newer than the live frame held: it is the run's word until the next frame.
		this.liveNewer = false;
		if (this.live && this.live.run_id === s.run_id && (s.state === 'unconfigured' || s.state === 'configured' || s.state === 'starting')) this.live = null;
		if (s.run_id !== this.logRun) void this.loadLog(s.run_id);
		if (was?.run_id !== s.run_id) {
			this.answer = null;
			if (s.run_id) void this.readServed();
		}
		this.listsFollow(s);
		// The run went live from this screen or another: the run is the next thing to watch.
		if (recorderIsLive(s) && !recorderIsLive(was) && this.step === 'setup') this.step = 'run';
	}

	private onFrame(l: RecorderLiveHead) {
		const s = this.status;
		if (s?.run_id && l.run_id !== s.run_id) return;
		// A frame older than the run's word that it finished does not take the screen back.
		if (regresses(s, l)) return;
		this.live = l;
		this.liveNewer = true;
		this.listsFollow(l);
	}

	/** The run's log from the start, after a new run or a reconnection. */
	async loadLog(runId: string | null): Promise<void> {
		this.logRun = runId;
		if (!runId) {
			this.log = [];
			this.live = null;
			return;
		}
		try {
			const res = await this.control.request<RecorderLogResult>('recorder.log', { since: 0 });
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
			this.served = await this.control.request<RecorderConfigResult>('recorder.config');
		} catch {
			/* kept as it was */
		}
	}

	// ---- the recordings and the marks ------------------------------------------------------

	/**
	 * The lists follow the run's counters: a recording saved or begun, a mark made, the run ended.
	 * Each counter is held state on the service, so a missed frame never loses a change.
	 */
	private listsFollow(h: { run_id: string | null; state: string; recordings: number; marks: number; recording: { name: string } | null }) {
		const runId = h.run_id;
		if (runId !== this.listRun) {
			this.listRun = runId;
			this.recordings = [];
			this.marks = [];
		}
		if (!runId || h.state === 'unconfigured' || h.state === 'configured') return;
		const saved = this.recordings.filter((r) => r.complete).length;
		const open = this.recordings.find((r) => !r.complete)?.name ?? null;
		const stale = saved !== h.recordings || open !== (h.recording?.name ?? null) || this.marks.length < h.marks;
		if (stale) void this.readLists();
	}

	/** Read what changed: the recordings from the first not yet saved, and the marks not yet held. */
	async readLists(): Promise<void> {
		if (this.listing) {
			this.listAgain = true;
			return;
		}
		const runId = this.listRun;
		if (!runId) return;
		this.listing = true;
		try {
			const kept = this.recordings.filter((r) => r.complete);
			const [recs, marks] = await Promise.all([
				this.control.request<RecorderRecordingsResult>('recorder.recordings', { since: kept.length }),
				this.control.request<RecorderMarksResult>('recorder.marks', { since: this.marks.length })
			]);
			if (this.listRun !== runId) return;
			this.recordings = [...kept, ...recs.recordings];
			if (marks.marks.length) this.marks = [...this.marks, ...marks.marks];
		} catch {
			/* read again with the next change */
		} finally {
			this.listing = false;
			if (this.listAgain) {
				this.listAgain = false;
				void this.readLists();
			}
		}
	}

	/** Start the draft from the service's configuration (the one the run holds). */
	adoptServed(): boolean {
		const c = this.served?.config;
		if (!c || this.locked) return false;
		this.setup = fromRecorderConfig(c);
		return true;
	}

	// =========================================================================================
	// The dry run

	scheduleCheck(delay = this.checkDelayMs) {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	/** The service's verdict on the draft (`dry_run`: needs no control, replaces nothing). */
	async checkSetup(): Promise<RecorderDryRunResult | null> {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = null;
		if (!this.connected || errorCount(this.local) > 0) return null;
		const config = this.wire;
		const key = this.key;
		const seq = ++this.checkSeq;
		this.checking = true;
		try {
			const res = await this.control.request<RecorderDryRunResult>('recorder.configure', { config, dry_run: true });
			if (seq === this.checkSeq) {
				this.check = res;
				this.checkedKey = key;
			}
			return res;
		} catch (err) {
			// A configuration whose shape is wrong is refused by parameter; show it as a finding there.
			const r = toRefusal('Check the setup', err);
			if (seq === this.checkSeq && r.reason === 'bad_params') {
				const field = typeof r.detail?.param === 'string' ? r.detail.param : 'setup';
				this.check = { errors: [{ reason: 'out_of_range', field, message: r.message }], warnings: [], derived: null };
				this.checkedKey = key;
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

	/** Send the setup. A table with edits is applied first, since the run records its rows. */
	async apply(): Promise<boolean> {
		if (!this.actions.apply.enabled) return false;
		if (this.acq.tableDirty) {
			const ok = await this.acq.applyTable();
			if (!ok) return false;
		}
		const config = this.wire;
		const key = this.key;
		const res = await this.act<RecorderConfigureResult>('Apply Setup', 'recorder.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				const d = r.detail as unknown as { errors?: RecorderDryRunResult['errors']; warnings?: RecorderDryRunResult['warnings'] };
				this.check = { errors: d.errors ?? [], warnings: d.warnings ?? [], derived: null };
				this.checkedKey = key;
			}
			return false;
		}
		this.status = res.state;
		this.live = null;
		this.answer = null;
		this.setHeld({ runId: res.state.run_id, key });
		this.served = { run_id: res.state.run_id, config, derived: res.derived };
		void this.loadLog(res.state.run_id);
		void this.readServed();
		this.acq.note = { tone: 'pass', text: 'The service holds this setup.' };
		return true;
	}

	/** The station opens with no outputs and acquisition starts, in one step. */
	async start(): Promise<boolean> {
		const res = await this.act<RecorderState>('Start', 'recorder.start');
		if (res) {
			this.onStatus(res);
			this.step = 'run';
			void this.readServed();
		}
		return res !== null;
	}

	/** The run bar's Arm, Proceed, level and ladder are not the Recorder's: the bar never offers them. */
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

	private fsNow(): number | null {
		return this.now?.grid?.fs_hz ?? null;
	}

	/** Arm the trigger: the pre-window fills from the next batch, and in level mode the detector waits. */
	async armTrigger(): Promise<RecorderArmResult | null> {
		const res = await this.act<RecorderArmResult>('Arm', 'recorder.arm');
		if (res) {
			const fs = this.fsNow();
			this.answer = { verb: 'arm', result: res, words: `Armed: the pre-window of ${fs ? secs(res.pre_frames / fs) : `${res.pre_frames} frames`} fills from the next batch` };
		}
		return res;
	}

	/** Back to monitoring; the pre-window is dropped. */
	async disarm(): Promise<boolean> {
		const res = await this.act<RecorderState>('Disarm', 'recorder.disarm');
		if (res) {
			this.onStatus(res);
			this.answer = { verb: 'disarm', result: res, words: 'Disarmed: monitoring, nothing is being saved' };
		}
		return res !== null;
	}

	/** Begin a recording at the next batch, with the pre-window held when armed. */
	async record(): Promise<RecorderRecordResult | null> {
		const res = await this.act<RecorderRecordResult>('Record', 'recorder.record');
		if (res) {
			const fs = this.fsNow();
			const pre = res.pre_frames ? `, with ${fs ? secs(res.pre_frames / fs) : `${res.pre_frames} frames`} before it` : ', with no pre-window';
			this.answer = { verb: 'record', result: res, words: `${res.name} began at station frame ${res.station_frame}${pre}` };
		}
		return res;
	}

	/** End the recording in progress. */
	async cut(): Promise<RecorderCutResult | null> {
		const res = await this.act<RecorderCutResult>('Cut', 'recorder.cut');
		if (res) this.answer = { verb: 'cut', result: res, words: `${res.name} ends at station frame ${res.last_station_frame}` };
		return res;
	}

	async stop(): Promise<boolean> {
		const res = await this.act<RecorderState>('Stop', 'recorder.stop', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async abort(): Promise<boolean> {
		const res = await this.act<RecorderState>('Abort', 'recorder.abort', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** A new run from the same configuration. */
	async reset(): Promise<boolean> {
		const res = await this.act<RecorderState>('New Run', 'recorder.reset');
		if (!res) return false;
		this.onStatus(res);
		this.live = null;
		this.answer = null;
		if (this.held && this.setupStatus !== 'changed') this.setHeld({ runId: res.run_id, key: this.key });
		return true;
	}

	private async act<R>(verb: string, method: string, params: Record<string, unknown> = {}, exclusive = true): Promise<R | null> {
		if (exclusive) this.busy = verb;
		this.#refusal = null;
		try {
			return await this.control.request<R>(method, params);
		} catch (err) {
			const r = toRefusal(verb, err);
			// The Recorder's own refusals, in its own words.
			if (r.reason === 'wrong_state') {
				const fact = recorderStateFact(r.detail);
				if (fact) r.facts = [fact];
			} else if (r.reason === 'wrong_mode') {
				r.title = 'Not in This Trigger Mode';
				const fact = recorderModeFact(r.detail);
				if (fact) r.facts = [fact];
			} else if (r.reason === 'not_enough_space') {
				r.title = 'Not Enough Free Space';
				r.facts = recorderSpaceFacts(r.detail);
			}
			this.#refusal = r;
			return null;
		} finally {
			if (exclusive && this.busy === verb) this.busy = null;
		}
	}
}
