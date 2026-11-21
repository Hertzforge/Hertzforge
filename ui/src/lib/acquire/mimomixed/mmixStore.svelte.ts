/**
 * MIMO Mixed Mode's working state, against the control service's `mimo_mixed.*` and `laws.*`
 * methods (`$lib/protocol/mimoMixed`).
 *
 * The station, the channel table and the controller seat are the Acquire store's; this store reads
 * them from there. What is MIMO Mixed Mode's own is here: the setup draft (kept across reloads),
 * the floor's reference as the service built or read it, the dry run of the setup, the two law
 * catalogs (world `mimo` for the floor, `sine_mimo` for the tones), the run's state, its live
 * numbers and its log, and the verbs. The service holds the truth and computes every number.
 */

import { untrack } from 'svelte';
import type { ServiceClient } from '$lib/protocol/client.svelte';
import type { ChannelRow, LadderMode, LogEntry, RunRecordRef } from '$lib/protocol/types';
import type { LawEntry, LawsListResult, MimoReferenceResult } from '$lib/protocol/mimo';
import {
	MIMO_MIXED_LAW_WORLDS,
	MIMO_MIXED_TOPICS,
	type MimoMixedConfig,
	type MimoMixedConfigResult,
	type MimoMixedConfigureResult,
	type MimoMixedDryRunResult,
	type MimoMixedLiveHead,
	type MimoMixedLogEvent,
	type MimoMixedLogResult,
	type MimoMixedOverrideReason,
	type MimoMixedRefusalReason,
	type MimoMixedReportItem,
	type MimoMixedState
} from '$lib/protocol/mimoMixed';
import type { ActionId, ActionState, RunHead, SetupStatus } from '../actions';
import { lawParamErrors, lawValues, type LawErrors } from '../law/lawForm';
import { canonical } from '../mimo/mimoSetup';
import { regresses } from '../runOrder';
import { errorCount, mergeErrors, type FieldErrors } from '../setup';
import type { AcquireState } from '../store.svelte';
import { toRefusal, type Refusal } from '../words';
import {
	breakpointParamsOf,
	defaultMmixSetup,
	fitChannels,
	freeId,
	localErrors,
	mergeMmixSetup,
	mmixByKey,
	mmixSectionOf,
	MMIX_SECTIONS,
	newBand,
	newTone,
	rowLabel,
	toMmixConfig,
	type MmixDraft,
	type MmixSection
} from './mmixSetup';
import { mmixBarActions, mmixIsLive, mmixModuleVerbs, mmixPartToggle, mmixPrimary, type MmixVerbContext, type MmixVerbId } from './mmixVerbs';
import { MMIX_OUTCOME_WORDS, MMIX_PHASE_WORDS, MMIX_REFUSAL_WORDS, MMIX_STATE_WORDS, mmixRefusalFacts } from './mmixWords';

const SETUP_KEY = 'hertzforge.acquire.mimo_mixed.setup.v1';
const HELD_KEY = 'hertzforge.acquire.mimo_mixed.held.v1';
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

export function saveMmixSetup(d: MmixDraft): string {
	return JSON.stringify(d, nanOut);
}

export function loadMmixSetup(raw: string | null): MmixDraft {
	if (!raw) return defaultMmixSetup();
	try {
		return mergeMmixSetup(JSON.parse(raw, nanIn) as Partial<MmixDraft>);
	} catch {
		return defaultMmixSetup();
	}
}

export interface MmixOptions {
	local?: Storage | null;
	checkDelayMs?: number;
	/** A display window observes: it keeps no draft and checks nothing. */
	drafts?: boolean;
}

export type MmixLawRole = 'floor' | 'tones';

function lawKeys(E: FieldErrors, role: MmixLawRole): LawErrors {
	const out: LawErrors = {};
	const prefix = `laws.${role}.`;
	for (const [k, v] of Object.entries(E)) if (k.startsWith(prefix)) out[`law.${k.slice(prefix.length)}`] = v;
	return out;
}

export class MimoMixedStore {
	readonly control: ServiceClient;
	private readonly localStore: Storage | null;
	private readonly checkDelayMs: number;
	private readonly drafts: boolean;

