import { describe, expect, it } from 'vitest';
import { CursorHub, type CursorState } from './cursor';

describe('CursorHub', () => {
	it('shares a cursor across panes in one link group', () => {
		const hub = new CursorHub();
		const a: CursorState[] = [];
		const b: CursorState[] = [];
		hub.subscribe('frequency', (s) => a.push(s));
		hub.subscribe('frequency', (s) => b.push(s));
		hub.set('frequency', 412.5, 'pane-a');
		expect(a.at(-1)).toEqual({ x: 412.5, source: 'pane-a' });
		expect(b.at(-1)).toEqual({ x: 412.5, source: 'pane-a' });
	});

	it('keeps groups apart', () => {
		const hub = new CursorHub();
		const other: CursorState[] = [];
		hub.subscribe('time', (s) => other.push(s));
		hub.set('frequency', 100, 'p');
		expect(other).toEqual([{ x: null, source: null }]);
		expect(hub.get('time').x).toBeNull();
	});

	it('clears only when the pane that owns the cursor leaves', () => {
		const hub = new CursorHub();
		hub.subscribe('f', () => {});
		hub.set('f', 50, 'a');
		hub.set('f', 60, 'b');
		hub.clear('f', 'a');
		expect(hub.get('f')).toEqual({ x: 60, source: 'b' });
		hub.clear('f', 'b');
		expect(hub.get('f')).toEqual({ x: null, source: null });
	});

	it('does not re-notify for an unchanged position, and treats non-finite x as leaving', () => {
		const hub = new CursorHub();
		let calls = 0;
		hub.subscribe('f', () => calls++);
		hub.set('f', 10, 'a');
		hub.set('f', 10, 'a');
		expect(calls).toBe(2); // initial state, then one move
		hub.set('f', Number.NaN, 'a');
		expect(hub.get('f').x).toBeNull();
	});

	it('forgets a group once its last subscriber leaves', () => {
		const hub = new CursorHub();
		const off = hub.subscribe('f', () => {});
		hub.set('f', 5, 'a');
		off();
		expect(hub.get('f')).toEqual({ x: null, source: null });
	});
});
