/**
 * Run folders as files a person manages: where runs are kept (the data folder), Show Folder,
 * Copy Folder Path and Delete Run for any run the shell names.
 *
 * In the desktop app the data folder is the app's (it starts the control service with it) and can
 * be changed there, from the services' next start; in a browser it is read from the analysis
 * service, which resolves it as the control service does, and is shown only. Showing a folder in
 * the file manager needs the desktop app. A run is deleted by the analysis service alone
 * (`runs.delete`), after a confirmation that names the folder and its project file; the shell
 * deletes nothing itself.
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

/** The confirmation of a delete: both paths, by name, and that it cannot be undone. */
export function deleteQuestion(plan: Pick<RunDeletePlan, 'folder' | 'project_file' | 'project_file_exists'>): string {
	const file = plan.project_file_exists
		? `and its project file, inside it:\n${plan.project_file}`
		: `It has not been opened in Analysis, so it has no project file (${plan.project_file}).`;
	return `Delete this run folder?\n${plan.folder}\n\n${file}\n\nBoth go from the disk. This cannot be undone.`;
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
			const plan = await this.host.request<RunDeletePlan>('runs.delete', { folder, dry_run: true });
			if (this.folder !== folder) return false;
			if (!(await this.host.confirm(deleteQuestion(plan), 'Delete Run', 'Delete Run?'))) {
				this.status = 'idle';
				return false;
			}
			this.status = 'working';
			await this.host.request<RunDeletePlan>('runs.delete', { folder });
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
}

/** The line under a run's actions, for its delete status; null when there is nothing to say. */
export function deleteLine(status: DeleteStatus, refusal: string | null): string | null {
	if (status === 'asking') return 'Checking the run folder before asking.';
	if (status === 'working') return 'Deleting the run folder and its project file.';
	if (status === 'done') return 'Deleted the run folder and its project file.';
	if (status === 'refused') return `The run was not deleted. ${refusal ?? ''}`.trim();
	return null;
}
