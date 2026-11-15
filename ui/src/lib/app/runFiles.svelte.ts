/**
 * Run folders as files a person manages: where runs are kept (the data folder), Show Folder,
 * Copy Folder Path and Delete Run for any run the shell names.
 *
 * In the desktop app the data folder is the app's (it starts the control service with it) and can
 * be changed there, from the services' next start; in a browser it is read from the analysis
 * service, which resolves it as the control service does, and is shown only. Showing a folder in
 * the file manager needs the desktop app. A run is deleted by the analysis service alone
 * (`runs.delete`), after a confirmation that names the folder and its project file; the shell
 * deletes nothing itself. A run that did not end cleanly is deleted only when the control service,
 * asked just then, reports no live run recording into its folder; while it cannot be asked, or when
 * the folder is a live run's record, the run is not deleted.
 */

import type { DataFolderInfo, RunDeletePlan } from '$lib/protocol/types';
import { RequestError } from '$lib/protocol/client.svelte';

/** The data folder as shown: the service's answer, and in the desktop app what the app keeps. */
export interface DataFolderView extends DataFolderInfo {
	/** Whether it can be changed here (the desktop app). */
	settable?: boolean;
	/** The folder chosen in the app, or null when the variable or the default is in use. */
	chosen?: string | null;
	/** The folder the next start uses, when it differs from the one in use. */
	next?: string;
	/** The default folder, for Use Default. */
	default?: string;
}

/** What the desktop app offers; null in a browser. */
export interface DesktopRunFiles {
	dataFolder(): Promise<DataFolderView>;
	setDataFolder(path: string | null): Promise<DataFolderView>;
	chooseFolder(start: string): Promise<string | null>;
	showFolder(path: string): Promise<void>;
}

export interface RunFilesHost {
	/** A request to the analysis service. */
	request<R>(method: string, params: Record<string, unknown>): Promise<R>;
	/** A request to the control service, which reads module states only here; absent when there is none to ask. */
	control?(method: string, params: Record<string, unknown>): Promise<unknown>;
	desktop: DesktopRunFiles | null;
	/** Ask before deleting; true to go on. */
	confirm(text: string, ok: string, title: string): Promise<boolean>;
	/** A run was deleted: the project is told, so a record of it can go. */
	deleted?(folder: string): void;
}

export type DeleteStatus = 'idle' | 'asking' | 'working' | 'done' | 'refused';

/** A message from a refusal or a failure, in the service's own words. */
function said(err: unknown): string {
	if (err instanceof RequestError) return err.message;
	return err instanceof Error ? err.message : String(err);
}

/** The confirmation of a delete: both paths, by name, that a run which did not end cleanly may hold a partial record, and that it cannot be undone. */
export function deleteQuestion(plan: Pick<RunDeletePlan, 'folder' | 'project_file' | 'project_file_exists'> & { complete?: boolean }): string {
	const file = plan.project_file_exists
		? `and its project file, inside it:\n${plan.project_file}`
		: `It has not been opened in Analysis, so it has no project file (${plan.project_file}).`;
	const unclean =
		plan.complete === false ? '\n\nThis run did not end cleanly: its record may be partial. The control service reports no live run recording into this folder.' : '';
	return `Delete this run folder?\n${plan.folder}\n\n${file}${unclean}\n\nBoth go from the disk. This cannot be undone.`;
}

/**
 * Each module's state request, the states in which its run is live, and its name in words: the
 * modules the desktop app's read-only probe asks about. A module the service does not have is idle.
 */
export const MODULE_STATES: readonly (readonly [string, readonly string[], string])[] = [
	['random.state', ['armed', 'running', 'review', 'stopping'], 'a Random test'],
	['mimo_random.status', ['armed', 'running', 'review', 'stopping'], 'a MIMO Random test'],
	['sine.state', ['armed', 'running', 'review', 'stopping'], 'a Sine test'],
	['mixed.state', ['armed', 'running', 'review', 'stopping'], 'a Mixed Mode test'],
	['shock.state', ['armed', 'running', 'review', 'stopping'], 'a Classical Shock test'],
	['srs.state', ['armed', 'running', 'review', 'stopping'], 'an SRS test'],
	['twr.state', ['armed', 'running', 'review', 'held', 'stopping'], 'a TWR test'],
	['mimo_shock.state', ['armed', 'running', 'review', 'stopping'], 'a MIMO Shock test'],
	['explorer.state', ['starting', 'running', 'stopping'], 'an Explorer run'],
	['recorder.state', ['starting', 'running', 'stopping'], 'a Recorder run']
];

