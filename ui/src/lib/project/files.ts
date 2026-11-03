/**
 * Files by path. In the desktop app (a Tauri window) the shell gets a dropped file's path and the
 * native dialogs; the service then reads the file where it is, and nothing is uploaded. In a
 * browser, which gives a dropped file's contents but never its path, the project panel takes a
 * typed path instead (OPENING_FILES_PLAN.md, section 1).
 *
 * Tauri is reached through the globals a Tauri v2 window exposes with `withGlobalTauri`, so the
 * shell needs no Tauri package of its own and runs unchanged in a browser.
 */

/** What `project.open` reads as a whole project; anything else is imported into one. */
export const PROJECT_SUFFIXES = ['.hforge', '.vdyn', '.mat', '.h5', '.hdf5', '.escdf'];

/** Whether a path names a project file, which opens, rather than a file to import. */
export function isProjectFile(path: string): boolean {
	const lower = path.toLowerCase();
	return PROJECT_SUFFIXES.some((suffix) => lower.endsWith(suffix));
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
	dialog?: {
		open(options: { multiple?: boolean; directory?: boolean; title?: string; filters?: DialogFilter[] }): Promise<string | string[] | null>;
		save(options: { title?: string; defaultPath?: string; filters?: DialogFilter[] }): Promise<string | null>;
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

/** A path for a report's page chosen in the native save dialog, or null when cancelled. */
export async function chooseReportPath(title: string): Promise<string | null> {
	const dialog = tauri()?.dialog;
	if (!dialog) return null;
	return dialog.save({ title: 'Save Report', defaultPath: `${title}.html`, filters: [{ name: 'Web page', extensions: ['html'] }] });
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
