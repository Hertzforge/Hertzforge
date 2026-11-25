/**
 * MIMO Sine's working state, against the control service's `mimo_sine.*` and `laws.*` methods
 * (schema/PROTOCOL.md, "MIMO Sine", "Control Laws").
 *
 * The station, the channel table and the controller seat are the service's and shared by every
 * test type: they stay in the Acquire store, and this one reads them from there. What is MIMO
 * Sine's own is here: the setup draft (kept across reloads), the dry run of the setup, the law
 * catalog of the `sine_mimo` world, the run's state, its live numbers, its last pass and its
 * report, and the verbs. The service holds the truth and computes every number; this store keeps
 * the operator's edits beside it and sends them when told to.
 *
 * The law panel lists whatever the catalog holds for the world: no law is named here. A law whose
 * entry does not accept the test type, the shape or an amplitude-only reference is listed apart
 * with the reason, and cannot be chosen; a draft on such a law takes the first law that accepts
 * the setup.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelRow, LadderMode, LogEntry, RunRecordRef, View } from '$lib/protocol/types';
import type { LawEntry, LawsListResult } from '$lib/protocol/mimo';
import {
	MIMO_SINE_TOPICS,
	MIMO_SINE_WORLD,
	type MimoSineConfig,
	type MimoSineConfigResult,
	type MimoSineConfigureResult,
	type MimoSineDryRunResult,
	type MimoSineLiveHead,
	type MimoSineLogEvent,
	type MimoSineLogResult,
	type MimoSineReport,
	type MimoSineReportItem,
	type MimoSineResonance,
	type MimoSineState,
	type MimoSineSweepEvent
} from '$lib/protocol/mimoSine';
import type { ActionId, ActionState, RunHead, SetupStatus } from '../actions';
import { lawParamErrors, lawValues } from '../law/lawForm';
import { canonical } from '../mimo/mimoSetup';
import { regresses } from '../runOrder';
import { errorCount, mergeErrors, type FieldErrors } from '../setup';
import type { AcquireState } from '../store.svelte';
import { toRefusal, type Refusal } from '../words';
import {
	bandOf,
	defaultMsineSetup,
	fitChannels,
	fromMsineConfig,
	lawRefusal,
	localErrors,
	mergeMsineSetup,
	msineByKey,
	msineSectionOf,
	MSINE_SECTIONS,
	newLimit,
	toMsineConfig,
	type MsineDraft,
	type MsineSection
} from './msineSetup';
import { msineBarActions, msineIsLive, msineModuleVerbs, msinePrimary, type MsineVerbContext, type MsineVerbId } from './msineVerbs';
import { MSINE_OUTCOME_WORDS, MSINE_PHASE_WORDS, MSINE_STATE_WORDS } from './msineWords';

const SETUP_KEY = 'hertzforge.acquire.mimo_sine.setup.v1';
const HELD_KEY = 'hertzforge.acquire.mimo_sine.held.v1';
/** Most log entries kept on screen; the service keeps the full log with the run. */
const LOG_LIMIT = 500;