	setup = $state<MmixDraft>(defaultMmixSetup());
	section = $state<MmixSection>('channels');
	step = $state<'setup' | 'run'>('setup');
	/** The operator's words for proceeding past the review or a failed gate, kept in the log. */
	acknowledgement = $state('');

	status = $state.raw<MimoMixedState | null>(null);
	live = $state.raw<MimoMixedLiveHead | null>(null);
	served = $state.raw<MimoMixedConfigResult | null>(null);
	check = $state.raw<MimoMixedDryRunResult | null>(null);
	checkedKey = $state('');
	checking = $state(false);
	/** The floor's reference as the service built or read it, and the draft it was built from. */
	ref = $state.raw<MimoReferenceResult | null>(null);
	refKey = $state('');
	refBusy = $state(false);
	floorLaws = $state.raw<LawEntry[] | null>(null);
	toneLaws = $state.raw<LawEntry[] | null>(null);
	lawProblems = $state.raw<{ file: string; reason: string }[]>([]);
	log = $state.raw<LogEntry[]>([]);
	held = $state.raw<{ runId: string | null; key: string } | null>(null);

	busy = $state<string | null>(null);
	#refusal = $state.raw<Refusal | null>(null);
	syncError = $state<string | null>(null);

	private liveNewer = $state(false);
	private checkSeq = 0;
	private refSeq = 0;
	private logRun: string | null = null;
	private checkTimer: ReturnType<typeof setTimeout> | null = null;

