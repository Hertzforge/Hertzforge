import { afterEach, describe, expect, it } from 'vitest';
import { advanced, checkForUpdates, listenForUpdateMenu, progressFraction, progressWords, type DownloadEvent, type UpdateProgress } from './updates';
import { updating } from './updating.svelte';

type Handler = () => void;

function desktop() {
	const handlers = new Map<string, Handler>();
	let unlistened = 0;
	(globalThis as { __TAURI__?: unknown }).__TAURI__ = {
		event: {
			listen: (name: string, handler: Handler) => {
				handlers.set(name, handler);
				return Promise.resolve(() => {
					unlistened += 1;
					handlers.delete(name);
				});
			}
		}
	};
	return { handlers, unlistened: () => unlistened };
}

afterEach(() => {
	delete (globalThis as { __TAURI__?: unknown }).__TAURI__;
});

describe('the app menu\'s Check for Updates', () => {
	it('runs the check each time the menu item is chosen', async () => {
		const app = desktop();
		let checks = 0;
		const stop = listenForUpdateMenu(() => (checks += 1));
		await Promise.resolve();
		app.handlers.get('update-menu')?.();
		app.handlers.get('update-menu')?.();
		expect(checks).toBe(2);
		stop();
		expect(app.unlistened()).toBe(1);
	});

	it('stops listening when stopped before the listener was in place', async () => {
		const app = desktop();
		const stop = listenForUpdateMenu(() => undefined);
		stop();
		await Promise.resolve();
		await Promise.resolve();
		expect(app.unlistened()).toBe(1);
		expect(app.handlers.has('update-menu')).toBe(false);
	});

	it('does nothing in a browser', () => {
		const stop = listenForUpdateMenu(() => {
			throw new Error('no menu in a browser');
		});
		expect(() => stop()).not.toThrow();
	});
});

describe('an update on its way in', () => {
	const MB = 1024 * 1024;
	const start: UpdateProgress = { version: '9.9.9', phase: 'downloading', received: 0, total: null };

	it('counts the bytes against the size the server gave', () => {
		let now = advanced(start, { event: 'Started', data: { contentLength: 80 * MB } });
		expect(progressFraction(now)).toBe(0);
		now = advanced(now, { event: 'Progress', data: { chunkLength: 15 * MB } });
		now = advanced(now, { event: 'Progress', data: { chunkLength: 5 * MB } });
		expect(progressFraction(now)).toBe(0.25);
		expect(progressWords(now)).toBe('Downloading 20.0 of 80.0 MB');
		now = advanced(now, { event: 'Finished' });
		expect(now.phase).toBe('installing');
		expect(progressFraction(now)).toBe(1);
		expect(progressWords(now)).toBe('Installing...');
	});

	it('has no fill to show without a size, and says what has come', () => {
		let now = advanced(start, { event: 'Started', data: {} });
		expect(progressFraction(now)).toBeNull();
		expect(progressWords(now)).toBe('Starting the download...');
		now = advanced(now, { event: 'Progress', data: { chunkLength: 3 * MB } });
		expect(progressFraction(now)).toBeNull();
		expect(progressWords(now)).toBe('Downloading 3.0 MB');
	});

	function updater(install: (onEvent?: (said: DownloadEvent) => void) => Promise<void>) {
		const seen: (UpdateProgress | null)[] = [];
		const said: string[] = [];
		(globalThis as { __TAURI__?: unknown }).__TAURI__ = {
			updater: { check: () => Promise.resolve({ version: '9.9.9', currentVersion: '9.9.8', downloadAndInstall: install }) },
			dialog: { ask: () => Promise.resolve(true), message: (text: string) => (said.push(text), Promise.resolve()) },
			process: { relaunch: () => (seen.push(updating.now), Promise.resolve()) }
		};
		return { seen, said };
	}

	it('shows the download, the install and the relaunch, and nothing after', async () => {
		const app = updater(async (onEvent) => {
			app.seen.push(updating.now);
			onEvent?.({ event: 'Started', data: { contentLength: 10 * MB } });
			onEvent?.({ event: 'Progress', data: { chunkLength: 4 * MB } });
			app.seen.push(updating.now);
			onEvent?.({ event: 'Finished' });
			app.seen.push(updating.now);
		});
		expect(await checkForUpdates()).toBe('Hertzforge 9.9.9 installed.');
		expect(app.seen.map((now) => now?.phase)).toEqual(['downloading', 'downloading', 'installing', 'relaunching']);
		expect(app.seen[0]).toMatchObject({ version: '9.9.9', received: 0, total: null });
		expect(app.seen[1]).toMatchObject({ received: 4 * MB, total: 10 * MB });
		expect(updating.now).toBeNull();
	});

	it('takes the bar down when the update does not install', async () => {
		const app = updater(() => Promise.reject(new Error('signature mismatch')));
		expect(await checkForUpdates()).toBe('The update did not install: signature mismatch');
		expect(app.said).toEqual(['The update did not install: signature mismatch']);
		expect(updating.now).toBeNull();
	});
});
