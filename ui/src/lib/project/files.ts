/**
 * Files by path. In the desktop app (a Tauri window) the shell gets a dropped file's path and the
 * native dialogs; the service then reads the file where it is, and nothing is uploaded. In a
 * browser, which gives a dropped file's contents but never its path, the project panel takes a
 * typed path instead (OPENING_FILES_PLAN.md, section 1).
 *
 * Tauri is reached through the globals a Tauri v2 window exposes with `withGlobalTauri`, so the
 * shell needs no Tauri package of its own and runs unchanged in a browser.
 */

import type { ExportFormat } from '$lib/protocol/types';

/** What `project.open` reads as a whole project; anything else is imported into one. */
export const PROJECT_SUFFIXES = ['.hforge', '.vdyn', '.mat', '.h5', '.hdf5', '.escdf'];

/** Whether a path names a project file, which opens, rather than a file to import. */
export function isProjectFile(path: string): boolean {
	const lower = path.toLowerCase();
	return PROJECT_SUFFIXES.some((suffix) => lower.endsWith(suffix));
}

/**
 * Which of several paths opens and which import into it. The first project file opens, whatever
 * order they came in: an import before it would go with the project it replaces, and a second project
 * file opened after it would replace the first.
 */
export function openingOrder(paths: string[]): { opens: string | null; imports: string[] } {
	const opens = paths.find(isProjectFile) ?? null;
	return { opens, imports: paths.filter((path) => path !== opens) };
}

/** Whether the person will lose the unsaved changes in `title` to open `next`. */
export async function askToReplace(title: string, next: string): Promise<boolean> {
	const text = `${title} has unsaved changes, and opening ${next} replaces it. Open it anyway?`;
	const ask = tauri()?.dialog?.ask;
	if (ask) return ask(text, { title: 'Hertzforge', okLabel: 'Open', cancelLabel: 'Keep Working' });
	return globalThis.confirm?.(text) ?? false;
}

/** The last part of a path, for messages. */
export function baseName(path: string): string {
	return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

interface DialogFilter {
	name: string;
	extensions: string[];
}

interface DragDropEvent {
	payload: { type: 'enter' | 'over' | 'drop' | 'leave'; paths?: string[] };
}

/** The part of Tauri's global API the shell uses. */
interface TauriGlobals {
	core?: {
		invoke<T>(command: string, args: Record<string, unknown>): Promise<T>;
	};
	event?: {
		listen: (name: string, handler: (event: { payload: string }) => void) => Promise<() => void>;
	};
	dialog?: {
		open(options: { multiple?: boolean; directory?: boolean; title?: string; filters?: DialogFilter[] }): Promise<string | string[] | null>;
		save(options: { title?: string; defaultPath?: string; filters?: DialogFilter[] }): Promise<string | null>;
		ask?(text: string, options: { title?: string; okLabel?: string; cancelLabel?: string }): Promise<boolean>;
	};
	webview?: {
		getCurrentWebview(): { onDragDropEvent(handler: (event: DragDropEvent) => void): Promise<() => void> };
	};
}

function tauri(): TauriGlobals | null {
	return (globalThis as { __TAURI__?: TauriGlobals }).__TAURI__ ?? null;
}

/** Whether the shell is in the desktop app, which can pick and drop files by path. */
export function hasNativeFiles(): boolean {
	return Boolean(tauri()?.dialog);
}

const PROJECT_FILTER: DialogFilter = { name: 'Projects', extensions: PROJECT_SUFFIXES.map((s) => s.slice(1)) };

/** Paths chosen in the native open dialog: one project, or files to import. Null when cancelled. */
export async function chooseFiles(purpose: 'open' | 'import'): Promise<string[] | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	const chosen = await dialog.open(
		purpose === 'open'
			? { title: 'Open Project', multiple: false, filters: [PROJECT_FILTER] }
			: { title: 'Import Files', multiple: true }
	);
	if (chosen === null) return null;
	return Array.isArray(chosen) ? chosen : [chosen];
}

/** A path chosen in the native save dialog, or null when cancelled. */
export async function chooseSavePath(title: string): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	return dialog.save({ title: 'Save Project', defaultPath: `${title}.hforge`, filters: [{ name: 'Hertzforge project', extensions: ['hforge'] }] });
}

/** A path for a run log's export chosen in the native save dialog, or null when cancelled. */
export async function chooseExportPath(title: string, defaultPath: string, filter: DialogFilter): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	return dialog.save({ title, defaultPath, filters: [filter] });
}

/** An object's name as a file's name: the characters a path gives a meaning to are replaced. */
export function fileNameOf(name: string): string {
	return name.replace(/[\\/:]/g, '-');
}

/** The format a path's suffix names, the first when two share it, or else the first of all. */
export function formatForPath(path: string, formats: ExportFormat[]): string {
	const lower = path.toLowerCase();
	return (formats.find((f) => f.suffix && lower.endsWith(f.suffix.toLowerCase())) ?? formats[0]).value;
}

