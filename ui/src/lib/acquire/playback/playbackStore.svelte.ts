/**
 * Playback's working state, against the control service's `playback.*` methods (schema/PROTOCOL.md,
 * "Playback") and the analysis service's `drive.*` methods, which read the operator's file and
 * write the native drive folder the control service plays.
 *
 * The station, the channel table and the controller seat are the service's and shared by every
 * test type: they stay in the Acquire store, and this one reads them from there. What is
 * Playback's own is here: the setup draft (kept across reloads), the file being converted, the dry
 * run of the draft with the drive file's check report, the run's state and its live progress, the
 * run's log, and the verbs. The control service checks the file and holds every protection; this
 * store keeps the operator's edits beside it and sends them when the operator says so. A converted
 * or fixed folder fills the draft and nothing else, and is checked like any setup typed in.
 */

import { untrack } from 'svelte';
import { regresses } from '../runOrder';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelRow, ProjectObject } from '$lib/protocol/types';
import {
	isFullCheck,
	isRun,
	PLAYBACK_TOPICS,
	type DriveConvertResult,
	type DriveFixEndsResult,
	type DriveInspectResult,
	type PlaybackCheck,
	type PlaybackCheckReport,
	type PlaybackConfig,
	type PlaybackConfigResult,
	type PlaybackConfigureResult,
	type PlaybackDerived,
	type PlaybackDryRunResult,
	type PlaybackInvalidDetail,
	type PlaybackLive,
	type PlaybackLogEntry,
	type PlaybackLogEvent,
	type PlaybackLogResult,
	type PlaybackRun,
	type PlaybackState
} from '$lib/protocol/playback';
import type { ActionId, ActionState, RunHead, SetupStatus } from '../actions';
import { canonical } from '../mimo/mimoSetup';
import { errorCount, type FieldErrors } from '../setup';
import type { AcquireState } from '../store.svelte';
import { toRefusal, type Refusal } from '../words';
import { defaultPlaybackSetup, fitMap, fromPlaybackConfig, localErrors, mergePlaybackSetup, PLAYBACK_SECTIONS, playbackByKey, playbackRows, playbackSectionOf, toPlaybackConfig, type PlaybackDraft, type PlaybackSection } from './playbackSetup';
import { playbackBarActions, playbackIsLive, playbackPrimary, type PlaybackVerbContext } from './playbackVerbs';
import { PLAYBACK_OUTCOME_WORDS, PLAYBACK_PHASE_WORDS, PLAYBACK_STATE_WORDS } from './playbackWords';

const SETUP_KEY = 'hertzforge.acquire.playback.setup.v1';
/** The setup this screen applied, by run: `{runId, key}`. */
const HELD_KEY = 'hertzforge.acquire.playback.held.v1';
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

export function savePlaybackSetup(d: PlaybackDraft): string {
	return JSON.stringify(d, nanOut);
}

export function loadPlaybackSetup(raw: string | null): PlaybackDraft {
	if (!raw) return defaultPlaybackSetup();
	try {
		return mergePlaybackSetup(JSON.parse(raw, nanIn) as Partial<PlaybackDraft>);
	} catch {
		return defaultPlaybackSetup();
	}
}

export interface PlaybackOptions {
	local?: Storage | null;
	/** Delay before a dry run after an edit, ms. */
	checkDelayMs?: number;
	/** A display window observes: it keeps no draft and checks nothing. */
	drafts?: boolean;
	/** The analysis service, which reads the operator's file and writes the drive folder. */
	analysis?: ServiceClient | null;
	/** The open project's objects, for a drive taken from its time histories. */
	objects?: () => ProjectObject[];
}

/** The file being made into a drive: where it is, what it holds, and what is chosen. */
export interface DriveSource {
	/** A file's full path on this computer, or the project's time histories by name. */
	from: { path: string } | { objects: string[] };
	inspected: DriveInspectResult | null;
	/** The signals chosen, by key, in drive order. */
	chosen: string[];
	/** The rate to give when the file states none; NaN until typed. */
	rate_hz: number;
}

/** A fix the analysis side wrote, waiting for the operator to use it or leave it. */
export interface PendingFix {
	result: DriveFixEndsResult;
	/** The folder and the check it replaces, as they were. */
	was: { folder: string; sha256: string };
}

export class PlaybackStore {
	readonly control: ServiceClient;
	readonly analysis: ServiceClient | null;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;
	private readonly objectsOf: () => ProjectObject[];