	constructor(
		readonly acq: AcquireState,
		options: MmixOptions = {}
	) {
		this.control = acq.control;
		this.localStore = options.local === undefined ? storageOf() : options.local;
		this.checkDelayMs = options.checkDelayMs ?? 300;
		this.drafts = options.drafts ?? true;
		this.setup = loadMmixSetup(read(this.localStore, SETUP_KEY));
		const held = read(this.localStore, HELD_KEY);
		if (held) {
			try {
				this.held = JSON.parse(held);
			} catch {
				this.held = null;
			}
		}
		this.control.on(MIMO_MIXED_TOPICS.state, (d) => this.onStatus(d as unknown as MimoMixedState));
		const onFrame = (d: Record<string, unknown>) => {
			const l = d as unknown as MimoMixedLiveHead;
			if (this.status?.run_id && l.run_id !== this.status.run_id) return;
			this.live = l;
			this.liveNewer = true;
		};
		this.control.on(MIMO_MIXED_TOPICS.live, onFrame);
		this.control.on(MIMO_MIXED_TOPICS.data, onFrame);
		this.control.on(MIMO_MIXED_TOPICS.log, (d) => {
			const e = d as unknown as MimoMixedLogEvent;
			if (this.status?.run_id && e.run_id !== this.status.run_id) return;
			const have = new Set(this.log.map((x) => x.seq));
			this.log = [...this.log, ...(e.entries as LogEntry[]).filter((x) => !have.has(x.seq))].slice(-LOG_LIMIT);
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

	/** The channels a run's streams would hold: every enabled input and drive of the table being edited. */
	get recordChannels(): number {
		return this.rows.filter((r) => r.enabled).length;
	}

	/** The sample rate the run asks for, for the size its streams take. */
	get recordRateHz(): number {
		return this.setup.estimation.fs_hz;
	}

	get tableFiles() {
		return this.acq.tableFiles;
	}

	get isLive(): boolean {
		return mmixIsLive(this.status);
	}

	get locked(): boolean {
		return this.isLive || this.acq.isLive;
	}

	/** The controls' names, in order. */
	get controlLabels(): string[] {
		return this.setup.controls.map((c) => rowLabel(this.rows, c.channel));
	}

	get driveLabels(): string[] {
		return this.setup.drives.map((x) => rowLabel(this.rows, x.channel));
	}

	get floorLaw(): LawEntry | null {
		return this.floorLaws?.find((l) => l.id === this.setup.laws.floor.id) ?? null;
	}

	get toneLaw(): LawEntry | null {
		return this.toneLaws?.find((l) => l.id === this.setup.laws.tones.id) ?? null;
	}

	readonly refParamsKey = $derived(this.setup.reference.method === 'breakpoints' ? canonical(breakpointParamsOf(this.setup, this.controlLabels)) : `import:${this.setup.reference.importPath.trim()}`);

	/** The reference the configuration carries: the service's, while it is of the draft as it stands. */
	get reference() {
		return this.ref?.ok && this.refKey === this.refParamsKey ? this.ref.reference : null;
	}

	readonly wire = $derived<MimoMixedConfig>(toMmixConfig(this.setup, this.reference, { floor: this.floorLaw, tones: this.toneLaw }));
	readonly key = $derived(canonical(this.wire));
	readonly local = $derived.by((): FieldErrors => {
		const prefix = (role: MmixLawRole, e: LawErrors): FieldErrors => Object.fromEntries(Object.entries(e).map(([k, v]) => [`laws.${role}.${k.replace(/^law\./, '')}`, v]));
		return mergeErrors(mergeErrors(localErrors(this.setup), prefix('floor', lawParamErrors(this.floorLaw, this.setup.laws.floor.params))), prefix('tones', lawParamErrors(this.toneLaw, this.setup.laws.tones.params)));
	});

	/** The reference's own refusals, while it is the draft's. */
	get referenceErrors(): FieldErrors {
		if (!this.ref || this.refKey !== this.refParamsKey || this.ref.ok) return {};
		return { reference: this.ref.errors.map((e) => e.message) };
	}

	get fieldErrors(): FieldErrors {
		const fromCheck = this.check && this.checkedKey === this.key ? mmixByKey(this.setup, this.check.errors) : {};
		const missing: FieldErrors = {};
		if (this.floorLaws && !this.floorLaw) missing['laws.floor.id'] = ['Choose the floor\'s law.'];
		if (this.toneLaws && !this.toneLaw) missing['laws.tones.id'] = ['Choose the tones\' law.'];
		return mergeErrors(mergeErrors(mergeErrors(this.local, fromCheck), this.referenceErrors), missing);
	}

	get fieldWarnings(): FieldErrors {
		return this.check && this.checkedKey === this.key ? mmixByKey(this.setup, this.check.warnings) : {};
	}

	get floorLawErrors(): LawErrors {
		return lawKeys(this.fieldErrors, 'floor');
	}

	get toneLawErrors(): LawErrors {
		return lawKeys(this.fieldErrors, 'tones');
	}

	get counts(): Record<MmixSection, { errors: number; warnings: number }> {
		const out = Object.fromEntries(MMIX_SECTIONS.map((s) => [s.id, { errors: 0, warnings: 0 }])) as Record<MmixSection, { errors: number; warnings: number }>;
		for (const [k, v] of Object.entries(this.fieldErrors)) out[mmixSectionOf(k)].errors += v.length;
		for (const [k, v] of Object.entries(this.fieldWarnings)) out[mmixSectionOf(k)].warnings += v.length;
		out.channels.errors += this.acq.tableErrorCount;
		return out;
	}

	get setupErrorCount(): number {
		return Object.values(this.counts).reduce((n, c) => n + c.errors, 0);
	}

	/** The service's refusals and warnings of the draft as it stands, by name. */
	get findings(): { errors: MimoMixedReportItem[]; warnings: MimoMixedReportItem[] } {
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
		if (this.acq.stationDirty) return 'Use the chosen station, or put the choice back, first.';
		if (!this.floorLaws || !this.toneLaws) return 'The law catalogs have not been read yet.';
		if (!this.reference) return this.refBusy ? 'The floor\'s reference is being built.' : 'The floor\'s reference has not been built yet.';
		return null;
	}

	get runConfig(): MimoMixedConfig | null {
		return this.served?.config ?? null;
	}

	/** The latest numbers: the state, with this run's live frame laid over it when the frame came after it. */
	get now(): MimoMixedState | null {
		const s = this.status;
		const l = this.live;
		if (!s || !l || l.run_id !== s.run_id || !this.liveNewer) return s;
		return {
			...s,
			state: l.state,
			phase: l.phase,
			mode: l.mode,
			level_db: l.level_db,
			t_s: l.t_s,
			held: l.held,
			alarm: l.alarm,
			limiting: l.limiting,
			floor_error_db: l.floor_error_db,
			floor_equalized: l.floor_equalized,
			scale: l.scale,
			parts: l.parts,
			drives: l.drives
		};
	}

	get frameOfRun(): MimoMixedLiveHead | null {
		const s = this.status;
		const l = this.live;
		return s && l && l.run_id === s.run_id ? l : null;
	}

	private get verbContext(): MmixVerbContext {
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
		return mmixBarActions(this.verbContext);
	}

	get primary(): ActionId | null {
		return mmixPrimary(this.verbContext);
	}

	get verbs(): Record<MmixVerbId, ActionState> {
		return mmixModuleVerbs(this.verbContext, this.acknowledgement);
	}

	partToggle(row: { kind: 'floor' | 'tone' | 'narrowband'; enabled: boolean }) {
		return mmixPartToggle(this.verbContext, row);
	}

	/** The run's control and drive names, by channel, from the table: the service reports channels, not names. */
	get runControlLabels(): string[] {
		const s = this.now;
		return s?.parts[0]?.controls.length ? s.parts[0].controls.map((c) => rowLabel(this.rows, c.channel)) : this.controlLabels;
	}

	get runDriveLabels(): string[] {
		const s = this.now;
		return s?.drives.length ? s.drives.map((d) => rowLabel(this.rows, d.channel)) : this.driveLabels;
	}

	/** A channel's name in the table, for the rows the service reports by channel. */
	channelLabel(channel: number | null | undefined): string {
		return channel === null || channel === undefined ? '' : rowLabel(this.rows, channel);
	}

	partName(id: string): string {
		return this.now?.parts.find((p) => p.id === id)?.name ?? [...this.setup.tones, ...this.setup.bands].find((p) => p.id === id)?.name ?? id;
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
			// the service does not report the time at a level, nor what is left of it
			at_level_s: null,
			remaining_at_level_s: null,
			end: s.end ? ({ outcome: s.end.outcome, message: s.end.message, at_s: s.end.at_s, level_db: s.end.level_db, trip: null } as unknown as RunHead['end']) : null,
			record: s.record ?? null
		};
	}

	get frame(): { level_db: number | null; at_level_s: number | null; remaining_at_level_s: number | null } | null {
		const l = this.frameOfRun;
		if (!l || this.status?.state === 'finished') return null;
		return { level_db: l.level_db, at_level_s: null, remaining_at_level_s: null };
	}

	get record(): RunRecordRef | null {
		return this.status?.record ?? null;
	}

	readonly outcomeWords: Record<string, string> = MMIX_OUTCOME_WORDS;
	readonly phaseWords: Record<string, string> = MMIX_PHASE_WORDS;
	readonly stateWords: Record<string, string> = MMIX_STATE_WORDS;

	/** The run's time since start, station seconds. */
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

	/** Keep the draft across reloads, fit it to the table, and re-check it as it changes. */
	watch(): () => void {
		return $effect.root(() => {
			$effect(() => {
				const text = saveMmixSetup($state.snapshot(this.setup) as MmixDraft);
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
				const key = this.key;
				const refKey = this.refParamsKey;
				untrack(() => {
					if (this.drafts && (key !== this.checkedKey || refKey !== this.refKey)) this.scheduleCheck();
				});
			});
		});
	}

	async sync(): Promise<void> {
		const c = this.control;
		try {
			const [status, served] = await Promise.all([c.request<MimoMixedState>('mimo_mixed.state'), c.request<MimoMixedConfigResult>('mimo_mixed.config')]);
			this.syncError = null;
			this.served = served;
			this.onStatus(status);
		} catch (err) {
			this.syncError = toRefusal('Read the control service', err).message;
			return;
		}
		await this.refreshLaws();
		if (!this.drafts) return;
		fitChannels(this.setup, this.acq.rows);
		this.scheduleCheck(0);
	}

	private onStatus(s: MimoMixedState) {
		const was = this.status;
		if (regresses(was, s)) return;
		this.status = s;
		this.liveNewer = false;
		if (this.live && this.live.run_id === s.run_id && (s.state === 'unconfigured' || s.state === 'configured' || s.state === 'armed' || s.state === 'blocked')) this.live = null;
		if (s.run_id !== this.logRun) void this.loadLog(s.run_id);
		if (was?.run_id !== s.run_id && s.run_id) void this.readServed();
		if (s.state !== 'review') this.acknowledgement = '';
		if (mmixIsLive(s) && !mmixIsLive(was) && this.step === 'setup' && s.state !== 'armed') this.step = 'run';
	}

	async loadLog(runId: string | null): Promise<void> {
		if (runId !== this.logRun) this.log = [];
		this.logRun = runId;
		if (!runId) {
			this.live = null;
			return;
		}
		try {
			const res = await this.control.request<MimoMixedLogResult>('mimo_mixed.log', { since: 0 });
			if (this.logRun !== runId) return;
			const have = new Map(this.log.map((e) => [e.seq, e]));
			for (const e of res.entries as LogEntry[]) have.set(e.seq, e);
			this.log = [...have.values()].sort((a, b) => a.seq - b.seq).slice(-LOG_LIMIT);
		} catch {
			/* the entries come with each log event */
		}
	}

	private async readServed(): Promise<void> {
		try {
			this.served = await this.control.request<MimoMixedConfigResult>('mimo_mixed.config');
		} catch {
			/* kept as it was */
		}
	}

	// ---- the laws ---------------------------------------------------------------------------

	/** Both catalogs, each from its own world; a draft's law the catalog does not hold takes the first. */
	async refreshLaws(): Promise<void> {
		try {
			const [floor, tones] = await Promise.all([
				this.control.request<LawsListResult>('laws.list', { world: MIMO_MIXED_LAW_WORLDS.floor }),
				this.control.request<LawsListResult>('laws.list', { world: MIMO_MIXED_LAW_WORLDS.tones })
			]);
			this.floorLaws = floor.laws.filter((l) => l.world === MIMO_MIXED_LAW_WORLDS.floor);
			this.toneLaws = tones.laws.filter((l) => l.world === MIMO_MIXED_LAW_WORLDS.tones);
			this.lawProblems = [...floor.problems, ...tones.problems];
			this.fitLaw('floor');
			this.fitLaw('tones');
		} catch (err) {
			this.refusal = toRefusal('Read the control laws', err);
		}
	}

	lawsOf(role: MmixLawRole): LawEntry[] {
		return (role === 'floor' ? this.floorLaws : this.toneLaws) ?? [];
	}

	private fitLaw(role: MmixLawRole) {
		const list = this.lawsOf(role);
		if (!list.length || this.locked) return;
		const pick = this.setup.laws[role];
		const law = list.find((l) => l.id === pick.id) ?? list[0];
		const same = law.id === pick.id;
		const params = lawValues(law, same ? pick.params : undefined, this.setup.controls.length || 1);
		if (same && canonical(params) === canonical(pick.params)) return;
		this.setup.laws[role] = { id: law.id, params };
	}

	chooseLaw(role: MmixLawRole, id: string): boolean {
		const law = this.lawsOf(role).find((l) => l.id === id);
		if (!law || this.locked) return false;
		const pick = this.setup.laws[role];
		this.setup.laws[role] = { id, params: lawValues(law, id === pick.id ? pick.params : undefined, this.setup.controls.length || 1) };
		return true;
	}

	async importLaw(role: MmixLawRole, path: string): Promise<boolean> {
		const res = await this.act<{ law: LawEntry }>('Import Law', 'laws.import', { path: path.trim() });
		if (!res) return false;
		await this.refreshLaws();
		const world = MIMO_MIXED_LAW_WORLDS[role];
		if (res.law.world !== world) this.acq.note = { tone: 'warn', text: `${res.law.name} is not a law for the ${role === 'floor' ? 'floor' : 'tones'}; it was imported for its own test type.` };
		else if (this.chooseLaw(role, res.law.id)) this.acq.note = { tone: 'pass', text: `Imported ${res.law.name} ${res.law.version}.` };
		return true;
	}

	// ---- the parts --------------------------------------------------------------------------

	addTone(): boolean {
		if (this.locked) return false;
		const d = this.setup;
		d.tones.push(newTone(freeId(d, 't'), d.tones.length + 1, d.controls.length, 100 + 20 * d.tones.length));
		return true;
	}

	addBand(): boolean {
		if (this.locked) return false;
		const d = this.setup;
		d.bands.push(newBand(freeId(d, 'b'), d.bands.length + 1, d.controls.length));
		return true;
	}

	removePart(id: string) {
		if (this.locked) return;
		this.setup.tones = this.setup.tones.filter((t) => t.id !== id);
		this.setup.bands = this.setup.bands.filter((b) => b.id !== id);
	}

	addLimit(channel: number): boolean {
		if (this.locked || this.setup.limits.some((l) => l.channel === channel)) return false;
		this.setup.limits.push({ channel, ceiling: NaN, abort_margin_db: 3 });
		return true;
	}

	removeLimit(channel: number) {
		if (this.locked) return;
		this.setup.limits = this.setup.limits.filter((l) => l.channel !== channel);
	}

	// =========================================================================================
	// The reference and the dry run

	scheduleCheck(delay = this.checkDelayMs) {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = setTimeout(() => void this.checkSetup(), delay);
	}

	/** Build the floor's reference from the breakpoint tables, when they changed. */
	async buildReference(): Promise<MimoReferenceResult | null> {
		if (this.setup.reference.method !== 'breakpoints' || !this.connected || !this.setup.controls.length) return null;
		const key = this.refParamsKey;
		if (key === this.refKey && this.ref) return this.ref;
		const seq = ++this.refSeq;
		this.refBusy = true;
		try {
			const res = await this.control.request<MimoReferenceResult>('mimo_mixed.reference.build_breakpoints', breakpointParamsOf(this.setup, this.controlLabels) as unknown as Record<string, unknown>);
			if (seq === this.refSeq) {
				this.ref = res;
				this.refKey = key;
			}
			return res;
		} catch (err) {
			if (seq === this.refSeq) {
				const r = toRefusal('Build the Reference', err);
				this.ref = { ok: false, reference: null, labels: [], unit: 'g', df_hz: 0, band_hz: null, lines: 0, errors: [{ reason: 'out_of_range', message: r.message }], warnings: [], psd: { violations: 0, min_eigenvalue_ratio: null, worst_hz: null, lines_hz: [], repair: null, repair_accepted: false }, transforms: { response: null, drive: null }, provenance: null, views: [] } as unknown as MimoReferenceResult;
				this.refKey = key;
			}
			return null;
		} finally {
			if (seq === this.refSeq) this.refBusy = false;
		}
	}

	/** Read a reference file by its path on the computer that runs the test. */
	async importReference(): Promise<boolean> {
		const path = this.setup.reference.importPath.trim();
		if (!path) return false;
		const key = this.refParamsKey;
		this.refBusy = true;
		try {
			const res = await this.act<MimoReferenceResult>('Import Reference', 'mimo_mixed.reference.import', { path, grid: { fs_hz: this.setup.estimation.fs_hz, n_pts: this.setup.estimation.n_pts }, labels: this.controlLabels });
			if (!res) return false;
			this.ref = res;
			this.refKey = key;
			this.scheduleCheck(0);
			return res.ok;
		} finally {
			this.refBusy = false;
		}
	}

	async checkSetup(): Promise<MimoMixedDryRunResult | null> {
		if (this.checkTimer) clearTimeout(this.checkTimer);
		this.checkTimer = null;
		if (!this.connected || errorCount(localErrors(this.setup)) > 0) return null;
		await this.buildReference();
		if (!this.reference) return null;
		const config = this.wire;
		const key = this.key;
		const seq = ++this.checkSeq;
		this.checking = true;
		try {
			const res = await this.control.request<MimoMixedDryRunResult>('mimo_mixed.configure', { config, dry_run: true });
			if (seq === this.checkSeq) {
				this.check = res;
				this.checkedKey = key;
			}
			return res;
		} catch (err) {
			const r = toRefusal('Check the setup', err);
			if (seq === this.checkSeq && (r.reason === 'bad_params' || r.reason === 'config_invalid')) {
				const d = (r.detail ?? {}) as { errors?: MimoMixedReportItem[]; warnings?: MimoMixedReportItem[] };
				this.check = { errors: d.errors ?? [{ reason: 'bad_config', message: r.message }], warnings: d.warnings ?? [], derived: null, views: [] };
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
		const res = await this.act<MimoMixedConfigureResult>('Apply Setup', 'mimo_mixed.configure', { config });
		if (!res) {
			const r = this.refusal;
			if (r?.reason === 'config_invalid' && r.detail) {
				const d = r.detail as unknown as { errors?: MimoMixedReportItem[]; warnings?: MimoMixedReportItem[] };
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

	/** Open the station; nothing is driven. The arm takes no parameters. */
	async arm(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Arm', 'mimo_mixed.arm', {});
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** Identification first, then the review hold, or the floor alone at the first level. */
	async start(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Start', 'mimo_mixed.start');
		if (res) {
			this.onStatus(res);
			this.step = 'run';
		}
		return res !== null;
	}

	/** Past a review that holds nothing: the proceed takes no parameters. */
	async proceed(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Proceed', 'mimo_mixed.proceed', {});
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** The hold in force at the review, the one an override acknowledges; null when nothing holds. */
	get holdInForce(): MimoMixedOverrideReason | null {
		const s = this.now;
		return s?.state === 'review' && (s.phase === 'gate_failed' || s.phase === 'prediction_exceeded') ? s.phase : null;
	}

	/**
	 * Past a failed gate or a prediction over a ceiling, with the operator's words. The override
	 * names the hold it acknowledges; past a gate, the run goes on to the prediction's review, which
	 * may hold again.
	 */
	async override(reason?: MimoMixedOverrideReason): Promise<boolean> {
		const ack = this.acknowledgement.trim();
		const why = reason ?? this.holdInForce;
		const res = await this.act<MimoMixedState>('Override', 'mimo_mixed.override', why ? { ack, reason: why } : { ack });
		if (res) {
			this.acknowledgement = '';
			this.onStatus(res);
		}
		return res !== null;
	}

	async setMode(mode: LadderMode): Promise<boolean> {
		return (await this.act<{ mode: LadderMode }>('Ladder Mode', 'mimo_mixed.mode', { mode })) !== null;
	}

	async level(deltaDb: number): Promise<number | null> {
		return (await this.act<{ level_db: number }>('Level', 'mimo_mixed.level', { delta_db: deltaDb }))?.level_db ?? null;
	}

	async goTo(db: number): Promise<number | null> {
		return (await this.act<{ level_db: number }>('Go to Level', 'mimo_mixed.level', { to_db: db }))?.level_db ?? null;
	}

	async hold(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Hold', 'mimo_mixed.hold');
		if (res) this.onStatus(res);
		return res !== null;
	}

	async resume(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Resume', 'mimo_mixed.resume');
		if (res) this.onStatus(res);
		return res !== null;
	}

	/** Switch one part on or off live, as a ramp. A refusal says why in the part's words. */
	async part(id: string, on: boolean): Promise<boolean> {
		const res = await this.act<{ id: string; on: boolean }>(on ? 'Part On' : 'Part Off', 'mimo_mixed.part', { id, on });
		return res !== null;
	}

	async stop(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Stop', 'mimo_mixed.stop', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async abort(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Abort', 'mimo_mixed.abort', {}, false);
		if (res) this.onStatus(res);
		return res !== null;
	}

	async reset(): Promise<boolean> {
		const held = this.setupStatus === 'applied' ? this.held : null;
		const res = await this.act<MimoMixedState>('New Run', 'mimo_mixed.reset');
		if (!res) return false;
		this.onStatus(res);
		this.live = null;
		if (held) this.setHeld({ runId: res.run_id, key: held.key });
		return true;
	}

	/** Forget the setup the service holds. */
	async clear(): Promise<boolean> {
		const res = await this.act<MimoMixedState>('Clear', 'mimo_mixed.clear');
		if (!res) return false;
		this.onStatus(res);
		this.setHeld(null);
		return true;
	}

	/** A refusal under this module's heading for its name, with the facts its detail carries in words. */
	private inWords(r: Refusal): Refusal {
		const title = r.reason ? MMIX_REFUSAL_WORDS[r.reason as MimoMixedRefusalReason] : undefined;
		const facts = mmixRefusalFacts(r.reason, r.detail, { channel: (c) => this.channelLabel(c), part: (id) => this.partName(id) });
		return { ...r, title: title ?? r.title, facts: facts.length ? facts : r.facts };
	}

	private async act<R>(verb: string, method: string, params: Record<string, unknown> = {}, exclusive = true): Promise<R | null> {
		if (exclusive) this.busy = verb;
		this.#refusal = null;
		try {
			return await this.control.request<R>(method, params);
		} catch (err) {
			this.#refusal = this.inWords(toRefusal(verb, err));
			return null;
		} finally {
			if (exclusive && this.busy === verb) this.busy = null;
		}
	}
}
