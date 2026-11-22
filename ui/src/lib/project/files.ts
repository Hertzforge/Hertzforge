/**
 * Files by path. In the desktop app (a Tauri window) the shell gets a dropped file's path and the
 * native dialogs; the service then reads the file where it is, and nothing is sent. A browser
 * gives a dropped file's contents and name but never its path, so a drop there sends the
 * contents (`sendFile`, `project.upload`) and the service reads its copy; the project panel also
 * takes a typed path.
 *
 * Tauri is reached through the globals a Tauri v2 window exposes with `withGlobalTauri`, so the
 * shell needs no Tauri package of its own and runs unchanged in a browser.
 */

import type { ExportFormat } from '$lib/protocol/types';

/** What `project.open` reads as a whole project; anything else is imported into one. */
export const PROJECT_SUFFIXES = ['.hforge', '.vdyn', '.mat', '.h5', '.hdf5', '.escdf'];

/** Whether a path names a project file, which opens, rather than a file to import: a run folder's `run.json` opens as the project the folder converts to. */
export function isProjectFile(path: string): boolean {
	const lower = path.toLowerCase();
	return PROJECT_SUFFIXES.some((suffix) => lower.endsWith(suffix)) || isRunManifest(path);
}

/** The run folder of the one run record picked, when its folder is there; null otherwise. */
export function runRecordFolder(objects: { name: string; kind: string; run_folder?: string }[], picked: string[]): string | null {
	if (picked.length !== 1) return null;
	const one = objects.find((o) => o.name === picked[0]);
	return one?.kind === 'run_record' && one.run_folder ? one.run_folder : null;
}

/** Whether a path names a control run's `run.json`, which opens its run folder. */
export function isRunManifest(path: string): boolean {
	return (path.split(/[\\/]/).pop() ?? '') === 'run.json';
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

export interface DialogFilter {
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
		/** What a command reports back through while it runs, as the drag reports where it ended. */
		Channel?: new () => { onmessage: (message: unknown) => void };
	};
	event?: {
		listen: (name: string, handler: (event: { payload: string }) => void) => Promise<() => void>;
	};
	dialog?: {
		open(options: { multiple?: boolean; directory?: boolean; title?: string; defaultPath?: string; filters?: DialogFilter[] }): Promise<string | string[] | null>;
		save(options: { title?: string; defaultPath?: string; filters?: DialogFilter[] }): Promise<string | null>;
		ask(text: string, options?: { title?: string; kind?: 'info' | 'warning' | 'error'; okLabel?: string; cancelLabel?: string }): Promise<boolean>;
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

/** A file of one of `filters`' kinds chosen in the native open dialog; null when cancelled or in a browser. */
export async function chooseOpenFile(title: string, filters: DialogFilter[]): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	const chosen = await dialog.open({ title, multiple: false, filters });
	return Array.isArray(chosen) ? (chosen[0] ?? null) : chosen;
}

/** Where to write a file, chosen in the native save dialog; null when cancelled or in a browser. */
export async function chooseSaveFile(title: string, defaultPath: string, filters: DialogFilter[]): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	return dialog.save({ title, defaultPath, filters });
}

