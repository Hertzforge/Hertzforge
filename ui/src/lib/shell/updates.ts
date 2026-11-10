/**
 * Check for Updates, in the desktop app: the Tauri updater reads the latest release's manifest
 * (`latest.json`, written by the release workflow), says through the native dialogs whether a
 * newer version is published, and on a yes downloads the signed archive, verifies it against the
 * key built into the app, installs it and relaunches. Asked from the app menu or the palette,
 * never at startup. In a browser there is no updater and the command is not offered.
 */

interface Update {
	version: string;
	currentVersion: string;
	body?: string | null;
	downloadAndInstall(): Promise<void>;
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
	try {
		await update.downloadAndInstall();
	} catch (err) {
		const why = err instanceof Error ? err.message : String(err);
		await say(`The update did not install: ${why}`, 'error');
		return `The update did not install: ${why}`;
	}
	await t.process?.relaunch();
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
