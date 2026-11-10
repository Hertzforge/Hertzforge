import { afterEach, describe, expect, it, vi } from 'vitest';
import { safeFileName, saveFile } from './saveFile';

describe('file names', () => {
	it('drops the characters a path gives a meaning to', () => {
		expect(safeFileName('Control: run 3/4')).toBe('Control- run 3-4');
		expect(safeFileName('  a   b  ')).toBe('a b');
		expect(safeFileName('../secret')).toBe('secret');
		expect(safeFileName('', 'plot')).toBe('plot');
	});
});

describe('saving in the desktop app', () => {
	afterEach(() => {
		delete (globalThis as { __TAURI__?: unknown }).__TAURI__;
	});

	it('hands the app the name, the file type and the bytes, and says whether it was saved', async () => {
		const invoke = vi.fn().mockResolvedValueOnce('/Users/me/Control.csv').mockResolvedValueOnce(null);
		(globalThis as { __TAURI__?: unknown }).__TAURI__ = { core: { invoke } };
		expect(await saveFile('Control.csv', 'a,b\n', { name: 'CSV', extensions: ['csv'] }, 'Save CSV')).toBe('saved');
		expect(invoke).toHaveBeenCalledWith('save_file', { title: 'Save CSV', name: 'Control.csv', filterName: 'CSV', extensions: ['csv'], contents: [97, 44, 98, 10] });
		expect(await saveFile('Control.csv', 'x', { name: 'CSV', extensions: ['csv'] })).toBe('cancelled');
	});

	it('passes on the app\'s words when it cannot write', async () => {
		(globalThis as { __TAURI__?: unknown }).__TAURI__ = { core: { invoke: vi.fn().mockRejectedValue('/locked/Control.csv: Permission denied') } };
		await expect(saveFile('Control.csv', 'x', { name: 'CSV', extensions: ['csv'] })).rejects.toThrow('Permission denied');
	});
});