/** A run the control service reports as live, and the folder it records into (null when it has not said). */
export interface LiveRun {
	name: string;
	folder: string | null;
}

/** The control service could not be asked which runs are live. */
export class ControlUnreachable extends Error {}

/**
 * The runs live now, asked of the control service module by module. A module the service does not
 * have (`unknown_method`) is idle; any other failure means the service could not be asked.
 */
export async function liveRuns(control: RunFilesHost['control']): Promise<LiveRun[]> {
	if (!control) throw new ControlUnreachable('the control service is not connected');
	const out: LiveRun[] = [];
	for (const [method, live, name] of MODULE_STATES) {
		let answer: unknown;
		try {
			answer = await control(method, {});
		} catch (err) {
			if (err instanceof RequestError && err.code === 'unknown_method') continue;
			throw new ControlUnreachable(said(err));
		}
		const a = (answer ?? {}) as { state?: unknown; record?: { folder?: unknown } | null };
		const nested = (typeof a.state === 'object' && a.state !== null ? a.state : {}) as { state?: unknown; record?: { folder?: unknown } | null };
		const state = typeof a.state === 'string' ? a.state : typeof nested.state === 'string' ? nested.state : '';
		if (!live.includes(state)) continue;
		const folder = a.record?.folder ?? nested.record?.folder;
		out.push({ name, folder: typeof folder === 'string' && folder !== '' ? folder : null });
	}
	return out;
}

/** A folder's path as compared here: forward slashes, no trailing separator, no `run.json`. */
function pathKey(path: string): string {
	return path.replace(/\\/g, '/').replace(/\/run\.json$/, '').replace(/\/+$/, '');
}

/** Whether two paths name one run folder: the same path, or the same run folder name (run folder names are unique in the data folder, so this errs toward refusing). */
function sameRun(a: string, b: string): boolean {
	const ka = pathKey(a);
	const kb = pathKey(b);
	const base = (k: string) => k.slice(k.lastIndexOf('/') + 1);
	return ka === kb || ka.toLowerCase() === kb.toLowerCase() || base(ka) === base(kb);
}

/**
 * Why a run that did not end cleanly is not deleted now, or null when the control service reports no
 * live run recording into it: it is a live run's record, a live run has not said where it records, or
 * the service could not be asked.
 */
export async function liveReason(control: RunFilesHost['control'], folders: string[]): Promise<string | null> {
	let live: LiveRun[];
	try {
		live = await liveRuns(control);
	} catch (err) {
		return `This run did not end cleanly, and the control service could not be asked whether it is still going (${err instanceof Error ? err.message : String(err)}), so it is not deleted.`;
	}
	for (const run of live) {
		if (run.folder === null) {
			return `This run did not end cleanly, and ${run.name} is live without saying which folder it records into, so it is not deleted now; delete it once that run has ended.`;
		}
		if (folders.some((f) => sameRun(f, run.folder as string))) {
			return `This run folder is the record of ${run.name} that is live now, so it is not deleted; delete it once that run has ended.`;
		}
	}
	return null;
}

/** Where the data folder comes from, in words. */
export function dataFolderSource(view: Pick<DataFolderView, 'source'>): string {
	if (view.source === 'setting') return 'Chosen here.';
	if (view.source === 'environment') return 'Named by HERTZFORGE_DATA_ROOT.';
	return 'The default, in the home folder.';
}

export class RunFiles {
	dataFolder = $state.raw<DataFolderView | null>(null);
	/** Why the data folder could not be read or changed. */
	dataError = $state<string | null>(null);
	dataBusy = $state(false);
	/** The folder the delete status is about. */
	folder = $state<string | null>(null);
	status = $state<DeleteStatus>('idle');
	/** The service's words when a delete was refused or failed. */
	refusal = $state<string | null>(null);
	/** Folders deleted in this session. */
	deleted = $state.raw<string[]>([]);
	/** Copy Folder Path and Show Folder: the folder, and how it went. */
	copied = $state<{ folder: string; ok: boolean } | null>(null);
	shown = $state<{ folder: string; error: string | null } | null>(null);

	constructor(private readonly host: RunFilesHost) {}

	/** Whether folders can be shown in the file manager (the desktop app). */
	get canShow(): boolean {
		return this.host.desktop !== null;
	}

	/** Whether the data folder can be changed here (the desktop app). */
	get settable(): boolean {
		return this.host.desktop !== null && this.dataFolder?.settable !== false;
	}

