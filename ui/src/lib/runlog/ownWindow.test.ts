import { afterEach, describe, expect, it, vi } from 'vitest';
import { followAttributes } from './ownWindow';

class Attrs {
	held = new Map<string, string>();
	get attributes() {
		return [...this.held].map(([name, value]) => ({ name, value }));
	}
	hasAttribute(name: string) {
		return this.held.has(name);
	}
	setAttribute(name: string, value: string) {
		this.held.set(name, value);
	}
	removeAttribute(name: string) {
		this.held.delete(name);
	}
}

describe("the log window's own document", () => {
	afterEach(() => void vi.unstubAllGlobals());

	it('follows the main window as the theme changes, to the system theme too, until told to stop', () => {
		let changed = () => {};
		const disconnect = vi.fn();
		vi.stubGlobal(
			'MutationObserver',
			class {
				constructor(callback: () => void) {
					changed = callback;
				}
				observe() {}
				disconnect = disconnect;
			}
		);
		const main = new Attrs();
		const own = new Attrs();
		main.setAttribute('data-theme', 'dark');
		const stop = followAttributes(main as never, own as never);
		expect(own.held.get('data-theme')).toBe('dark');
		main.setAttribute('data-theme', 'light');
		changed();
		expect(own.held.get('data-theme')).toBe('light');
		main.removeAttribute('data-theme');
		changed();
		expect(own.hasAttribute('data-theme')).toBe(false);
		stop();
		expect(disconnect).toHaveBeenCalled();
	});
});
