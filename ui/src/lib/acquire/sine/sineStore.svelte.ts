/**
 * Sine's working state, against the control service's `sine.*` and `laws.*` methods
 * (schema/PROTOCOL.md, "Sine", "Control Laws").
 *
 * The station, the channel table and the controller seat are the service's and shared by every
 * test type: they stay in the Acquire store, and this one reads them from there. What is Sine's own
 * is here: the setup draft (kept across reloads), the service's reading of the draft's profile
 * (`sine.profile.validate`) and of rows pasted or read from a file (`sine.profile.import`), the dry
 * run of the setup, the law catalog of the Sine world, the run's state and its live numbers, and
 * the verbs. As everywhere in Acquire, the service holds the truth and computes every number; this
 * store keeps the operator's edits beside it and sends them when the operator says so.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelRow, LadderMode, LogEntry } from '$lib/protocol/types';
import type { LawEntry, LawsListResult } from '$lib/protocol/mimo';
import {
	SINE_TOPICS,
	type SineAdjustResult,
	type SineConfig,
	type SineConfigResult,
	type SineConfigureResult,
	type SineDryRunResult,
	type SineFrequencyResult,
	type SineLevelResult,
	type SineLive,
	type SineLogEvent,
	type SineLogResult,
	type SineModeResult,
	type SineProfileImportResult,
	type SineProfileValidateResult,
	type SineReportItem,
	type SineState
} from '$lib/protocol/sine';
import type { ActionId, ActionState, RunHead, SetupStatus } from '../actions';
import { lawParamErrors, lawsOfWorld, lawValues } from '../law/lawForm';
import { canonical } from '../mimo/mimoSetup';
import { errorCount, mergeErrors, type FieldErrors } from '../setup';
import type { AcquireState } from '../store.svelte';
import { toRefusal, type Refusal } from '../words';
import {
	bandOf,
	defaultSineSetup,
	fromSineConfig,
	localErrors,
	mergeSineSetup,
	newLimitChannel,
	profileDraftOf,
	SINE_SECTIONS,
	sineByKey,
	sineSectionOf,
	toSineConfig,
	validateParams,
	type SineDraft,
	type SineSection
} from './sineSetup';
import { liveParams, sineBarActions, sineIsLive, sineModuleVerbs, sinePrimary, type LiveParamState, type SineVerbContext, type SineVerbId } from './sineVerbs';
import { SINE_OUTCOME_WORDS, SINE_PHASE_WORDS } from './sineWords';

const SETUP_KEY = 'hertzforge.acquire.sine.setup.v1';
/** The setup this screen applied, by run: `{runId, key}`. */
const HELD_KEY = 'hertzforge.acquire.sine.held.v1';
/** Most log entries kept on screen; the service keeps the full log with the run. */
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

/** JSON that keeps NaN (a field the operator has emptied) through a reload. */
const nanOut = (_k: string, v: unknown) => (typeof v === 'number' && Number.isNaN(v) ? '__NaN__' : v);
const nanIn = (_k: string, v: unknown) => (v === '__NaN__' ? NaN : v);

export function saveSineSetup(d: SineDraft): string {
	return JSON.stringify(d, nanOut);
}

export function loadSineSetup(raw: string | null): SineDraft {
	if (!raw) return defaultSineSetup();
	try {
		return mergeSineSetup(JSON.parse(raw, nanIn) as Partial<SineDraft>);
	} catch {
		return defaultSineSetup();
	}
}

export interface SineOptions {
	local?: Storage | null;
	/** Delay before a dry run or a profile check after an edit, ms. */
	checkDelayMs?: number;
	/** A display window observes: it keeps no draft and checks nothing. */
	drafts?: boolean;
}

/** What reading profile rows said, for the profile editor. */
export interface ProfileImportNote {
	/** The file read, or null for pasted rows. */
	path: string | null;
	errors: SineReportItem[];
	warnings: SineReportItem[];
	/** Rows taken into the draft. */
	rows: number;
}

export class SineStore {
	readonly control: ServiceClient;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;

	// ---- the operator's draft --------------------------------------------------------------
	setup = $state<SineDraft>(defaultSineSetup());
	section = $state<SineSection>('profile');
	/** Where the operator is: the setup, or the run. */
	step = $state<'setup' | 'run'>('setup');