/**
 * Where to write an object and in which of its formats, from the native save window with the
 * formats as its file types. Null when cancelled. macOS's window comes from the app itself, since
 * the dialog plugin's has no file type list there. Elsewhere the plugin's has one but does not say
 * which was chosen, so the path's suffix says, and of two formats sharing a suffix the first is written.
 */
export async function chooseObjectFile(name: string, formats: ExportFormat[]): Promise<{ path: string; format: string } | null> {
	const t = tauri();
	if (!t?.dialog) return null;
	const title = `Export ${name}`;
	const stem = fileNameOf(name);
	if (hasFileMenu() && t.core) {
		return t.core.invoke<{ path: string; format: string } | null>('save_with_file_types', { title, name: stem, types: formats });
	}
	const path = await t.dialog.save({
		title,
		defaultPath: `${stem}${formats[0].suffix}`,
		filters: formats.filter((f) => f.suffix).map((f) => ({ name: f.label, extensions: [f.suffix.slice(1)] }))
	});
	return path === null ? null : { path, format: formatForPath(path, formats) };
}

/** A path for a report's page chosen in the native save dialog, or null when cancelled. */
export async function chooseReportPath(title: string): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	return dialog.save({ title: 'Save Report', defaultPath: `${title}.html`, filters: [{ name: 'Web page', extensions: ['html'] }] });
}

/** The paths of files on the clipboard, in the desktop app on a Mac; none anywhere else. */
export async function pastedFiles(): Promise<string[]> {
	const t = tauri();
	if (!t?.core || !t.dialog) return [];
	try {
		return await t.core.invoke<string[]>('pasted_files', {});
	} catch {
		// no such command off a Mac
		return [];
	}
}

/**
 * Call `onPaths` with the paths of files dropped on the window. In a browser a drop has no paths,
 * so `onPaths` gets null and the caller says to use the desktop app or type the path; the drop is
 * kept from opening the file in the page. Returns the function that stops listening.
 */
export function listenForDrops(onPaths: (paths: string[] | null) => void): () => void {
	const webview = tauri()?.webview;
	if (webview) {
		let stop: (() => void) | null = null;
		let stopped = false;
		void webview
			.getCurrentWebview()
			.onDragDropEvent((event) => {
				if (event.payload.type === 'drop' && event.payload.paths?.length) onPaths(event.payload.paths);
			})
			.then((unlisten) => (stopped ? unlisten() : (stop = unlisten)));
		return () => {
			stopped = true;
			stop?.();
		};
	}
	const carriesFiles = (e: DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes('Files');
	const over = (e: DragEvent) => {
		if (carriesFiles(e)) e.preventDefault();
	};
	const drop = (e: DragEvent) => {
		if (!carriesFiles(e)) return;
		e.preventDefault();
		onPaths(null);
	};
	window.addEventListener('dragover', over);
	window.addEventListener('drop', drop);
	return () => {
		window.removeEventListener('dragover', over);
		window.removeEventListener('drop', drop);
	};
}

/** A PDF chosen in the native open dialog (the one an issue was printed to), or null when cancelled. */
export async function choosePdf(): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	const chosen = await dialog.open({ title: 'The Issued PDF', multiple: false, filters: [{ name: 'PDF', extensions: ['pdf'] }] });
	return Array.isArray(chosen) ? (chosen[0] ?? null) : chosen;
}

const RUN_LOG_FILTER: DialogFilter = { name: 'Run log', extensions: ['hflog', 'vdlog'] };

/** A run log chosen in the native dialog: one to open, or where to start one. Null when cancelled. */
export async function chooseRunLog(purpose: 'open' | 'new'): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	if (purpose === 'new') return dialog.save({ title: 'New Run Log', defaultPath: 'Run log.hflog', filters: [RUN_LOG_FILTER] });
	const chosen = await dialog.open({ title: 'Open Run Log', multiple: false, filters: [RUN_LOG_FILTER] });
	return Array.isArray(chosen) ? (chosen[0] ?? null) : chosen;
}

/** Whether the app has a File menu (the macOS desktop app): without one, the top bar has the buttons. */
export function hasFileMenu(): boolean {
	return hasNativeFiles() && typeof navigator !== 'undefined' && /Mac/.test(navigator.platform || navigator.userAgent);
}

/** The File menu chosen ('open', 'import', 'save', 'save-as'); returns how to stop listening. */
export function listenForFileMenu(onAct: (act: string) => void): () => void {
	const events = tauri()?.event;
	if (!events) return () => {};
	let stop: (() => void) | null = null;
	let stopped = false;
	void events
		.listen('file-menu', (event) => onAct(event.payload))
		.then((unlisten) => (stopped ? unlisten() : (stop = unlisten)));
	return () => {
		stopped = true;
		stop?.();
	};
}