/** Whether to go on, asked in the native dialog or the browser's. */
export async function confirmFirst(text: string, ok: string, title = 'Hertzforge'): Promise<boolean> {
	const ask = tauri()?.dialog?.ask;
	if (ask) return ask(text, { title, okLabel: ok, cancelLabel: 'Cancel' });
	return globalThis.confirm?.(text) ?? false;
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
 * Call `onPaths` with the paths of files dropped on the window, in the desktop app. In a browser a
 * drop has no paths, so `onFiles` gets the files themselves, to be sent by their contents, and the
 * drop is kept from opening the file in the page. Returns the function that stops listening.
 */
export function listenForDrops(onPaths: (paths: string[]) => void, onFiles: (files: File[]) => void): () => void {
	const webview = tauri()?.webview;
	if (webview) {
		let stop: (() => void) | null = null;
		let stopped = false;
		void webview
			.getCurrentWebview()
			.onDragDropEvent((event) => {
				// a drag out let go over the window itself brings back the files it wrote: not an import
				const paths = (event.payload.paths ?? []).filter((path) => !draggedOut.has(path));
				if (event.payload.type === 'drop' && paths.length) onPaths(paths);
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
		onFiles(Array.from(e.dataTransfer?.files ?? []));
	};
	window.addEventListener('dragover', over);
	window.addEventListener('drop', drop);
	return () => {
		window.removeEventListener('dragover', over);
		window.removeEventListener('drop', drop);
	};
}

/** The most of a file one `project.upload` carries, under the service's frame limit. */
export const PIECE_BYTES = 4 * 2 ** 20;

/** What one `project.upload` sends: the file's name and size, and a piece of it at `offset`. */
export type UploadPiece = { name: string; size: number; offset: number; data: Uint8Array; upload?: string };

/**
 * Send a file a browser gave by its contents, in pieces of `PIECE_BYTES`, through `send` (one
 * `project.upload` each), and resolve with the upload's id, which `project.import` and
 * `project.open` take in place of a path. `sent` hears the bytes sent so far. A folder, which a
 * browser drops as a file it cannot read, is refused by name.
 */
export async function sendFile(
	file: File,
	send: (piece: UploadPiece) => Promise<{ upload: string; received: number }>,
	sent: (done: number, total: number) => void = () => {}
): Promise<string> {
	let upload: string | undefined;
	let offset = 0;
	do {
		let data: Uint8Array;
		try {
			data = new Uint8Array(await file.slice(offset, offset + PIECE_BYTES).arrayBuffer());
		} catch {
			throw new Error(`${file.name} cannot be read here: a browser sends files, not folders. Give a folder's path under Project.`);
		}
		const piece: UploadPiece = { name: file.name, size: file.size, offset, data };
		if (upload) piece.upload = upload;
		const got = await send(piece);
		upload = got.upload;
		offset = got.received;
		sent(offset, file.size);
	} while (offset < file.size);
	return upload!;
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

/** Whether to let a project's unsaved changes go: asked in the native dialog, or the browser's. */
export async function discardUnsaved(title: string): Promise<boolean> {
	const text = `${title} has unsaved changes. Discard them?`;
	const dialog = tauri()?.dialog;
	if (dialog) return dialog.ask(text, { title: 'Unsaved Changes', kind: 'warning', okLabel: 'Discard', cancelLabel: 'Cancel' });
	return typeof window !== 'undefined' && window.confirm(text);
}

/** Whether the app has a File menu (the macOS desktop app): without one, the top bar has the buttons. */
export function hasFileMenu(): boolean {
	return hasNativeFiles() && typeof navigator !== 'undefined' && /Mac/.test(navigator.platform || navigator.userAgent);
}

/** The File menu chosen ('new', 'open', 'import', 'save', 'save-as'); returns how to stop listening. */
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

/**
 * The files the operating system asked the desktop app to open (a double-clicked project, a run's
 * `run.json` opened with the app), each time more come: `onPaths` gets them in order. Nothing in a
 * browser. Returns how to stop listening.
 */
export function listenForOpenedFiles(onPaths: (paths: string[]) => void): () => void {
	const t = tauri();
	if (!t?.core || !t.dialog) return () => {};
	const core = t.core;
	const take = () =>
		void core
			.invoke<string[]>('opened_files', {})
			.then((paths) => {
				if (paths.length) onPaths(paths);
			})
			.catch(() => {});
	let stop: (() => void) | null = null;
	let stopped = false;
	if (t.event) {
		void t.event.listen('opened-files', () => take()).then((unlisten) => (stopped ? unlisten() : (stop = unlisten)));
	}
	// the files the app was opened with, queued before the window was there
	take();
	return () => {
		stopped = true;
		stop?.();
	};
}

/** The desktop app's run files: its data folder, a folder chooser and the file manager. Null in a browser. */
export function desktopRunFiles(): import('$lib/app/runFiles.svelte').DesktopRunFiles | null {
	const t = tauri();
	const core = t?.core;
	const dialog = t?.dialog;
	if (!core || !dialog) return null;
	return {
		dataFolder: () => core.invoke('data_folder', {}),
		setDataFolder: (path) => core.invoke('set_data_folder', { path }),
		chooseFolder: async (start) => {
			const picked = await dialog.open({ title: 'Where Runs Are Kept', directory: true, multiple: false, ...(start ? { defaultPath: start } : {}) });
			return Array.isArray(picked) ? (picked[0] ?? null) : picked;
		},
		showFolder: (path) => core.invoke('show_folder', { path })
	};
}

/**
 * Whether a report's page is printed in the system's browser and not in place: in the desktop app,
 * whose web view does not print a page that asks to be. There the report offers no Print (the saved
 * HTML file is opened in a browser instead), and an issue's page is opened in the browser by the
 * app's own analysis service, on the same machine (`report.issue` with `browser`).
 */
export function printsInBrowser(): boolean {
	return Boolean(tauri()?.core?.invoke && tauri()?.dialog);
}

/** The files dragged out of the window this session, which a drop back on it leaves alone. */
const draggedOut = new Set<string>();

/** Whether the shell can drag files out of the window: the desktop app, whose drag plugin hands them to the system. */
export function canDragOut(): boolean {
	const core = tauri()?.core;
	return Boolean(core?.invoke && core.Channel && tauri()?.dialog);
}

/** A picture of what is dragged: its name on a card, as a PNG data URL the drag plugin takes. */
export function dragPicture(label: string): string {
	// a clear pixel where there is no page to draw on, as the drag needs a picture of some kind
	const pixel = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';
	if (typeof document === 'undefined') return pixel;
	const canvas = document.createElement('canvas');
	const scale = Math.max(1, Math.round(globalThis.devicePixelRatio || 1));
	const width = Math.min(260, 24 + 7.5 * label.length);
	canvas.width = width * scale;
	canvas.height = 28 * scale;
	const g = canvas.getContext('2d');
	if (g) {
		g.scale(scale, scale);
		g.fillStyle = 'rgba(40, 44, 52, 0.92)';
		g.beginPath();
		g.roundRect(0.5, 0.5, width - 1, 27, 6);
		g.fill();
		g.fillStyle = '#ffffff';
		g.font = '13px -apple-system, "Segoe UI", sans-serif';
		g.textBaseline = 'middle';
		g.fillText(label, 12, 14, width - 24);
	} else {
		return pixel;
	}
	return canvas.toDataURL('image/png');
}

/**
 * Start the system's drag of files already written, from the desktop app: dropped in a file
 * manager or another application, they are copied there. Resolves when the drag starts; false
 * when there is no desktop app to drag from.
 */
export async function startDragOut(paths: string[], label: string): Promise<boolean> {
	const core = tauri()?.core;
	if (!core?.Channel || !paths.length) return false;
	for (const path of paths) draggedOut.add(path);
	const onEvent = new core.Channel();
	onEvent.onmessage = () => {};
	await core.invoke('plugin:drag|start_drag', { item: paths, image: dragPicture(label), options: { mode: 'copy' }, onEvent });
	return true;
}