	// ---- the operator's draft --------------------------------------------------------------
	setup = $state<PlaybackDraft>(defaultPlaybackSetup());
	section = $state<PlaybackSection>('drive');
	step = $state<'setup' | 'run'>('setup');

	// ---- the file being converted (not the setup: the folder's sidecar keeps where it came from)
	driveSource = $state<DriveSource>({ from: { path: '' }, inspected: null, chosen: [], rate_hz: NaN });
	/** The last conversion's notes and the folder it wrote. */
	converted = $state.raw<DriveConvertResult | null>(null);
	/** A Fix Ends written and shown, not yet used. */
	fix = $state.raw<PendingFix | null>(null);
	/** A drive-file request in flight, in words, or null. */
	working = $state<string | null>(null);
	/** Why the last drive-file request was refused, in the service's words, or null. */
	fileError = $state<string | null>(null);

	// ---- from the service ------------------------------------------------------------------
	status = $state.raw<PlaybackState | null>(null);
	live = $state.raw<PlaybackLive | null>(null);
	served = $state.raw<PlaybackConfigResult | null>(null);
	check = $state.raw<PlaybackDryRunResult | null>(null);
	checkedKey = $state('');
	checking = $state(false);
	log = $state.raw<PlaybackLogEntry[]>([]);
	held = $state.raw<{ runId: string | null; key: string } | null>(null);

	// ---- feedback --------------------------------------------------------------------------
	busy = $state<string | null>(null);
	#refusal = $state.raw<Refusal | null>(null);
	syncError = $state<string | null>(null);

