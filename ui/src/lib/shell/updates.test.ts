import { afterEach, describe, expect, it } from 'vitest';
import { listenForUpdateMenu } from './updates';

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
