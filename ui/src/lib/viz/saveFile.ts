/**
 * A file the shell made itself (a plot's image, its lines as CSV) saved where the person chooses.
 * In the desktop app the app's save window asks where and the app writes the file there; in a
 * browser the file is downloaded. Says which happened.
 */

export interface SaveFilter {
	/** The kind of file, in the save window's list ("PNG image"). */
	name: string;
	/** Suffixes without the dot. */
	extensions: string[];
}

interface TauriCore {
	invoke<T>(command: string, args: Record<string, unknown>): Promise<T>;
}

function core(): TauriCore | null {
	return (globalThis as { __TAURI__?: { core?: TauriCore } }).__TAURI__?.core ?? null;
}

/** A name a file can take on any system: no path separators or reserved characters, not empty. */
export function safeFileName(name: string, fallback = 'plot'): string {
	const cleaned = name
		.replace(/[\\/:*?"<>|\u0000-\u001f]+/g, '-')
		.replace(/\s+/g, ' ')
		.trim()
		.slice(0, 120);
	return cleaned.replace(/^[.\s-]+/, '') || fallback;
}

function download(name: string, blob: Blob): void {
	const link = document.createElement('a');
	link.href = URL.createObjectURL(blob);
	link.download = name;
	link.click();
	setTimeout(() => URL.revokeObjectURL(link.href), 1000);
}

/**
 * Save `data` as `name` (its suffix included). In the desktop app: 'saved', 'cancelled', or an
 * error with the app's words; in a browser: 'downloaded'.
 */
export async function saveFile(name: string, data: Blob | string, filter: SaveFilter, title = 'Save'): Promise<'saved' | 'cancelled' | 'downloaded'> {
	const blob = typeof data === 'string' ? new Blob([data], { type: 'text/plain;charset=utf-8' }) : data;
	const app = core();
	if (app) {
		const contents = Array.from(new Uint8Array(await blob.arrayBuffer()));
		const path = await app.invoke<string | null>('save_file', { title, name, filterName: filter.name, extensions: filter.extensions, contents }).catch((e: unknown) => {
			throw new Error(String(e));
		});
		return path ? 'saved' : 'cancelled';
	}
	download(name, blob);
	return 'downloaded';
}