	private liveNewer = $state(false);
	private checkSeq = 0;
	private logRun: string | null = null;
	private checkTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(
		readonly acq: AcquireState,
		options: PlaybackOptions = {}
	) {
		this.control = acq.control;
		this.analysis = options.analysis ?? null;
		this.objectsOf = options.objects ?? (() => []);
		this.localStore = options.local === undefined ? storageOf('localStorage') : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 300;
		this.drafts = options.drafts ?? true;
		this.setup = loadPlaybackSetup(read(this.localStore, SETUP_KEY));
		const held = read(this.localStore, HELD_KEY);
		if (held) {
			try {
				this.held = JSON.parse(held);
			} catch {
				this.held = null;
			}
		}
		this.control.on(PLAYBACK_TOPICS.state, (d) => this.onStatus(d as unknown as PlaybackState));
		this.control.on(PLAYBACK_TOPICS.live, (d) => this.onFrame(d as unknown as PlaybackLive));
		this.control.on(PLAYBACK_TOPICS.log, (d) => {
			const e = d as unknown as PlaybackLogEvent;
			if (this.run_?.run_id && e.run_id !== this.run_.run_id) return;
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

	/** The run the service holds, or null when it holds none. */
	private get run_(): PlaybackRun | null {
		return isRun(this.status) ? this.status : null;
	}

	/** The rows a run records: every enabled input and every enabled drive (the Run Record panel's count). */
	get recordChannels(): number {
		const r = playbackRows(this.rows);
		return r.inputs.length + r.drives.length;
	}

	/** The run's rate is the file's: the last check's, else the converted folder's. */
	get recordRateHz(): number {
		const c = this.checkReport;
		return c?.rate_hz ?? this.converted?.drive.rate_hz ?? NaN;
	}

	get isLive(): boolean {
		return playbackIsLive(this.status);
	}

	get locked(): boolean {
		return this.isLive || this.acq.isLive;
	}

	/** The project's time histories, which a drive can be taken from. */
	get timeObjects(): ProjectObject[] {
		return this.objectsOf().filter((o) => o.kind === 'time_history');
	}

	readonly wire = $derived.by((): PlaybackConfig => toPlaybackConfig(this.setup));
	readonly key = $derived(canonical(this.wire));
	readonly local = $derived.by((): FieldErrors => localErrors(this.setup));

	/** The dry run for the draft as it stands, or null. */
	get current(): PlaybackDryRunResult | null {
		return this.check && this.checkedKey === this.key ? this.check : null;
	}

	/** The drive file's check: the draft's dry run, else the run's own once applied. */
	get checkOf(): PlaybackCheck | null {
		return this.current?.check ?? (this.setupStatus === 'applied' ? (this.served?.check ?? null) : null);
	}

	/** The check report with its numbers, or null when there is none (or the file could not be read). */
	get checkReport(): PlaybackCheckReport | null {
		const c = this.checkOf;
		return isFullCheck(c) ? c : null;
	}

	get fieldErrors(): FieldErrors {
		const fromCheck = this.current ? playbackByKey(this.current.errors) : {};
		const out: FieldErrors = {};
		for (const map of [this.local, fromCheck]) for (const [k, v] of Object.entries(map)) for (const m of v) if (!(out[k] ??= []).includes(m)) out[k].push(m);
		return out;
	}

	get fieldWarnings(): FieldErrors {
		return this.current ? playbackByKey(this.current.warnings) : {};
	}

	get counts(): Record<PlaybackSection, { errors: number; warnings: number }> {
		const out = Object.fromEntries(PLAYBACK_SECTIONS.map((s) => [s.id, { errors: 0, warnings: 0 }])) as Record<PlaybackSection, { errors: number; warnings: number }>;
		for (const [k, v] of Object.entries(this.fieldErrors)) out[playbackSectionOf(k)].errors += v.length;
		for (const [k, v] of Object.entries(this.fieldWarnings)) out[playbackSectionOf(k)].warnings += v.length;
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

	get pending(): string | null {
		if (this.acq.stationDirty) return 'Use the chosen station, or put the choice back, first.';
		return null;
	}

	get runConfig(): PlaybackConfig | null {
		return this.served?.config ?? null;
	}

	get derived(): PlaybackDerived | null {
		return this.current?.derived ?? this.served?.derived ?? null;
	}

	/** The state, with this run's live progress laid over it when the frame came after it. */
	get now(): PlaybackRun | null {
		const s = this.run_;
		const l = this.live;
		if (!s || !l || l.run_id !== s.run_id || !this.liveNewer) return s;
		return { ...s, state: l.state, phase: l.phase, progress: l.progress };
	}

	get folder(): string | null {
		return this.run_?.record?.folder ?? null;
	}

	private get verbContext(): PlaybackVerbContext {
		const n = this.now;
		return {
			connected: this.connected,
			controller: this.isController,
			run: n ? { state: n.state, phase: n.phase } : null,
			setup: this.setupStatus,
			setupErrors: this.setupErrorCount,
			busy: this.busy ?? this.working,
			pending: this.pending
		};
	}

	get actions(): Record<ActionId, ActionState> {
		return playbackBarActions(this.verbContext);
	}

	get primary(): ActionId | null {
		return playbackPrimary(this.verbContext);
	}

	// ---- the run as the shared run bar and notices read it ---------------------------------

	get run(): RunHead | null {
		const s = this.now;
		if (!s) return this.status ? { run_id: null, state: 'unconfigured', phase: 'idle', title: null, mode: null, levels_db: null, level_db: null, at_level_s: null, remaining_at_level_s: null, end: null, record: null } : null;
		return {
			run_id: s.run_id,
			state: s.state as RunHead['state'],
			phase: s.phase,
			title: s.title,
			mode: null,
			levels_db: null,
			level_db: s.level_db,
			at_level_s: null,
			remaining_at_level_s: null,
			end: s.end ? ({ ...s.end, level_db: s.level_db } as unknown as RunHead['end']) : null,
			record: s.record
		};
	}

	/** The level is fixed for the run: the bar shows no level readout, and the play's progress takes its place. */
	readonly frame = null;
	readonly levelHidden = true;

	readonly outcomeWords: Record<string, string> = PLAYBACK_OUTCOME_WORDS;
	readonly stateWords: Record<string, string> = PLAYBACK_STATE_WORDS;
	readonly phaseWords: Record<string, string> = PLAYBACK_PHASE_WORDS;

	get elapsed(): number | null {
		return this.now?.progress.elapsed_s ?? null;
	}

	get source(): { source: 'SIM' | 'HARDWARE'; cls: 'sim' | 'ni_simulated' | 'ni_real' } | null {
		const s = this.run_;
		if (s) return { source: s.source, cls: s.class };
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
				const text = savePlaybackSetup($state.snapshot(this.setup) as PlaybackDraft);
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

	async sync(): Promise<void> {
		const c = this.control;
		try {
			const [status, served] = await Promise.all([c.request<PlaybackState>('playback.state'), c.request<PlaybackConfigResult>('playback.config')]);
			this.syncError = null;
			this.served = served;
			this.onStatus(status);
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		if (this.drafts) this.scheduleCheck(0);
	}

	private onStatus(s: PlaybackState) {
		const was = this.status;
		if (regresses(was, s)) return;
		this.status = s;
		this.liveNewer = false;
		if (this.live && (s.state === 'unconfigured' || (this.live.run_id === s.run_id && s.state === 'configured'))) this.live = null;
		if (s.run_id !== this.logRun) void this.loadLog(s.run_id);
		if (was?.run_id !== s.run_id && s.run_id) void this.readServed();
		if (playbackIsLive(s) && !playbackIsLive(was) && this.step === 'setup') this.step = 'run';
	}

	private onFrame(l: PlaybackLive) {
		const s = this.run_;
		if (s?.run_id && l.run_id !== s.run_id) return;
		if (regresses(s, l)) return;
		this.live = l;
		this.liveNewer = true;
	}

	async loadLog(runId: string | null): Promise<void> {
		if (runId !== this.logRun) this.log = [];
		this.logRun = runId;
		if (!runId) {
			this.live = null;
			return;
		}
		try {
			const res = await this.control.request<PlaybackLogResult>('playback.log', { since: 0 });
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
			this.served = await this.control.request<PlaybackConfigResult>('playback.config');
		} catch {
			/* kept as it was */
		}
	}

	/** Start the draft from the service's configuration (the one the run holds). */
	adoptServed(): boolean {
		const c = this.served?.config;
		if (!c || this.locked) return false;
		this.setup = fromPlaybackConfig(c);
		return true;
	}

	// =========================================================================================
	// The drive file, on the analysis side

	private async file<R>(what: string, method: string, params: Record<string, unknown>): Promise<R | null> {
		this.fileError = null;
		if (!this.analysis?.isOpen) {
			this.fileError = 'The analysis service is not connected: it reads the file and writes the drive.';
			return null;
		}
		this.working = what;
		try {
			return await this.analysis.request<R>(method, params);
		} catch (err) {
			this.fileError = toRefusal(what, err).message;
			return null;
		} finally {
			if (this.working === what) this.working = null;
		}
	}

	/** What the file at `path` (or the project's objects) holds; every usable signal is chosen, in file order. */
	async inspect(from: DriveSource['from']): Promise<boolean> {
		const res = await this.file<DriveInspectResult>('Read the File', 'drive.inspect', { ...from });
		if (!res) return false;
		this.driveSource = { from, inspected: res, chosen: res.signals.filter((s) => !s.problem).map((s) => s.key), rate_hz: res.rate_hz ?? NaN };
		return true;
	}

	/** Move a chosen signal one place earlier (-1) or later (+1) in drive order. */
	move(key: string, by: -1 | 1) {
		const c = [...this.driveSource.chosen];
		const i = c.indexOf(key);
		const j = i + by;
		if (i < 0 || j < 0 || j >= c.length) return;
		[c[i], c[j]] = [c[j], c[i]];
		this.driveSource.chosen = c;
	}

	toggle(key: string, on: boolean) {
		const c = this.driveSource.chosen.filter((k) => k !== key);
		this.driveSource.chosen = on ? [...c, key] : c;
	}

	/** The rate the conversion needs given: the file's own, or the one typed when the file states none. */
	get rateNeeded(): boolean {
		return this.driveSource.inspected !== null && this.driveSource.inspected.rate_hz === null;
	}

	/** Why Convert waits, or null. */
	get convertWhy(): string | null {
		if (this.locked) return 'The run is live; the setup is locked until it ends.';
		if (!this.driveSource.inspected) return 'Read a file first.';
		if (!this.driveSource.chosen.length) return 'Choose at least one signal.';
		if (this.rateNeeded && !(Number.isFinite(this.driveSource.rate_hz) && this.driveSource.rate_hz > 0)) return 'The file states no rate: give the rate it was made at.';
		return null;
	}

	/** Write the chosen signals to a new drive folder, and set the draft to play it. */
	async convert(): Promise<boolean> {
		if (this.convertWhy) return false;
		const s = this.driveSource;
		const params: Record<string, unknown> = { ...s.from, selections: [...s.chosen] };
		if (this.rateNeeded) params.rate_hz = s.rate_hz;
		const res = await this.file<DriveConvertResult>('Convert', 'drive.convert', params);
		if (!res) return false;
		this.converted = res;
		this.fix = null;
		this.useFolder(res.folder, res.drive.sha256, res.drive.channels.length);
		return true;
	}

	/** The draft plays `folder`, its pin `sha256`, each of its channels on a drive row. */
	private useFolder(folder: string, sha256: string, channels: number) {
		this.setup.drive = { folder, sha256 };
		this.setup.map = fitMap(this.setup.map, channels, playbackRows(this.rows).drives);
	}

	/** Write a copy of the drive with its ends at rest; shown before and after, and used only when accepted. */
	async fixEnds(taper_s?: number): Promise<boolean> {
		if (this.locked || !this.setup.drive.folder) return false;
		const params: Record<string, unknown> = { folder: this.setup.drive.folder };
		if (taper_s !== undefined && Number.isFinite(taper_s)) params.taper_s = taper_s;
		const res = await this.file<DriveFixEndsResult>('Fix Ends', 'drive.fix_ends', params);
		if (!res) return false;
		this.fix = { result: res, was: { ...this.setup.drive } };
		return true;
	}

	/** Play the fixed folder: the draft takes it, and the control service checks it as any other. */
	acceptFix(): boolean {
		const f = this.fix;
		if (!f || this.locked) return false;
		this.useFolder(f.result.folder, f.result.drive.sha256, f.result.drive.channels.length);
		this.fix = null;
		return true;
	}

	/** Leave the fix unused: the draft keeps the folder it had. The fixed folder stays on disk. */
	discardFix() {
		this.fix = null;
	}

	// =========================================================================================
	// The dry run

	scheduleCheck(delay = this.checkDelayMs) {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	/** The service's verdict on the draft, with the drive file's check (`dry_run`: needs no control). */
	async checkSetup(): Promise<PlaybackDryRunResult | null> {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = null;
		if (!this.connected || errorCount(this.local) > 0) return null;
		const config = this.wire;
		const key = this.key;
		const seq = ++this.checkSeq;
		this.checking = true;
		try {
			const res = await this.control.request<PlaybackDryRunResult>('playback.configure', { config, dry_run: true });
			if (seq === this.checkSeq) {
				this.check = res;
				this.checkedKey = key;
			}
			return res;
		} catch (err) {
			const r = toRefusal('Check the setup', err);
			if (seq === this.checkSeq && r.reason === 'bad_params') {
				const field = typeof r.detail?.param === 'string' ? r.detail.param : 'setup';
				this.check = { errors: [{ reason: 'out_of_range', field, message: r.message }], warnings: [], check: { pass: false, refusals: [] } as unknown as PlaybackCheck, derived: null };
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

	/** Send the setup. A table with edits is applied first, since the map names its drive rows. */
	async apply(): Promise<boolean> {
		if (!this.actions.apply.enabled) return false;
		if (this.acq.tableDirty) {
			const ok = await this.acq.applyTable();
			if (!ok) return false;
		}
		const config = this.wire;
		const key = this.key;
		const res = await this.act<PlaybackConfigureResult>('Apply Setup', 'playback.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				const d = r.detail as unknown as PlaybackInvalidDetail;
				this.check = { errors: d.errors ?? [], warnings: d.warnings ?? [], check: d.check ?? ({ pass: false, refusals: [] } as unknown as PlaybackCheck), derived: null };
				this.checkedKey = key;
			}
			return false;
		}
		this.status = res.state;
		this.live = null;
		this.setHeld({ runId: res.state.run_id, key });
		this.served = { run_id: res.state.run_id, config, check: res.check, derived: res.derived };
		void this.loadLog(res.state.run_id);
		void this.readServed();
		this.acq.note = { tone: 'pass', text: 'The service holds this setup; the drive file checked clean.' };
		return true;
	}

	/** The file is checked again, the station opens at its rate and the pre-roll starts, in one step. */
	async start(): Promise<boolean> {
		const res = await this.act<PlaybackState>('Start', 'playback.start');
		if (res) {
			this.onStatus(res);
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

	async stop(): Promise<boolean> {
		const res = await this.act<PlaybackState>('Stop', 'playback.stop', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async abort(): Promise<boolean> {
		const res = await this.act<PlaybackState>('Abort', 'playback.abort', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** A new run from the same configuration: the file is checked again. */
	async reset(): Promise<boolean> {
		const res = await this.act<PlaybackState>('New Run', 'playback.reset');
		if (!res) return false;
		this.onStatus(res);
		this.live = null;
		if (this.held && this.setupStatus !== 'changed') this.setHeld({ runId: res.run_id, key: this.key });
		return true;
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
