import { describe, expect, it } from 'vitest';
import { actFor, byDate, keyName, notices, sinceWords, span, statusWords } from './words';
import type { RunLogRow, RunLogView } from './types';

describe('the status strip', () => {
	it('reads the test, the place and the state', () => {
		expect(statusWords(undefined).test).toBe('No log open');
		expect(statusWords({ test: null }).test).toBe('No test open');
		const words = statusWords({ test: 2, title: 'Z random', kind: 'Random Vibration', axis: 'Z', run: 3, level_db: -6, outcome: 'running' });
		expect(words).toEqual({ test: 'Test 2: Z random (Random Vibration)', place: 'axis Z, run 3, -6 dB', state: 'RUNNING' });
		expect(statusWords({ test: 1, title: 't', kind: 'k', level_db: 0, outcome: 'aborted' }).place).toBe('no axis, full level');
		expect(statusWords({ test: 1, title: 't', kind: 'k', outcome: 'aborted' }).state).toBe('STOPPED: aborted');
	});

	it('counts the time since start, and how long a stopped run ran', () => {
		expect(span(65)).toBe('1:05');
		expect(span(3725)).toBe('1:02:05');
		const started = '2026-11-03T10:00:00Z';
		expect(sinceWords({ test: 1, started }, Date.parse('2026-11-03T10:01:30Z'))).toBe('1:30 since start');
		expect(sinceWords({ test: 1, started, stopped: '2026-11-03T10:10:00Z' }, 0)).toBe('stopped after 10:00');
		expect(sinceWords({ test: 1 }, 0)).toBe('');
	});

	it('notes a log read-only and lines not read', () => {
		const log = { writable: false, read_only: 'held by KC', mirror_problem: '', problems: ['a', 'b'] } as unknown as RunLogView;
		expect(notices(log)).toEqual(['Read-only: held by KC', '2 lines not read or not fitting: a; b']);
	});
});

describe('the day', () => {
	const row = (id: number, date: string, test: number | null): RunLogRow => ({ id, kind: 'note', test, date, line: '', corrections: [], late: null });
	it('groups a test\'s rows under their dates', () => {
		const rows = [row(1, '2026-11-02', 1), row(2, '2026-11-02', 2), row(3, '2026-11-03', 1)];
		expect(byDate(rows, null).map((d) => [d.date, d.rows.map((r) => r.id)])).toEqual([
			['2026-11-02', [1, 2]],
			['2026-11-03', [3]]
		]);
		expect(byDate(rows, 2).map((d) => d.rows.map((r) => r.id))).toEqual([[2]]);
	});
});

describe('the keys', () => {
	const press = (key: string, more: Partial<{ shiftKey: boolean; metaKey: boolean; ctrlKey: boolean; altKey: boolean }> = {}) => ({
		key,
		shiftKey: true,
		metaKey: false,
		ctrlKey: false,
		altKey: false,
		...more
	});
	it('takes Shift and the platform\'s command key with an act\'s letter', () => {
		expect(actFor(press('a', { metaKey: true }), true)).toBe('abort');
		expect(actFor(press('A', { ctrlKey: true }), false)).toBe('abort');
		expect(actFor(press('a', { ctrlKey: true }), true)).toBeNull();
		expect(actFor(press('a', { metaKey: true, shiftKey: false }), true)).toBeNull();
		expect(actFor(press('q', { metaKey: true }), true)).toBeNull();
		expect(keyName('note', true)).toBe('Cmd+Shift+N');
	});
});
