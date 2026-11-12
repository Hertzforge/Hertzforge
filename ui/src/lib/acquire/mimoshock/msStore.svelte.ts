/**
 * MIMO Shock's working state, against the control service's `mimo_shock.*` and `laws.*` methods
 * (schema/PROTOCOL.md, "MIMO Shock", "Control Laws").
 *
 * The station, the channel table and the controller seat are the service's and shared by every
 * test type: they stay in the Acquire store, and this one reads them from there. What is MIMO
 * Shock's own is here: the setup draft (kept across reloads), the dry run of the setup, the law
 * catalog of the `shock_mimo` world, the run's state and its pulse rows, the operator's
 * acknowledgement of the unreachable hold, and the verbs. The service holds the truth and computes
 * every number; this store keeps the operator's edits beside it and sends them when told to.
 *
 * The law panel lists whatever the catalog holds for the world and the target kind: no law is
 * named here. A new setup opens on the law the catalog names the default for its target kind; a
 * draft whose law does not accept the kind takes that default.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelRow, LogEntry, View } from '$lib/protocol/types';
import type { LawsListResult } from '$lib/protocol/mimo';
import {
	MIMO_SHOCK_TOPICS,
	MIMO_SHOCK_WORLD,
	type MimoShockConfig,
	type MimoShockConfigResult,
	type MimoShockConfigureResult,
	type MimoShockDryRunResult,
	type MimoShockLawEntry,
	type MimoShockLive,
	type MimoShockLogEvent,
	type MimoShockLogResult,
	type MimoShockPulseEvent,
	type MimoShockPulseRow,
	type MimoShockPulsesResult,
	type MimoShockState,
	type MimoShockTargetKind
} from '$lib/protocol/mimoShock';
import type { ActionId, ActionState, RunHead, SetupStatus } from '../actions';
import { lawParamErrors, lawValues } from '../law/lawForm';
import { canonical } from '../mimo/mimoSetup';
import { regresses } from '../runOrder';
import { errorCount, mergeErrors, type FieldErrors } from '../setup';
import type { AcquireState } from '../store.svelte';
import { toRefusal, type Refusal } from '../words';
import { defaultMsSetup, fitChannels, fromMsConfig, localErrors, mergeMsSetup, msByKey, msSectionOf, MS_SECTIONS, toMsConfig, type MsDraft, type MsSection } from './msSetup';
import { msBarActions, msFire, msIsLive, msPrimary, type MsVerbContext } from './msVerbs';
import { MS_OUTCOME_WORDS, MS_PHASE_WORDS, MS_STATE_WORDS, needsAcknowledgement } from './msWords';

const SETUP_KEY = 'hertzforge.acquire.mimo_shock.setup.v1';
const HELD_KEY = 'hertzforge.acquire.mimo_shock.held.v1';

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

export function saveMsSetup(d: MsDraft): string {
	return JSON.stringify(d, nanOut);
}

export function loadMsSetup(raw: string | null): MsDraft {
	if (!raw) return defaultMsSetup();
	try {
		return mergeMsSetup(JSON.parse(raw, nanIn) as Partial<MsDraft>);
	} catch {
		return defaultMsSetup();
	}
}

/** Whether a law accepts a target kind: by its entry's `targets`; an entry that names none takes any. */
export function lawAccepts(law: MimoShockLawEntry, kind: MimoShockTargetKind): boolean {
	return !law.targets || law.targets.includes(kind);
}

export interface MsOptions {
	local?: Storage | null;
	checkDelayMs?: number;
	/** A display window observes: it keeps no draft and checks nothing. */
	drafts?: boolean;
}

export class MimoShockStore {
	readonly control: ServiceClient;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;

	setup = $state<MsDraft>(defaultMsSetup());
	section = $state<MsSection>('channels');
	step = $state<'setup' | 'run'>('setup');
	/** The operator's own words for going on past the unreachable hold. */
	acknowledgement = $state('');

	status = $state.raw<MimoShockState | null>(null);
	live = $state.raw<MimoShockLive | null>(null);
	served = $state.raw<MimoShockConfigResult | null>(null);
	check = $state.raw<MimoShockDryRunResult | null>(null);
	checkedKey = $state('');
	checking = $state(false);
	/** Every law of the `shock_mimo` world the catalog lists, built in first. */
	laws = $state.raw<MimoShockLawEntry[] | null>(null);
	lawProblems = $state.raw<{ file: string; reason: string }[]>([]);
	pulses = $state.raw<MimoShockPulseRow[]>([]);
	lastViews = $state.raw<View[]>([]);
	log = $state.raw<LogEntry[]>([]);
	/** The pulse whose judgement the verdict detail shows, by row index; null: the latest. */
	picked = $state<number | null>(null);
	held = $state.raw<{ runId: string | null; key: string } | null>(null);

