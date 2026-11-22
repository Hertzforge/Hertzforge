/**
 * Check for Updates, in the desktop app: the Tauri updater reads the latest release's manifest
 * (`latest.json`, written by the release workflow), says through the native dialogs whether a
 * newer version is published, and on a yes downloads the signed archive, verifies it against the
 * key built into the app, installs it and relaunches. Asked from the app menu or the palette,
 * never at startup. In a browser there is no updater and the command is not offered.
 *
 * From the yes to the relaunch the shell shows how far the update is (`updating`, drawn by
 * UpdateProgress): the bytes downloaded of the archive's size, then installing, then relaunching.
 */
import { updating } from './updating.svelte';

/** What the updater says while it downloads: the archive's size if the server gave one, each chunk, the end. */
export type DownloadEvent =
	| { event: 'Started'; data: { contentLength?: number | null } }
	| { event: 'Progress'; data: { chunkLength: number } }
	| { event: 'Finished'; data?: unknown };

/** How far an update is. `total` is null when the server gave no size, and the bar then has no fill to show. */
export interface UpdateProgress {
	version: string;
	phase: 'downloading' | 'installing' | 'relaunching';
	received: number;
	total: number | null;
}

/** The progress after one more word from the updater. */
export function advanced(now: UpdateProgress, said: DownloadEvent): UpdateProgress {
	if (said.event === 'Started') return { ...now, phase: 'downloading', received: 0, total: said.data.contentLength || null };
	if (said.event === 'Progress') return { ...now, received: now.received + said.data.chunkLength };
	return { ...now, phase: 'installing' };
}

const MB = 1024 * 1024;

/** The line under the bar. */
export function progressWords(now: UpdateProgress): string {
	if (now.phase === 'installing') return 'Installing...';
	if (now.phase === 'relaunching') return 'Relaunching...';
	const got = (now.received / MB).toFixed(1);
	return now.total ? `Downloading ${got} of ${(now.total / MB).toFixed(1)} MB` : now.received ? `Downloading ${got} MB` : 'Starting the download...';
}

/** The part of the bar to fill, 0 to 1, or null when there is no size to measure against. */
export function progressFraction(now: UpdateProgress): number | null {
	if (now.phase !== 'downloading') return 1;
	return now.total ? Math.min(1, now.received / now.total) : null;
}

interface Update {
	version: string;
	currentVersion: string;
	body?: string | null;
	downloadAndInstall(onEvent?: (said: DownloadEvent) => void): Promise<void>;
}

interface TauriUpdates {
	event?: { listen(name: string, handler: () => void): Promise<() => void> };
	updater?: { check(): Promise<Update | null> };
	process?: { relaunch(): Promise<void> };
	dialog?: {
		message(text: string, options?: { title?: string; kind?: 'info' | 'warning' | 'error' }): Promise<void>;
		ask(text: string, options?: { title?: string; kind?: 'info' | 'warning' | 'error'; okLabel?: string; cancelLabel?: string }): Promise<boolean>;
	};
}

function tauri(): TauriUpdates | null {
	return (globalThis as { __TAURI__?: TauriUpdates }).__TAURI__ ?? null;
}

/** Whether the desktop app's updater is here to ask. */
export function hasUpdater(): boolean {
	return Boolean(tauri()?.updater);
}

const TITLE = 'Check for Updates';

/** Ask the updater, tell the person what it found, and install on their yes. Returns what was said, for the log. */
export async function checkForUpdates(): Promise<string> {
	const t = tauri();
	if (!t?.updater) return 'No updater outside the desktop app.';
	const say = (text: string, kind: 'info' | 'warning' | 'error' = 'info') => t.dialog?.message(text, { title: TITLE, kind }) ?? Promise.resolve();
	let update: Update | null;
	try {
		update = await t.updater.check();
	} catch (err) {
		const why = err instanceof Error ? err.message : String(err);
		const said = /json|404|not found/i.test(why) ? 'No update is published yet.' : `Could not check for updates: ${why}`;
		await say(said, 'warning');
		return said;
	}
	if (!update) {
		const said = 'This is the latest version of Hertzforge.';
		await say(said);
		return said;
	}
	const wanted = (await t.dialog?.ask(`Hertzforge ${update.version} is available; this is ${update.currentVersion}. Install it and relaunch?`, { title: TITLE, okLabel: 'Install', cancelLabel: 'Not Now' })) ?? false;
	if (!wanted) return `Hertzforge ${update.version} is available; not installed.`;
	let now: UpdateProgress = { version: update.version, phase: 'downloading', received: 0, total: null };
	updating.now = now;
	try {
		await update.downloadAndInstall((said) => (updating.now = now = advanced(now, said)));
	} catch (err) {
		updating.now = null;
		const why = err instanceof Error ? err.message : String(err);
		await say(`The update did not install: ${why}`, 'error');
		return `The update did not install: ${why}`;
	}
	updating.now = { ...now, phase: 'relaunching' };
	try {
		await t.process?.relaunch();
	} finally {
		updating.now = null;
	}
	return `Hertzforge ${update.version} installed.`;
}

/** The app menu's Check for Updates (the macOS desktop app) runs the check; returns how to stop listening. */
export function listenForUpdateMenu(check: () => unknown = checkForUpdates): () => void {
	const events = tauri()?.event;
	if (!events) return () => {};
	let stop: (() => void) | null = null;
	let stopped = false;
	void events
		.listen('update-menu', () => void check())
		.then((unlisten) => (stopped ? unlisten() : (stop = unlisten)));
	return () => {
		stopped = true;
		stop?.();
	};
}
