/**
 * The shared setup bar's state, one per test screen: Save Setup (the draft into the open project),
 * Open Setup (a saved setup from the project) and Open Setup From Run (the setup a run's record
 * in the project holds).
 *
 * Opening fills the draft and nothing else. A setup for another module or protocol is refused
 * before the draft is touched; one made with a different channel table, or one the draft cannot
 * hold exactly, waits for the operator with every difference named. Once in the draft it is
 * checked by the control service's dry run as an edit is, and the service's refusals are shown
 * in its own words. Nothing here applies, arms or starts anything.
 */

import { PROTOCOL, type ProjectSummary } from '$lib/protocol/types';
import { MODULE_TITLES, vetSetup, type SetupListing, type SetupOrigin, type TestSetupObject, type Vetting } from './setupFile';
import type { SetupHost } from './setupHosts';
import type { SetupOriginState } from './setupOrigin.svelte';

/** The analysis service's client, or anything that answers as it does. */
export interface AnalysisClient {
	readonly isOpen: boolean;
	request<R = Record<string, unknown>>(method: string, params?: Record<string, unknown>, options?: { timeoutMs?: number }): Promise<R>;
}

/** The open project, as the shell keeps it. */
export interface ProjectSink {
	readonly summary: ProjectSummary | null;
	apply(summary: ProjectSummary): void;
}

export interface Note {
	tone: 'pass' | 'warn' | 'info';
	text: string;
}

/** A setup read and vetted, waiting for the operator to see its differences. */
export interface Pending {
	name: string;
	setup: TestSetupObject;
	origin: SetupOrigin;
	vetting: Vetting;
}

function words(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

export class SetupFiles {
	listing = $state.raw<SetupListing | null>(null);
	pending = $state.raw<Pending | null>(null);
	/** Why the last setup asked for was not opened, in words. */
	refused = $state<string | null>(null);
	note = $state.raw<Note | null>(null);
	busy = $state<string | null>(null);
	saveName = $state('');
	saveNotes = $state('');
	/** The saved setup a save would replace, waiting for the operator's word. */
	replacing = $state<string | null>(null);

	constructor(
		readonly host: SetupHost,
		private readonly analysis: AnalysisClient,
		private readonly project: ProjectSink,
		readonly origin: SetupOriginState
	) {}

	get projectOpen(): boolean {
		return this.analysis.isOpen && this.project.summary !== null;
	}

	/** Why Save Setup cannot run now, or null. */
	get saveBlocked(): string | null {
		if (!this.projectOpen) return 'Open or start a project first: a setup is saved into it.';
		if (!this.host.config()) return 'The draft makes no setup yet.';
		return null;
	}

	/** Why the Open verbs cannot run now, or null. */
	get openBlocked(): string | null {
		if (!this.projectOpen) return 'Open a project first: setups are opened from it.';
		return this.host.locked();
	}

	/** This module's saved setups first, then the others, which open on their own screens. */
	get setups() {
		const all = this.listing?.setups ?? [];
		return [...all.filter((s) => s.module === this.host.module), ...all.filter((s) => s.module !== this.host.module)];
	}

	/** The project's runs that hold a setup, this module's first. */
	get runs() {
		const all = (this.listing?.runs ?? []).filter((r) => r.has_config);
		return [...all.filter((r) => r.module === this.host.module), ...all.filter((r) => r.module !== this.host.module)];
	}

	/** Where the draft came from, in words. */
	get originWords(): string {
		return this.origin.words;
	}

	/** The service's refusals of an opened setup, while it is as opened. */
	get serviceWords(): string[] {
		if (this.origin.origin.kind === 'typed' || this.origin.edited) return [];
		return this.host.findings();
	}

	/** Read what the project offers (`project.setups`). */
	async refresh(): Promise<void> {
		if (!this.analysis.isOpen) return;
		try {
			this.listing = await this.analysis.request<SetupListing>('project.setups', {});
		} catch (err) {
			this.note = { tone: 'warn', text: `The project's setups could not be listed: ${words(err)}` };
		}
	}

	/** Save the draft's setup into the project under `saveName` (`project.save_setup`). */
	async save(replace = false): Promise<boolean> {
		const why = this.saveBlocked;
		const config = this.host.config();
		const name = this.saveName.trim() || this.host.title().trim() || MODULE_TITLES[this.host.module];
		if (why || !config) {
			this.note = { tone: 'warn', text: why ?? 'The draft makes no setup yet.' };
			return false;
		}
		if (!replace && this.listing?.setups.some((s) => s.name === name)) {
			this.replacing = name;
			return false;
		}
		const setup: TestSetupObject = {
			module: this.host.module,
			protocol: this.host.protocol() ?? PROTOCOL,
			config,
			table: this.host.table(),
			title: this.host.title(),
			notes: this.saveNotes,
			origin: this.origin.recorded
		};
		this.busy = 'Saving the setup';
		try {
			const res = await this.analysis.request<{ name: string; project: ProjectSummary }>('project.save_setup', { name, setup, ...(replace ? { replace: true } : {}) });
			this.project.apply(res.project);
			this.origin.set({ kind: 'setup', name: res.name }, this.host.key());
			this.replacing = null;
			this.note = { tone: 'pass', text: `Saved as "${res.name}" in the project. Save the project to keep it.` };
			await this.refresh();
			return true;
		} catch (err) {
			this.note = { tone: 'warn', text: `The setup was not saved: ${words(err)}` };
			return false;
		} finally {
			this.busy = null;
		}
	}

	/** Open a saved setup by its name in the project. */
	openSaved(name: string): Promise<boolean> {
		return this.open(name, () => ({ kind: 'setup', name }));
	}

	/** Open the setup a run record in the project holds, by the record's name. */
	openRun(name: string): Promise<boolean> {
		return this.open(name, (s) => s.origin);
	}

	private async open(name: string, originOf: (s: TestSetupObject) => SetupOrigin): Promise<boolean> {
		this.refused = null;
		this.pending = null;
		this.note = null;
		const why = this.openBlocked;
		if (why) {
			this.refused = why;
			return false;
		}
		this.busy = `Opening "${name}"`;
		let setup: TestSetupObject;
		try {
			setup = (await this.analysis.request<{ setup: TestSetupObject }>('project.open_setup', { name })).setup;
		} catch (err) {
			this.refused = `"${name}" was not opened: ${words(err)}`;
			return false;
		} finally {
			this.busy = null;
		}
		const vetting = vetSetup(setup, { module: this.host.module, protocol: this.host.protocol(), rows: this.host.rows(), roundTrip: (c) => this.host.roundTrip(c) });
		if (vetting.refused) {
			this.refused = vetting.refused;
			return false;
		}
		const pending = { name, setup, origin: originOf(setup), vetting };
		if (vetting.table.length || vetting.config.length) {
			this.pending = pending;
			return false;
		}
		return this.take(pending);
	}

	/** The operator has seen the differences: the draft takes the setup. */
	confirm(): boolean {
		const p = this.pending;
		if (!p) return false;
		// The screen may have changed while the question was open: the setup is vetted again.
		const why = this.host.locked();
		if (why) {
			this.refused = why;
			this.pending = null;
			return false;
		}
		return this.take(p);
	}

	cancel(): void {
		this.pending = null;
		this.replacing = null;
	}

	private take(p: Pending): boolean {
		this.pending = null;
		this.host.fill(p.setup.config);
		this.origin.set(p.origin, this.host.key());
		this.note = { tone: 'info', text: `"${p.name}" is in the draft. Nothing is applied or armed: the control service checks it as it checks a typed setup, and it is applied only when you apply it.` };
		return true;
	}
}