	busy = $state<string | null>(null);
	#refusal = $state.raw<Refusal | null>(null);
	syncError = $state<string | null>(null);

	private checkSeq = 0;
	private pulsesRun: string | null = null;
	private checkTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(
		readonly acq: AcquireState,
		options: MsOptions = {}
	) {
		this.control = acq.control;
		this.localStore = options.local === undefined ? storageOf() : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 300;
		this.drafts = options.drafts ?? true;
		this.setup = loadMsSetup(read(this.localStore, SETUP_KEY));
		const held = read(this.localStore, HELD_KEY);
		if (held) {
			try {
				this.held = JSON.parse(held);
			} catch {
				this.held = null;
			}
		}
		this.control.on(MIMO_SHOCK_TOPICS.state, (d) => this.onStatus(d as unknown as MimoShockState));
		this.control.on(MIMO_SHOCK_TOPICS.live, (d) => (this.live = d as unknown as MimoShockLive));
		this.control.on(MIMO_SHOCK_TOPICS.pulse, (d) => this.onPulse(d as unknown as MimoShockPulseEvent));
		this.control.on(MIMO_SHOCK_TOPICS.log, (d) => {
			const e = d as unknown as MimoShockLogEvent;
			if (this.status?.run_id && e.run_id !== this.status.run_id) return;
			const have = new Set(this.log.map((x) => x.seq));
			this.log = [...this.log, ...e.entries.filter((x) => !have.has(x.seq))];
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
		return msIsLive(this.status);
	}

	get locked(): boolean {
		return this.isLive || this.acq.isLive;
	}

	/** The laws offered for the draft's target kind, in the catalog's order. */
	get kindLaws(): MimoShockLawEntry[] {
		return (this.laws ?? []).filter((l) => lawAccepts(l, this.setup.kind));
	}

	get law(): MimoShockLawEntry | null {
		return this.kindLaws.find((l) => l.id === this.setup.law.id) ?? null;
	}

	readonly wire = $derived<MimoShockConfig>(toMsConfig(this.setup, this.law));
	readonly key = $derived(canonical(this.wire));
	readonly local = $derived.by((): FieldErrors => mergeErrors(localErrors(this.setup), lawParamErrors(this.law, this.setup.law.params)));

	get fieldErrors(): FieldErrors {
		const fromCheck = this.check && this.checkedKey === this.key ? msByKey(this.check.errors) : {};
		const lawMissing: FieldErrors =
			this.laws && !this.law ? { 'law.id': [this.kindLaws.length ? 'Choose a control law for this kind of target.' : 'The catalog holds no control law for this kind of target.'] } : {};
		return mergeErrors(mergeErrors(this.local, fromCheck), lawMissing);
	}

	get fieldWarnings(): FieldErrors {
		return this.check && this.checkedKey === this.key ? msByKey(this.check.warnings) : {};
	}

	get counts(): Record<MsSection, { errors: number; warnings: number }> {
		const out = Object.fromEntries(MS_SECTIONS.map((s) => [s.id, { errors: 0, warnings: 0 }])) as Record<MsSection, { errors: number; warnings: number }>;
		for (const [k, v] of Object.entries(this.fieldErrors)) out[msSectionOf(k)].errors += v.length;
		for (const [k, v] of Object.entries(this.fieldWarnings)) out[msSectionOf(k)].warnings += v.length;
		out.channels.errors += this.acq.tableErrorCount;
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
		if (!this.laws) return 'The law catalog has not been read yet.';
		return null;
	}

	get runConfig(): MimoShockConfig | null {
		return this.served?.config ?? null;
	}

	/** The latest numbers: the live frame's where it is for this run, else the state's. */
	get now(): MimoShockState | null {
		const s = this.status;
		const l = this.live;
		if (!s || !l || l.run_id !== s.run_id) return s;
		return { ...s, phase: l.phase, state: l.state, level_db: l.level_db, rung: l.rung, pulses: l.pulses, next_pulse_s: l.next_pulse_s };
	}

	private get verbContext(): MsVerbContext {
		return {
			connected: this.connected,
			controller: this.isController,
			run: this.now,
			setup: this.setupStatus,
			setupErrors: this.setupErrorCount,
			busy: this.busy,
			pending: this.pending,
			acknowledgement: this.acknowledgement
		};
	}

	get actions(): Record<ActionId, ActionState> {
		return msBarActions(this.verbContext);
	}

	get primary(): ActionId | null {
		return msPrimary(this.verbContext);
	}

	get fireAction(): ActionState {
		return msFire(this.verbContext);
	}

	/** The run holds before any pulse for the operator's acknowledgement of a floor above tolerance. */
	get mustAcknowledge(): boolean {
		return needsAcknowledgement(this.status);
	}

	/** Whether the run in hand stops on the scaled abort lines (its own word, else the setup's). */
	get scaledAborts(): boolean {
		const s = this.status;
		if (s && s.state !== 'unconfigured' && typeof s.scaled_aborts === 'boolean') return s.scaled_aborts;
		return this.runConfig?.control?.scaled_aborts ?? this.setup.control.scaled_aborts;
	}

	/** Why the scaled-abort switch cannot move now, or null: it is locked while a run is live. */
	get scaledToggleWhy(): string | null {
		return this.locked ? 'Locked while the run is live: the scaled abort lines cannot change once a run is armed or running.' : null;
	}

	/** Turn the scaled abort lines on or off in the setup; false while the switch is locked. */
	setScaledAborts(on: boolean): boolean {
		if (this.scaledToggleWhy) return false;
		this.setup.control.scaled_aborts = on;
		return true;
	}

	/** The standing notice while the run in hand has its scaled aborts off, or null. */
	get unscaledNotice(): string | null {
		const s = this.status;
		if (!s || s.state === 'unconfigured' || this.scaledAborts) return null;
		return s.notice ?? "Level-scaled aborts are off: a pulse over its level's abort line is flagged and judged failing, and only the full-level (absolute) lines end the run.";
	}

	/** The timing window of the run in hand, seconds. */
	get windowS(): number | null {
		return this.runConfig?.timing?.window_s ?? null;
	}

	/** The pulse the verdict detail shows: the one picked, else the latest. */
	get shownPulse(): MimoShockPulseRow | null {
		if (this.picked !== null) {
			const p = this.pulses.find((x) => x.index === this.picked);
			if (p) return p;
		}
		return this.pulses.at(-1) ?? this.status?.last_pulse ?? null;
	}

	/** The run's controls, by table index. */
	get runControls(): number[] {
		return this.runConfig?.channels.controls.map((c) => c.channel) ?? this.status?.review?.controls.map((c) => c.channel) ?? this.setup.controls;
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
			at_level_s: null,
			remaining_at_level_s: null,
			end: s.end as unknown as RunHead['end']
		};
	}

	readonly frame = null;
	readonly outcomeWords: Record<string, string> = MS_OUTCOME_WORDS;
	readonly phaseWords: Record<string, string> = MS_PHASE_WORDS;
	readonly stateWords: Record<string, string> = MS_STATE_WORDS;

	get elapsed(): number | null {
		return this.status?.elapsed_s ?? null;
	}

	get source(): { source: 'SIM' | 'HARDWARE'; cls: 'sim' | 'ni_simulated' | 'ni_real' } | null {
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

	/** The ladder alone sets the level: the shared bar's level and mode verbs are not offered. */
	setMode() {}
	level() {}
	goTo() {}

	dismissRefusal() {
		this.#refusal = null;
		this.acq.dismissRefusal();
	}

	dismissNote() {
		this.acq.dismissNote();
	}

	// =========================================================================================
	// Wiring

	/** Keep the draft across reloads, fit it to the table, and re-check it as it changes. */
	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				const text = saveMsSetup($state.snapshot(this.setup) as MsDraft);
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
				void this.setup.kind;
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
				c.request<MimoShockState>('mimo_shock.state'),
				c.request<MimoShockConfigResult>('mimo_shock.config'),
				c.request<LawsListResult>('laws.list', { world: MIMO_SHOCK_WORLD })
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

	private onStatus(s: MimoShockState) {
		const was = this.status;
		if (regresses(was, s)) return;
		this.status = s;
		if (this.live && this.live.run_id === s.run_id) this.live = null;
		if (s.run_id !== this.pulsesRun) void this.loadPulses(s.run_id);
		else if (s.last_pulse && !this.pulses.some((p) => p.index === s.last_pulse!.index)) this.pulses = [...this.pulses, s.last_pulse];
		if (was?.run_id !== s.run_id && s.run_id) void this.readServed();
		if (s.state !== 'review') this.acknowledgement = '';
		if (msIsLive(s) && !msIsLive(was) && this.step === 'setup' && s.state !== 'armed') this.step = 'run';
	}

	private onPulse(e: MimoShockPulseEvent) {
		if (this.status?.run_id && e.run_id !== this.status.run_id) return;
		if (!this.pulses.some((p) => p.index === e.pulse.index)) this.pulses = [...this.pulses, e.pulse].sort((a, b) => a.index - b.index);
		this.lastViews = e.views;
	}

	async loadPulses(runId: string | null): Promise<void> {
		// Another run's rows never stay: a pulse row is keyed by its index within its own run.
		if (runId !== this.pulsesRun) {
			this.pulses = [];
			this.log = [];
		}
		this.pulsesRun = runId;
		this.lastViews = [];
		this.picked = null;
		if (!runId) return;
		try {
			const [res, log] = await Promise.all([
				this.control.request<MimoShockPulsesResult>('mimo_shock.pulses', { since: 0 }),
				this.control.request<MimoShockLogResult>('mimo_shock.log', { since: 0 })
			]);
			if (this.pulsesRun !== runId) return;
			const have = new Map(this.pulses.map((p) => [p.index, p]));
			for (const p of res.pulses) have.set(p.index, p);
			this.pulses = [...have.values()].sort((a, b) => a.index - b.index);
			this.log = log.entries;
		} catch {
			/* the rows come with each pulse's event */
		}
	}

	private async readServed(): Promise<void> {
		try {
			this.served = await this.control.request<MimoShockConfigResult>('mimo_shock.config');
		} catch {
			/* kept as it was */
		}
	}

	private setLaws(res: LawsListResult) {
		this.laws = res.laws.filter((l) => (l.world as string) === MIMO_SHOCK_WORLD) as MimoShockLawEntry[];
		this.lawProblems = res.problems;
		this.fitLaw();
	}

	/** The law the catalog names the default for a target kind, when it lists one. */
	defaultLaw(kind: MimoShockTargetKind): MimoShockLawEntry | null {
		return this.laws?.find((l) => l.default_for?.includes(kind) && lawAccepts(l, kind)) ?? null;
	}

	/**
	 * The draft's law stays when it accepts the target kind; otherwise the service's default for the
	 * kind, else the first law that accepts it.
	 */
	private fitLaw() {
		if (!this.laws || this.locked) return;
		const law = this.law ?? this.defaultLaw(this.setup.kind) ?? this.kindLaws[0] ?? null;
		if (!law) return;
		const same = law.id === this.setup.law.id;
		const params = lawValues(law, same ? this.setup.law.params : undefined, this.setup.controls.length || 1);
		if (same && canonical(params) === canonical(this.setup.law.params)) return;
		this.setup.law = { id: law.id, params };
	}

	chooseLaw(id: string) {
		const law = this.kindLaws.find((l) => l.id === id);
		if (!law || this.locked) return;
		this.setup.law = { id, params: lawValues(law, id === this.setup.law.id ? this.setup.law.params : undefined, this.setup.controls.length || 1) };
	}

	/**
	 * Change the target kind. A draft on the old kind's default law moves to the new kind's default;
	 * a law the operator chose stays while it accepts the new kind, and otherwise gives way.
	 */
	setKind(kind: MimoShockTargetKind) {
		if (this.locked || kind === this.setup.kind) return;
		const wasDefault = this.setup.law.id !== '' && this.setup.law.id === this.defaultLaw(this.setup.kind)?.id;
		this.setup.kind = kind;
		const next = this.defaultLaw(kind);
		if (wasDefault && next && next.id !== this.setup.law.id) this.setup.law = { id: next.id, params: lawValues(next, undefined, this.setup.controls.length || 1) };
		if (kind === 'waveform' && this.setup.verdict === 'global') this.setup.verdict = 'all';
		this.fitLaw();
	}

	async refreshLaws(): Promise<void> {
		try {
			this.setLaws(await this.control.request<LawsListResult>('laws.list', { world: MIMO_SHOCK_WORLD }));
		} catch (err) {
			this.refusal = toRefusal('Read the control laws', err);
		}
	}

	async importLaw(path: string): Promise<boolean> {
		const res = await this.act<{ law: MimoShockLawEntry }>('Import Law', 'laws.import', { path: path.trim() });
		if (!res) return false;
		await this.refreshLaws();
		if ((res.law.world as string) === MIMO_SHOCK_WORLD && lawAccepts(res.law, this.setup.kind)) {
			this.chooseLaw(res.law.id);
			this.acq.note = { tone: 'pass', text: `Imported ${res.law.name} ${res.law.version}.` };
		} else if ((res.law.world as string) === MIMO_SHOCK_WORLD) this.acq.note = { tone: 'warn', text: `${res.law.name} does not accept this kind of target.` };
		else this.acq.note = { tone: 'warn', text: `${res.law.name} is not a MIMO Shock law; it was imported for its own test type.` };
		return true;
	}

	adoptServed(): boolean {
		const c = this.served?.config;
		if (!c || this.locked) return false;
		this.setup = fromMsConfig(c);
		fitChannels(this.setup, this.acq.rows);
		return true;
	}

	// =========================================================================================
	// The dry run

	scheduleCheck(delay = this.checkDelayMs) {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	async checkSetup(): Promise<MimoShockDryRunResult | null> {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = null;
		if (!this.connected || errorCount(this.local) > 0) return null;
		const config = this.wire;
		const key = this.key;
		const seq = ++this.checkSeq;
		this.checking = true;
		try {
			const res = await this.control.request<MimoShockDryRunResult>('mimo_shock.configure', { config, dry_run: true });
			if (seq === this.checkSeq) {
				this.check = res;
				this.checkedKey = key;
			}
			return res;
		} catch (err) {
			const r = toRefusal('Check the setup', err);
			if (seq === this.checkSeq && (r.reason === 'bad_params' || r.reason === 'config_invalid')) {
				const d = (r.detail ?? {}) as { errors?: MimoShockDryRunResult['errors']; warnings?: MimoShockDryRunResult['warnings']; param?: string };
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

	async apply(): Promise<boolean> {
		if (!this.actions.apply.enabled) return false;
		if (this.acq.tableDirty) {
			const ok = await this.acq.applyTable();
			if (!ok) return false;
		}
		const config = this.wire;
		const key = this.key;
		const res = await this.act<MimoShockConfigureResult>('Apply Setup', 'mimo_shock.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				const d = r.detail as unknown as { errors?: MimoShockDryRunResult['errors']; warnings?: MimoShockDryRunResult['warnings'] };
				this.check = { errors: d.errors ?? [], warnings: d.warnings ?? [], derived: null, views: [] };
				this.checkedKey = key;
			}
			return false;
		}
		this.status = res.state;
		this.setHeld({ runId: res.state.run_id, key });
		this.served = { run_id: res.state.run_id, config, derived: res.derived, views: [] };
		void this.loadPulses(res.state.run_id);
		void this.readServed();
		this.acq.note = { tone: 'pass', text: 'The service holds this setup.' };
		return true;
	}

	async arm(): Promise<boolean> {
		const res = await this.act<MimoShockState>('Arm', 'mimo_shock.arm');
		if (res) this.onStatus(res);
		return res !== null;
	}

	async start(): Promise<boolean> {
		const res = await this.act<MimoShockState>('Start', 'mimo_shock.start');
		if (res) {
			this.onStatus(res);
			this.step = 'run';
		}
		return res !== null;
	}

	/** Seed the law and go on; past the unreachable hold, with the operator's own words. */
	async proceed(): Promise<boolean> {
		const ack = this.acknowledgement.trim();
		const res = await this.act<MimoShockState>('Proceed', 'mimo_shock.proceed', this.mustAcknowledge && ack ? { acknowledge: ack } : {});
		if (res) this.onStatus(res);
		return res !== null;
	}

	async fire(): Promise<boolean> {
		const res = await this.act<MimoShockState>('Fire', 'mimo_shock.fire');
		if (res) this.onStatus(res);
		return res !== null;
	}

	async stop(): Promise<boolean> {
		const res = await this.act<MimoShockState>('Stop', 'mimo_shock.stop', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async abort(): Promise<boolean> {
		const res = await this.act<MimoShockState>('Abort', 'mimo_shock.abort', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async reset(): Promise<boolean> {
		const res = await this.act<MimoShockState>('New Run', 'mimo_shock.reset');
		if (!res) return false;
		this.onStatus(res);
		if (this.held && this.setupStatus !== 'changed') this.setHeld({ runId: res.run_id, key: this.key });
		return true;
	}

	/** Show one pulse's judgement in the verdict detail, or the latest again. */
	pick(index: number | null) {
		this.picked = index;
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