function storageOf(): Storage | null {
	try {
		return typeof window === 'undefined' ? null : window.localStorage;
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

export function saveMsineSetup(d: MsineDraft): string {
	return JSON.stringify(d, nanOut);
}

export function loadMsineSetup(raw: string | null): MsineDraft {
	if (!raw) return defaultMsineSetup();
	try {
		return mergeMsineSetup(JSON.parse(raw, nanIn) as Partial<MsineDraft>);
	} catch {
		return defaultMsineSetup();
	}
}

export interface MsineOptions {
	local?: Storage | null;
	checkDelayMs?: number;
	/** A display window observes: it keeps no draft and checks nothing. */
	drafts?: boolean;
}

/** A law of the catalog the setup cannot run, and why in plain words. */
export interface RefusedLaw {
	law: LawEntry;
	why: string;
}

export class MimoSineStore {
	readonly control: ServiceClient;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;

	setup = $state<MsineDraft>(defaultMsineSetup());
	section = $state<MsineSection>('channels');
	step = $state<'setup' | 'run'>('setup');
	/** The operator's words for proceeding past the review, kept in the log. */
	acknowledgement = $state('');

	status = $state.raw<MimoSineState | null>(null);
	/** The run's numbers from the last frame: `mimo_sine.live`, or this screen's own `mimo_sine.data`. */
	live = $state.raw<MimoSineLiveHead | null>(null);
	served = $state.raw<MimoSineConfigResult | null>(null);
	check = $state.raw<MimoSineDryRunResult | null>(null);
	checkedKey = $state('');
	checking = $state(false);
	/** Every law of the `sine_mimo` world the catalog lists, built in first. */
	laws = $state.raw<LawEntry[] | null>(null);
	lawProblems = $state.raw<{ file: string; reason: string }[]>([]);
	log = $state.raw<LogEntry[]>([]);
	/** The last pass's results against frequency (`mimo_sine.sweep`). */
	lastPass = $state.raw<MimoSineSweepEvent | null>(null);
	report = $state.raw<MimoSineReport | null>(null);
	held = $state.raw<{ runId: string | null; key: string } | null>(null);

	busy = $state<string | null>(null);
	#refusal = $state.raw<Refusal | null>(null);
	syncError = $state<string | null>(null);

	private liveNewer = $state(false);
	private checkSeq = 0;
	private logRun: string | null = null;
	private checkTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(
		readonly acq: AcquireState,
		options: MsineOptions = {}
	) {
		this.control = acq.control;
		this.localStore = options.local === undefined ? storageOf() : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 300;
		this.drafts = options.drafts ?? true;
		this.setup = loadMsineSetup(read(this.localStore, SETUP_KEY));
		const held = read(this.localStore, HELD_KEY);
		if (held) {
			try {
				this.held = JSON.parse(held);
			} catch {
				this.held = null;
			}
		}
		this.control.on(MIMO_SINE_TOPICS.state, (d) => this.onStatus(d as unknown as MimoSineState));
		const onFrame = (d: Record<string, unknown>) => {
			const l = d as unknown as MimoSineLiveHead;
			if (this.status?.run_id && l.run_id !== this.status.run_id) return;
			this.live = l;
			this.liveNewer = true;
		};
		this.control.on(MIMO_SINE_TOPICS.live, onFrame);
		this.control.on(MIMO_SINE_TOPICS.data, onFrame);
		this.control.on(MIMO_SINE_TOPICS.sweep, (d) => {
			const e = d as unknown as MimoSineSweepEvent;
			if (this.status?.run_id && e.run_id !== this.status.run_id) return;
			this.lastPass = e;
		});
		this.control.on(MIMO_SINE_TOPICS.report, (d) => {
			const r = (d as { report?: MimoSineReport | null }).report ?? null;
			if (r && (!this.status?.run_id || r.run_id === this.status.run_id)) this.report = r;
		});
		this.control.on(MIMO_SINE_TOPICS.log, (d) => {
			const e = d as unknown as MimoSineLogEvent;
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
		return msineIsLive(this.status);
	}

	get locked(): boolean {
		return this.isLive || this.acq.isLive;
	}

	/** What the laws are judged against: the test type, the references and the counts. */
	private get lawContext() {
		return { kind: this.setup.test.kind, phase_specified: this.setup.phase_specified, controls: this.setup.controls.length, drives: this.setup.drives.length };
	}

	/** The laws that accept this setup, in the catalog's order. */
	get offeredLaws(): LawEntry[] {
		const ctx = this.lawContext;
		return (this.laws ?? []).filter((l) => lawRefusal(l, ctx) === null);
	}

	/** The laws that do not, each with why. */
	get refusedLaws(): RefusedLaw[] {
		const ctx = this.lawContext;
		return (this.laws ?? []).flatMap((law) => {
			const why = lawRefusal(law, ctx);
			return why ? [{ law, why }] : [];
		});
	}

	get law(): LawEntry | null {
		return this.offeredLaws.find((l) => l.id === this.setup.law.id) ?? null;
	}

	readonly wire = $derived<MimoSineConfig>(toMsineConfig(this.setup, this.law));
	/**
	 * The simulator's structure the station names (Configure), null on NI or when it is sized to the
	 * table. The service plays it and checks it at configure and arm, so it is part of what is applied.
	 */
	get stationModel(): string | null {
		const sel = this.acq.selected;
		return sel?.kind === 'sim' ? (sel.simulator?.sine_structure ?? null) : null;
	}

	/** The setup as applied: the wire's configuration and the station's structure; another structure is a setup to apply again. */
	readonly key = $derived(canonical({ config: this.wire, model: this.stationModel }));
	readonly local = $derived.by((): FieldErrors => mergeErrors(localErrors(this.setup), lawParamErrors(this.law, this.setup.law.params)));

	get fieldErrors(): FieldErrors {
		const fromCheck = this.check && this.checkedKey === this.key ? msineByKey(this.check.errors, this.setup) : {};
		let lawMissing: FieldErrors = {};
		if (this.laws && !this.law) {
			const refused = this.refusedLaws.find((r) => r.law.id === this.setup.law.id);
			lawMissing = { 'law.id': [refused ? refused.why : this.offeredLaws.length ? 'Choose a control law.' : 'The catalog holds no control law that accepts this setup.'] };
		}
		return mergeErrors(mergeErrors(this.local, fromCheck), lawMissing);
	}

	get fieldWarnings(): FieldErrors {
		return this.check && this.checkedKey === this.key ? msineByKey(this.check.warnings, this.setup) : {};
	}

	get counts(): Record<MsineSection, { errors: number; warnings: number }> {
		const out = Object.fromEntries(MSINE_SECTIONS.map((s) => [s.id, { errors: 0, warnings: 0 }])) as Record<MsineSection, { errors: number; warnings: number }>;
		for (const [k, v] of Object.entries(this.fieldErrors)) out[msineSectionOf(k)].errors += v.length;
		for (const [k, v] of Object.entries(this.fieldWarnings)) out[msineSectionOf(k)].warnings += v.length;
		out.channels.errors += this.acq.tableErrorCount;
		return out;
	}

	get setupErrorCount(): number {
		return Object.values(this.counts).reduce((n, c) => n + c.errors, 0);
	}

	/** The service's refusals and warnings of the draft as it stands, by name, for the review before arming. */
	get findings(): { errors: MimoSineReportItem[]; warnings: MimoSineReportItem[] } {
		if (!this.check || this.checkedKey !== this.key) return { errors: [], warnings: [] };
		return { errors: this.check.errors, warnings: this.check.warnings };
	}

	get setupStatus(): SetupStatus {
		const s = this.status;
		if (!s || s.state === 'unconfigured') return 'none';
		const held = this.held;
		if (!held || held.runId !== s.run_id) return 'unread';
		return held.key === this.key ? 'applied' : 'changed';
	}

	get pending(): string | null {
		if (!this.laws) return 'The law catalog has not been read yet.';
		return null;
	}

	get runConfig(): MimoSineConfig | null {
		return this.served?.config ?? null;
	}

	/** The band of the test the draft describes. */
	get band(): [number, number] {
		return bandOf(this.setup);
	}

	/**
	 * The latest numbers: the state, with this run's live frame laid over it when the frame came
	 * after it. A state that came after the frame is the run's word until the next frame.
	 */
	get now(): MimoSineState | null {
		const s = this.status;
		const l = this.live;
		if (!s || !l || l.run_id !== s.run_id || !this.liveNewer) return s;
		return {
			...s,
			state: l.state,
			phase: l.phase,
			mode: l.mode,
			level_db: l.commanded_db ?? s.level_db,
			frequency_hz: l.frequency_hz,
			direction: l.direction,
			sweeps: l.sweeps,
			steps: l.steps,
			held: l.held,
			request_held: l.request_held,
			elapsed_s: l.elapsed_s ?? s.elapsed_s,
			alarm: l.alarm,
			notch: l.notch,
			notch_scale: l.notch_scale,
			law: s.law && l.law ? { ...s.law, misses: l.law.misses, capped_ticks: l.law.capped_ticks } : s.law
		};
	}

	/** The last frame's numbers, when they are this run's. */
	get frameOfRun(): MimoSineLiveHead | null {
		const s = this.status;
		const l = this.live;
		return s && l && l.run_id === s.run_id ? l : null;
	}

	/** The resonances the sweep passes too fast for, from the review. */
	get flaggedResonances(): MimoSineResonance[] {
		return this.status?.review?.resonances.filter((r) => r.flagged) ?? [];
	}

	private get verbContext(): MsineVerbContext {
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

	get actions(): Record<ActionId, ActionState> {
		return msineBarActions(this.verbContext);
	}

	get primary(): ActionId | null {
		return msinePrimary(this.verbContext);
	}

	get verbs(): Record<MsineVerbId, ActionState> {
		return msineModuleVerbs(this.verbContext);
	}

	/** The run's law: the one the service runs, else the draft's choice. */
	get runLaw(): LawEntry | null {
		const id = this.status?.law?.id ?? this.served?.derived?.law.id ?? null;
		return (id ? this.laws?.find((l) => l.id === id) : null) ?? this.law;
	}

	/** The run's controls, by table index. */
	get runControls(): number[] {
		return this.runConfig?.channels.controls.map((c) => c.channel) ?? this.setup.controls.map((c) => c.channel);
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
			end: s.end as unknown as RunHead['end'],
			record: s.record ?? null
		};
	}

	get frame(): { level_db: number | null; at_level_s: number | null; remaining_at_level_s: number | null } | null {
		const l = this.frameOfRun;
		if (!l || this.status?.state === 'finished') return null;
		return { level_db: l.level_db, at_level_s: this.status?.at_level_s ?? null, remaining_at_level_s: null };
	}

	get record(): RunRecordRef | null {
		return this.status?.record ?? null;
	}

	get recordChannels(): number {
		return this.rows.filter((r) => r.enabled).length;
	}

	get recordRateHz(): number {
		return this.setup.acquisition.fs_hz ?? 16384;
	}

	readonly outcomeWords: Record<string, string> = MSINE_OUTCOME_WORDS;
	readonly phaseWords: Record<string, string> = MSINE_PHASE_WORDS;
	readonly stateWords: Record<string, string> = MSINE_STATE_WORDS;

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

	/** Keep the draft across reloads, fit it to the table and the laws, and re-check it as it changes. */
	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				const text = saveMsineSetup($state.snapshot(this.setup) as MsineDraft);
				untrack(() => write(this.localStore, SETUP_KEY, text));
			});
			$effect(() => {
				const rows = this.acq.rows;
				void rows.map((r) => [r.enabled, r.role, r.index]);
				untrack(() => {
					if (this.drafts && !this.locked) fitChannels(this.setup, rows);
				});
			});
			$effect(() => {
				void this.setup.test.kind;
				void this.setup.phase_specified;
				void this.setup.controls.length;
				void this.setup.drives.length;
				void this.laws;
				untrack(() => this.fitLaw());
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
			const [status, served, laws] = await Promise.all([
				c.request<MimoSineState>('mimo_sine.state'),
				c.request<MimoSineConfigResult>('mimo_sine.config'),
				c.request<LawsListResult>('laws.list', { world: MIMO_SINE_WORLD })
			]);
			this.syncError = null;
			this.served = served;
			this.onStatus(status);
			this.setLaws(laws);
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		if (!this.drafts) return;
		fitChannels(this.setup, this.acq.rows);
		this.scheduleCheck(0);
	}

	private onStatus(s: MimoSineState) {
		const was = this.status;
		if (regresses(was, s)) return;
		this.status = s;
		this.liveNewer = false;
		if (this.live && this.live.run_id === s.run_id && (s.state === 'unconfigured' || s.state === 'configured' || s.state === 'armed' || s.state === 'blocked')) this.live = null;
		if (s.run_id !== this.logRun) void this.loadLog(s.run_id);
		if (was?.run_id !== s.run_id && s.run_id) void this.readServed();
		if (s.state !== 'review') this.acknowledgement = '';
		if (s.state === 'finished' && was?.state !== 'finished') void this.readReport();
		if (msineIsLive(s) && !msineIsLive(was) && this.step === 'setup' && s.state !== 'armed') this.step = 'run';
	}

	async loadLog(runId: string | null): Promise<void> {
		if (runId !== this.logRun) {
			this.log = [];
			this.lastPass = null;
			this.report = null;
		}
		this.logRun = runId;
		if (!runId) {
			this.live = null;
			return;
		}
		try {
			const res = await this.control.request<MimoSineLogResult>('mimo_sine.log', { since: 0 });
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
			this.served = await this.control.request<MimoSineConfigResult>('mimo_sine.config');
		} catch {
			/* kept as it was */
		}
	}

	private async readReport(): Promise<void> {
		try {
			const res = await this.control.request<{ report: MimoSineReport | null }>('mimo_sine.report');
			if (res.report) this.report = res.report;
		} catch {
			/* the report comes with its event */
		}
	}

	private setLaws(res: LawsListResult) {
		this.laws = res.laws.filter((l) => (l.world as string) === MIMO_SINE_WORLD);
		this.lawProblems = res.problems;
		this.fitLaw();
	}

	/** The draft's law stays while it accepts the setup; otherwise the first law that does. */
	private fitLaw() {
		if (!this.laws || this.locked) return;
		const law = this.law ?? this.offeredLaws[0] ?? null;
		if (!law) return;
		const same = law.id === this.setup.law.id;
		const params = lawValues(law, same ? this.setup.law.params : undefined, this.setup.controls.length || 1);
		if (same && canonical(params) === canonical(this.setup.law.params)) return;
		this.setup.law = { id: law.id, params };
	}

	/** Choose a law; one the setup cannot run is not taken. */
	chooseLaw(id: string): boolean {
		const law = this.offeredLaws.find((l) => l.id === id);
		if (!law || this.locked) return false;
		this.setup.law = { id, params: lawValues(law, id === this.setup.law.id ? this.setup.law.params : undefined, this.setup.controls.length || 1) };
		return true;
	}

	async refreshLaws(): Promise<void> {
		try {
			this.setLaws(await this.control.request<LawsListResult>('laws.list', { world: MIMO_SINE_WORLD }));
		} catch (err) {
			this.refusal = toRefusal('Read the control laws', err);
		}
	}

	async importLaw(path: string): Promise<boolean> {
		const res = await this.act<{ law: LawEntry }>('Import Law', 'laws.import', { path: path.trim() });
		if (!res) return false;
		await this.refreshLaws();
		if ((res.law.world as string) !== MIMO_SINE_WORLD) this.acq.note = { tone: 'warn', text: `${res.law.name} is not a MIMO Sine law; it was imported for its own test type.` };
		else if (this.chooseLaw(res.law.id)) this.acq.note = { tone: 'pass', text: `Imported ${res.law.name} ${res.law.version}.` };
		else this.acq.note = { tone: 'warn', text: `Imported ${res.law.name}; ${lawRefusal(res.law, this.lawContext) ?? 'it does not accept this setup.'}` };
		return true;
	}

	adoptServed(): boolean {
		const c = this.served?.config;
		if (!c || this.locked) return false;
		this.setup = fromMsineConfig(c);
		fitChannels(this.setup, this.acq.rows);
		return true;
	}

	/** A limit channel on the first enabled input that is not a control and not limited yet. */
	addLimit(channel?: number): boolean {
		if (this.locked) return false;
		const controls = new Set(this.setup.controls.map((c) => c.channel));
		const used = new Set(this.setup.limits.channels.map((c) => c.channel));
		const row = channel !== undefined ? this.rows.find((r) => r.index === channel) : this.rows.find((r) => r.enabled && r.role !== 'drive' && !controls.has(r.index) && !used.has(r.index));
		if (!row || used.has(row.index) || controls.has(row.index)) return false;
		const band = this.band.every(Number.isFinite) ? this.band : ([20, 2000] as [number, number]);
		this.setup.limits.channels.push(newLimit(row.index, band));
		return true;
	}

	removeLimit(channel: number) {
		if (this.locked) return;
		this.setup.limits.channels = this.setup.limits.channels.filter((c) => c.channel !== channel);
	}

	// =========================================================================================
	// The dry run

	scheduleCheck(delay = this.checkDelayMs) {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	async checkSetup(): Promise<MimoSineDryRunResult | null> {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = null;
		if (!this.connected || errorCount(this.local) > 0) return null;
		const config = this.wire;
		const key = this.key;
		const seq = ++this.checkSeq;
		this.checking = true;
		try {
			const res = await this.control.request<MimoSineDryRunResult>('mimo_sine.configure', { config, dry_run: true });
			if (seq === this.checkSeq) {
				this.check = res;
				this.checkedKey = key;
			}
			return res;
		} catch (err) {
			const r = toRefusal('Check the setup', err);
			if (seq === this.checkSeq && (r.reason === 'bad_params' || r.reason === 'config_invalid')) {
				const d = (r.detail ?? {}) as { errors?: MimoSineDryRunResult['errors']; warnings?: MimoSineDryRunResult['warnings']; param?: string };
				const errors = d.errors ?? [{ reason: 'out_of_range' as const, field: typeof d.param === 'string' ? d.param : 'title', message: r.message }];
				this.check = { errors, warnings: d.warnings ?? [], derived: null, views: [] };
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
		const res = await this.act<MimoSineConfigureResult>('Apply Setup', 'mimo_sine.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				const d = r.detail as unknown as { errors?: MimoSineDryRunResult['errors']; warnings?: MimoSineDryRunResult['warnings'] };
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
		const res = await this.act<MimoSineState>(ack ? 'Arm Past Feasibility' : 'Arm', 'mimo_sine.arm', ack ? { override_ack: ack } : {});
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** Identification first, then the review hold or the tone engaged at the first level. */
	async start(): Promise<boolean> {
		const res = await this.act<MimoSineState>('Start', 'mimo_sine.start');
		if (res) {
			this.onStatus(res);
			this.step = 'run';
		}
		return res !== null;
	}

	/** Build the law from the identification and engage; the operator's words go in the log. */
	async proceed(): Promise<boolean> {
		const ack = this.acknowledgement.trim();
		const res = await this.act<MimoSineState>('Proceed', 'mimo_sine.proceed', ack ? { acknowledge: ack } : {});
		if (res) this.onStatus(res);
		return res !== null;
	}

	async setMode(mode: LadderMode): Promise<boolean> {
		return (await this.act<{ mode: LadderMode }>('Ladder Mode', 'mimo_sine.mode', { mode })) !== null;
	}

	async level(deltaDb: number): Promise<number | null> {
		return (await this.act<{ level_db: number }>('Level', 'mimo_sine.level', { delta_db: deltaDb }))?.level_db ?? null;
	}

	async goTo(db: number): Promise<number | null> {
		return (await this.act<{ level_db: number }>('Go to Level', 'mimo_sine.level', { to_db: db }))?.level_db ?? null;
	}

	async hold(): Promise<boolean> {
		const res = await this.act<MimoSineState>('Hold', 'mimo_sine.hold');
		if (res) this.onStatus(res);
		return res !== null;
	}

	async resume(): Promise<boolean> {
		const res = await this.act<MimoSineState>('Resume', 'mimo_sine.resume');
		if (res) this.onStatus(res);
		return res !== null;
	}

	async jog(deltaHz: number): Promise<number | null> {
		return (await this.act<{ frequency_hz: number }>('Jog', 'mimo_sine.jog', { delta_hz: deltaHz }))?.frequency_hz ?? null;
	}

	async gotoHz(hz: number): Promise<number | null> {
		return (await this.act<{ frequency_hz: number }>('Go to Frequency', 'mimo_sine.goto', { hz }))?.frequency_hz ?? null;
	}

	async stop(): Promise<boolean> {
		const res = await this.act<MimoSineState>('Stop', 'mimo_sine.stop', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async abort(): Promise<boolean> {
		const res = await this.act<MimoSineState>('Abort', 'mimo_sine.abort', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async reset(): Promise<boolean> {
		// The new run holds the same configuration: the draft is still the service's only if it was before.
		const held = this.setupStatus === 'applied' ? this.held : null;
		const res = await this.act<MimoSineState>('New Run', 'mimo_sine.reset');
		if (!res) return false;
		this.onStatus(res);
		this.live = null;
		if (held) this.setHeld({ runId: res.run_id, key: held.key });
		return true;
	}

	/** The last pass's views, for a plot before this window's own frame has come. */
	get lastViews(): View[] {
		return this.lastPass?.views ?? [];
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