	// ---- from the service ------------------------------------------------------------------
	status = $state.raw<SineState | null>(null);
	live = $state.raw<SineLive | null>(null);
	/** What the service holds: the configuration of the present run. */
	served = $state.raw<SineConfigResult | null>(null);
	/** The dry run of the draft's setup, and the key of the setup it checked. */
	check = $state.raw<SineDryRunResult | null>(null);
	checkedKey = $state('');
	checking = $state(false);
	/** The service's reading of the draft's profile, and the key of what it read. */
	validation = $state.raw<SineProfileValidateResult | null>(null);
	validationKey = $state('');
	validating = $state(false);
	/** Why the service would not read the profile as it stands, in its words. */
	validationError = $state<string | null>(null);
	laws = $state.raw<LawEntry[] | null>(null);
	lawProblems = $state.raw<{ file: string; reason: string }[]>([]);
	/** The last rows pasted or read from a file, and what the service said about them. */
	imported = $state.raw<ProfileImportNote | null>(null);
	log = $state.raw<LogEntry[]>([]);
	/** The setup this screen applied: the run it made and the setup's key. */
	held = $state.raw<{ runId: string | null; key: string } | null>(null);

	// ---- feedback --------------------------------------------------------------------------
	busy = $state<string | null>(null);
	#refusal = $state.raw<Refusal | null>(null);
	syncError = $state<string | null>(null);