	/** Read where runs are kept: from the app in the desktop app, else from the analysis service. */
	async loadDataFolder(): Promise<void> {
		try {
			this.dataFolder = this.host.desktop ? await this.host.desktop.dataFolder() : await this.host.request<DataFolderInfo>('runs.data_folder', {});
			this.dataError = null;
		} catch (err) {
			this.dataError = said(err);
		}
	}

	/** Choose another data folder (null: back to the default), from the services' next start. */
	async setDataFolder(path: string | null): Promise<boolean> {
		const desktop = this.host.desktop;
		if (!desktop || this.dataBusy) return false;
		this.dataBusy = true;
		try {
			this.dataFolder = await desktop.setDataFolder(path);
			this.dataError = null;
			return true;
		} catch (err) {
			this.dataError = said(err);
			return false;
		} finally {
			this.dataBusy = false;
		}
	}

	/** Pick a folder in the system's window, then keep it. */
	async chooseDataFolder(): Promise<boolean> {
		const desktop = this.host.desktop;
		if (!desktop) return false;
		const picked = await desktop.chooseFolder(this.dataFolder?.next ?? this.dataFolder?.path ?? '');
		return picked ? this.setDataFolder(picked) : false;
	}

	async showFolder(folder: string): Promise<boolean> {
		const desktop = this.host.desktop;
		if (!desktop) return false;
		try {
			await desktop.showFolder(folder);
			this.shown = { folder, error: null };
			return true;
		} catch (err) {
			this.shown = { folder, error: said(err) };
			return false;
		}
	}

	async copyPath(folder: string, clipboard: Pick<Clipboard, 'writeText'> | undefined = globalThis.navigator?.clipboard): Promise<boolean> {
		let ok = false;
		try {
			if (clipboard) {
				await clipboard.writeText(folder);
				ok = true;
			}
		} catch {
			ok = false;
		}
		this.copied = { folder, ok };
		return ok;
	}

	/** The delete status for `folder`. */
	at(folder: string | null): DeleteStatus {
		if (folder !== null && this.deleted.includes(folder)) return 'done';
		return folder !== null && folder === this.folder ? this.status : 'idle';
	}

	/**
	 * Delete a run: the service checks it and names what would go, the person confirms with both
	 * paths in front of them, and the service deletes it. A refusal at either step says why.
	 */
	async deleteRun(folder: string): Promise<boolean> {
		if (this.at(folder) === 'working' || this.at(folder) === 'asking') return false;
		this.folder = folder;
		this.refusal = null;
		this.status = 'asking';
		try {
			// a dry run deletes nothing; `not_live` on it only lets its answer say whether the run ended cleanly
			const plan = await this.host.request<RunDeletePlan>('runs.delete', { folder, dry_run: true, not_live: true });
			if (this.folder !== folder) return false;
			const unclean = plan.complete === false;
			if (unclean && (await this.refuseLive(folder, plan.folder))) return false;
			if (!(await this.host.confirm(deleteQuestion(plan), 'Delete Run', 'Delete Run?'))) {
				this.status = 'idle';
				return false;
			}
			// asked again just before the delete: a run may have started while the question was open
			if (unclean && (await this.refuseLive(folder, plan.folder))) return false;
			this.status = 'working';
			await this.host.request<RunDeletePlan>('runs.delete', unclean ? { folder, not_live: true } : { folder });
			this.deleted = [...this.deleted, folder];
			this.status = 'done';
			this.host.deleted?.(folder);
			return true;
		} catch (err) {
			if (this.folder === folder) {
				this.refusal = said(err);
				this.status = 'refused';
			}
			return false;
		}
	}

	/** For a run that did not end cleanly: refuse, saying why, unless the control service reports no live run recording into it. */
	private async refuseLive(folder: string, real: string): Promise<boolean> {
		const why = await liveReason(this.host.control?.bind(this.host), [folder, real]);
		if (why === null) return false;
		if (this.folder === folder) {
			this.refusal = why;
			this.status = 'refused';
		}
		return true;
	}
}

/** The line under a run's actions, for its delete status; null when there is nothing to say. */
export function deleteLine(status: DeleteStatus, refusal: string | null): string | null {
	if (status === 'asking') return 'Checking the run folder before asking.';
	if (status === 'working') return 'Deleting the run folder and its project file.';
	if (status === 'done') return 'Deleted the run folder and its project file.';
	if (status === 'refused') return `The run was not deleted. ${refusal ?? ''}`.trim();
	return null;
}