	private checkSeq = 0;
	private validateSeq = 0;
	private logRun: string | null = null;
	private checkTimer: ReturnType<typeof setTimeout> | null = null;
	private validateTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(
		readonly acq: AcquireState,
		options: SineOptions = {}
	) {
		this.control = acq.control;
		this.localStore = options.local === undefined ? storageOf('localStorage') : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 300;
		this.drafts = options.drafts ?? true;
		this.setup = loadSineSetup(read(this.localStore, SETUP_KEY));
		const held = read(this.localStore, HELD_KEY);
		if (held) {
			try {
				this.held = JSON.parse(held);
			} catch {
				this.held = null;
			}
		}
		this.control.on(SINE_TOPICS.state, (d) => this.onStatus(d as unknown as SineState));
		this.control.on(SINE_TOPICS.live, (d) => {
			const l = d as unknown as SineLive;
			if (this.status?.run_id && l.run_id !== this.status.run_id) return;
			this.live = l;
		});
		this.control.on(SINE_TOPICS.log, (d) => {
			const e = d as unknown as SineLogEvent;
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
		return sineIsLive(this.status);
	}

	/** Setup, station and table are locked while a run is live. */
	get locked(): boolean {
		return this.isLive || this.acq.isLive;
	}

	/** The Sine world's laws: built in first, then imported, as the service lists them. */
	get sineLaws(): LawEntry[] {
		return lawsOfWorld(this.laws, 'sine');
	}

	/** The law chosen, from the catalog. */
	get law(): LawEntry | null {
		return this.sineLaws.find((l) => l.id === this.setup.law.id) ?? null;
	}

	readonly wire = $derived.by((): SineConfig => toSineConfig(this.setup, this.law, this.acq.rows));
	readonly key = $derived(canonical(this.wire));
	readonly validateInput = $derived(validateParams(this.setup));
	readonly validateInputKey = $derived(this.validateInput ? canonical(this.validateInput) : '');

	/** The service's reading of the profile, when it answered the draft's profile as it stands. */
	get profileCheck(): SineProfileValidateResult | null {
		return this.validation && this.validationKey === this.validateInputKey ? this.validation : null;
	}

	/** The test's band as the draft gives it. */
	get band(): [number, number] {
		return bandOf(this.setup);
	}

	readonly local = $derived.by((): FieldErrors => mergeErrors(localErrors(this.setup, this.acq.rows), lawParamErrors(this.law, this.setup.law.params)));

	/** Findings by key: the editor's own and the dry run's for the draft as it stands. */
	get fieldErrors(): FieldErrors {
		const fromCheck = this.check && this.checkedKey === this.key ? sineByKey(this.check.errors) : {};
		const lawMissing: FieldErrors = this.laws && !this.law ? { 'law.id': ['The chosen law is not in the catalog; choose another.'] } : {};
		return mergeErrors(mergeErrors(this.local, fromCheck), lawMissing);
	}

	get fieldWarnings(): FieldErrors {
		return this.check && this.checkedKey === this.key ? sineByKey(this.check.warnings) : {};
	}

	/** Errors and warnings per section. */
	get counts(): Record<SineSection, { errors: number; warnings: number }> {
		const out = Object.fromEntries(SINE_SECTIONS.map((s) => [s.id, { errors: 0, warnings: 0 }])) as Record<SineSection, { errors: number; warnings: number }>;
		for (const [k, v] of Object.entries(this.fieldErrors)) out[sineSectionOf(k)].errors += v.length;
		for (const [k, v] of Object.entries(this.fieldWarnings)) out[sineSectionOf(k)].warnings += v.length;
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
		if (!this.laws) return 'The law catalog has not been read yet.';
		return null;
	}

	/** The configuration the run holds, as the service has it. */
	get runConfig(): SineConfig | null {
		return this.served?.config ?? null;
	}

	/** The latest numbers: the state with the live frame for this run laid over it. */
	get now(): SineState | null {
		const s = this.status;
		const l = this.live;
		if (!s || !l || l.run_id !== s.run_id) return s;
		return {
			...s,
			state: l.state,
			phase: l.phase,
			mode: l.mode,
			level_db: l.commanded_db ?? s.level_db,
			frequency_hz: l.frequency_hz,
			direction: l.direction,
			sweeps: l.sweeps,
			held: l.held,
			elapsed_s: l.elapsed_s ?? s.elapsed_s,
			alarm: l.alarm,
			notch: l.notch,
			law: s.law && l.law ? { ...s.law, misses: l.law.misses, capped_ticks: l.law.capped_ticks } : s.law
		};
	}

	/** The live frame, when it is this run's. */
	get frameOfRun(): SineLive | null {
		const s = this.status;
		const l = this.live;
		return s && l && l.run_id === s.run_id ? l : null;
	}

	private get verbContext(): SineVerbContext {
		return {
			connected: this.connected,
			controller: this.isController,
			run: this.now,
			setup: this.setupStatus,
			setupErrors: this.setupErrorCount,
			busy: this.busy,
			pending: this.pending
		};
	}

	/** The run bar's verbs, by the shared bar's names. */
	get actions(): Record<ActionId, ActionState> {
		return sineBarActions(this.verbContext);
	}

	get primary(): ActionId | null {
		return sinePrimary(this.verbContext);
	}

	/** Hold or Resume, and on a sweep Reverse, Jog and Go to. */
	get verbs(): Record<SineVerbId, ActionState> {
		return sineModuleVerbs(this.verbContext);
	}

	/** The run's law: the one the service runs, else the draft's choice. */
	get runLaw(): LawEntry | null {
		const id = this.status?.law?.id ?? this.served?.derived?.law.id ?? null;
		return (id ? this.sineLaws.find((l) => l.id === id) : null) ?? this.law;
	}

	/** The run's law's number parameters: the live ones changeable during the run, the others fixed. */
	get liveParams(): LiveParamState[] {
		const values = (this.runConfig?.control?.law?.params ?? this.setup.law.params) as Record<string, unknown>;
		return liveParams(this.runLaw, this.now?.law ?? null, values, this.verbContext);
	}

	// ---- the run as the shared run bar and notices read it ---------------------------------

	get run(): RunHead | null {
		const s = this.now;
		if (!s) return null;
		return {
			run_id: s.run_id,
			state: s.state,
			phase: s.phase,
			title: s.title,
			mode: s.mode,
			levels_db: s.levels_db,
			level_db: s.level_db,
			at_level_s: s.at_level_s,
			remaining_at_level_s: null,
			end: s.end as unknown as RunHead['end']
		};
	}

	/** The level the drive is at now, from the live frame, for the bar's level readout. */
	get frame(): { level_db: number | null; at_level_s: number | null; remaining_at_level_s: number | null } | null {
		const l = this.frameOfRun;
		return l ? { level_db: l.level_db, at_level_s: this.status?.at_level_s ?? null, remaining_at_level_s: null } : null;
	}

	/** The run bar's words for Sine's outcomes and phases. */
	readonly outcomeWords: Record<string, string> = SINE_OUTCOME_WORDS;
	readonly phaseWords: Record<string, string> = SINE_PHASE_WORDS;

	get elapsed(): number | null {
		return this.frameOfRun?.elapsed_s ?? this.status?.elapsed_s ?? null;
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

	/** Keep the draft across reloads, and read and re-check it as it changes. Returns the stopper. */
	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				const text = saveSineSetup($state.snapshot(this.setup) as SineDraft);
				untrack(() => write(this.localStore, SETUP_KEY, text));
			});
			$effect(() => {
				const key = this.validateInputKey;
				untrack(() => {
					if (this.drafts && key && key !== this.validationKey) this.scheduleValidate();
				});
			});
			$effect(() => {
				const key = this.key;
				untrack(() => {
					if (this.drafts && key && key !== this.checkedKey) this.scheduleCheck();
				});
			});
		});
	}

	/** Read what the service holds: the run, its configuration, the law catalog. */
	async sync(): Promise<void> {
		const c = this.control;
		try {
			const [status, served, laws] = await Promise.all([c.request<SineState>('sine.state'), c.request<SineConfigResult>('sine.config'), c.request<LawsListResult>('laws.list', { world: 'sine' })]);
			this.syncError = null;
			this.served = served;
			this.onStatus(status);
			this.setLaws(laws);
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		if (!this.drafts) return;
		this.scheduleValidate(0);
		this.scheduleCheck(0);
	}

	private onStatus(s: SineState) {
		const was = this.status;
		this.status = s;
		// The state is newer than the live frame held: its words go over the frame's until the next one.
		const l = this.live;
		if (l && l.run_id === s.run_id) {
			if (s.state === 'unconfigured' || s.state === 'configured' || s.state === 'armed' || s.state === 'blocked') this.live = null;
			else
				this.live = {
					...l,
					state: s.state,
					phase: s.phase,
					mode: s.mode ?? l.mode,
					held: s.held,
					alarm: s.alarm,
					notch: s.notch,
					commanded_db: s.level_db ?? l.commanded_db,
					direction: s.direction ?? l.direction,
					sweeps: s.sweeps ?? l.sweeps
				};
		}
		if (s.run_id !== this.logRun) void this.loadLog(s.run_id);
		if (was?.run_id !== s.run_id && s.run_id) void this.readServed();
		// The run went live from this screen or another: the run is the next thing to watch.
		if (sineIsLive(s) && !sineIsLive(was) && this.step === 'setup' && s.state !== 'armed') this.step = 'run';
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
			const res = await this.control.request<SineLogResult>('sine.log', { since: 0 });
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
			this.served = await this.control.request<SineConfigResult>('sine.config');
		} catch {
			/* kept as it was */
		}
	}

	private setLaws(res: LawsListResult) {
		this.laws = res.laws;
		this.lawProblems = res.problems;
		const law = this.law;
		if (law) this.setup.law.params = lawValues(law, this.setup.law.params, 1);
	}

	/** Choose a law: its parameters start from the operator's last valid values, else its defaults. */
	chooseLaw(id: string) {
		const law = this.sineLaws.find((l) => l.id === id);
		if (!law || this.locked) return;
		this.setup.law = { id, params: lawValues(law, id === this.setup.law.id ? this.setup.law.params : undefined, 1) };
	}

	async refreshLaws(): Promise<void> {
		try {
			this.setLaws(await this.control.request<LawsListResult>('laws.list', { world: 'sine' }));
		} catch (err) {
			this.refusal = toRefusal('Read the control laws', err);
		}
	}

	/** Import a law file by its path on the computer that runs the test. */
	async importLaw(path: string): Promise<boolean> {
		const res = await this.act<{ law: LawEntry }>('Import Law', 'laws.import', { path: path.trim() });
		if (!res) return false;
		await this.refreshLaws();
		if (res.law.world === 'sine') {
			this.chooseLaw(res.law.id);
			this.acq.note = { tone: 'pass', text: `Imported ${res.law.name} ${res.law.version}.` };
		} else this.acq.note = { tone: 'warn', text: `${res.law.name} is not a Sine law; it was imported for its own test type.` };
		return true;
	}

	/** Start the draft from the service's configuration (the one the run holds). */
	adoptServed(): boolean {
		const c = this.served?.config;
		if (!c || this.locked) return false;
		this.setup = fromSineConfig(c);
		return true;
	}

	// =========================================================================================
	// Profiles, limit channels

	/**
	 * Read profile rows pasted as text, or from a file on the computer that runs the test. The
	 * service reads them and says what it found; the draft takes the rows only when they read
	 * cleanly, keeping its bands.
	 */
	async importProfile(from: { text: string } | { path: string }): Promise<boolean> {
		if (this.locked) return false;
		const params = 'text' in from ? { text: from.text } : { path: from.path.trim() };
		const res = await this.act<SineProfileImportResult>('Read Profile', 'sine.profile.import', params);
		if (!res) return false;
		const path = 'path' in from ? from.path.trim() : null;
		this.imported = { path, errors: res.errors, warnings: res.warnings, rows: res.profile?.rows.length ?? 0 };
		if (!res.profile) return false;
		const p = this.setup.profile;
		const read = profileDraftOf({ ...res.profile, alarm_db: p.alarm_db, abort_db: p.abort_db }, p);
		this.setup.profile = { ...p, rows: read.rows, label: res.profile.label || p.label };
		return true;
	}

	/** A limit channel on the first enabled input that is not a control row and not limited yet. */
	addLimitChannel(): boolean {
		if (this.locked) return false;
		const used = new Set(this.setup.limits.channels.map((c) => c.channel));
		const row = this.rows.find((r) => r.enabled && r.role !== 'drive' && r.role !== 'control' && !used.has(r.index));
		this.setup.limits.channels.push(newLimitChannel(row, this.band));
		return true;
	}

	// =========================================================================================
	// The profile check and the dry run

	scheduleValidate(delay = this.checkDelayMs) {
		if (this.validateTimer) clearTimeout(this.validateTimer);
		this.validateTimer = setTimeout(() => void this.refreshValidate(), delay);
	}

	/** The service reads the profile: its findings, crossovers, kinematics and the three views. */
	async refreshValidate(): Promise<SineProfileValidateResult | null> {
		if (this.validateTimer) clearTimeout(this.validateTimer);
		this.validateTimer = null;
		const params = this.validateInput;
		if (!this.connected || !params) return null;
		const key = this.validateInputKey;
		const seq = ++this.validateSeq;
		this.validating = true;
		try {
			const res = await this.control.request<SineProfileValidateResult>('sine.profile.validate', params as unknown as Record<string, unknown>);
			if (seq === this.validateSeq) {
				this.validation = res;
				this.validationKey = key;
				this.validationError = null;
			}
			return res;
		} catch (err) {
			if (seq === this.validateSeq) {
				this.validation = null;
				this.validationKey = key;
				this.validationError = toRefusal('Read the profile', err).message;
			}
			return null;
		} finally {
			if (seq === this.validateSeq) this.validating = false;
		}
	}

	scheduleCheck(delay = this.checkDelayMs) {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	/** The service's verdict on the draft (`dry_run`: needs no control, replaces nothing). */
	async checkSetup(): Promise<SineDryRunResult | null> {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = null;
		if (!this.connected || errorCount(this.local) > 0) return null;
		const config = this.wire;
		const key = this.key;
		const seq = ++this.checkSeq;
		this.checking = true;
		try {
			const res = await this.control.request<SineDryRunResult>('sine.configure', { config, dry_run: true });
			if (seq === this.checkSeq) {
				this.check = res;
				this.checkedKey = key;
			}
			return res;
		} catch (err) {
			// A configuration whose shape is wrong is refused by field; show it as a finding there.
			const r = toRefusal('Check the setup', err);
			if (seq === this.checkSeq && r.reason === 'bad_params') {
				const field = typeof r.detail?.param === 'string' ? r.detail.param : 'title';
				this.check = { errors: [{ reason: 'out_of_range', field, message: r.message }], warnings: [], derived: null, views: [] };
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

	/** Send the setup. A table with edits is applied first, since the run uses its rows. */
	async apply(): Promise<boolean> {
		if (!this.actions.apply.enabled) return false;
		if (this.acq.tableDirty) {
			const ok = await this.acq.applyTable();
			if (!ok) return false;
		}
		const config = this.wire;
		const key = this.key;
		const res = await this.act<SineConfigureResult>('Apply Setup', 'sine.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				const d = r.detail as unknown as { errors?: SineDryRunResult['errors']; warnings?: SineDryRunResult['warnings'] };
				this.check = { errors: d.errors ?? [], warnings: d.warnings ?? [], derived: null, views: [] };
				this.checkedKey = key;
			}
			return false;
		}
		this.status = res.state;
		this.live = null;
		this.setHeld({ runId: res.state.run_id, key });
		this.served = { run_id: res.state.run_id, config, derived: res.derived, views: [] };
		void this.loadLog(res.state.run_id);
		void this.readServed();
		this.acq.note = { tone: 'pass', text: 'The service holds this setup.' };
		return true;
	}

	/** Arm: the station opens and gates; nothing is driven. With words, past a feasibility block. */
	async arm(ack?: string): Promise<boolean> {
		const res = await this.act<SineState>(ack ? 'Arm Past Feasibility' : 'Arm', 'sine.arm', ack ? { override_ack: ack } : {});
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** Identification first, then the review hold or the tone engaged at the first level. */
	async start(): Promise<boolean> {
		const res = await this.act<SineState>('Start', 'sine.start');
		if (res) {
			this.onStatus(res);
			this.step = 'run';
		}
		return res !== null;
	}

	/** Build the law from the identification and engage. */
	async proceed(): Promise<boolean> {
		const res = await this.act<SineState>('Proceed', 'sine.proceed');
		if (res) this.onStatus(res);
		return res !== null;
	}

	async setMode(mode: LadderMode): Promise<boolean> {
		const res = await this.act<SineModeResult>('Ladder Mode', 'sine.mode', { mode });
		return res !== null;
	}

	/** Step the level, manual ladder only. */
	async level(deltaDb: number): Promise<number | null> {
		const res = await this.act<SineLevelResult>('Level', 'sine.level', { delta_db: deltaDb });
		return res?.level_db ?? null;
	}

	/** Go to a level of the schedule. */
	async goTo(db: number): Promise<number | null> {
		const res = await this.act<SineLevelResult>('Go to Level', 'sine.level', { to_db: db });
		return res?.level_db ?? null;
	}

	/** Freeze the test's progress: the tone stays at its frequency and level, still under control. */
	async hold(): Promise<boolean> {
		const res = await this.act<SineState>('Hold', 'sine.hold');
		if (res) this.onStatus(res);
		return res !== null;
	}

	async resume(): Promise<boolean> {
		const res = await this.act<SineState>('Resume', 'sine.resume');
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** Turn the sweep at the next tick. */
	async reverse(): Promise<boolean> {
		const res = await this.act<SineState>('Reverse', 'sine.reverse');
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** Move the frequency by `deltaHz`, stopping at the band's edge; the frequency applied. */
	async jog(deltaHz: number): Promise<number | null> {
		const res = await this.act<SineFrequencyResult>('Jog', 'sine.jog', { delta_hz: deltaHz });
		return res?.frequency_hz ?? null;
	}

	/** Move the frequency to `hz`, inside the band; the frequency applied. */
	async gotoHz(hz: number): Promise<number | null> {
		const res = await this.act<SineFrequencyResult>('Go to Frequency', 'sine.goto', { hz });
		return res?.frequency_hz ?? null;
	}

	/** Change a live parameter of the run's law; it applies from the next tick. */
	async adjust(key: string, value: number): Promise<SineAdjustResult | null> {
		return this.act<SineAdjustResult>('Change Parameter', 'sine.adjust', { key, value });
	}

	async stop(): Promise<boolean> {
		const res = await this.act<SineState>('Stop', 'sine.stop', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async abort(): Promise<boolean> {
		const res = await this.act<SineState>('Abort', 'sine.abort', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** A new run from the same configuration. */
	async reset(): Promise<boolean> {
		const res = await this.act<SineState>('New Run', 'sine.reset');
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
