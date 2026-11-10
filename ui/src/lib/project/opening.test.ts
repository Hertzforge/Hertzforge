import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ProjectState, type AppState } from '$lib/app/state.svelte';
import { askToReplace, openingOrder } from './files';

const g = globalThis as Record<string, unknown>;

function summary(modified: boolean) {
	return { title: 'Survey', file: null, modified, objects: [], groups: [] };
}

function state(modified: boolean | null) {
	const request = vi.fn(async (_method: string, _params: Record<string, unknown>) => ({ project: summary(false) }));
	const app = { analysis: { request, on: vi.fn() }, analyze: { selectProject: vi.fn() }, workspace: 'signals' };
	const project = new ProjectState(app as unknown as AppState);
	project.apply = vi.fn();
	if (modified !== null) project.summary = summary(modified) as unknown as ProjectState['summary'];
	return { project, sent: () => request.mock.calls.map(([method, params]) => [method, params.path]) };
}

afterEach(() => {
	delete g.__TAURI__;
	vi.unstubAllGlobals();
});

describe('which of several paths opens', () => {
	it('opens the first project file, whatever came before it, and imports the rest', () => {
		expect(openingOrder(['/a/x.unv', '/a/one.hforge', '/a/two.hforge'])).toEqual({
			opens: '/a/one.hforge',
			imports: ['/a/x.unv', '/a/two.hforge']
		});
		expect(openingOrder(['/a/x.unv'])).toEqual({ opens: null, imports: ['/a/x.unv'] });
	});
});

describe('asking before a project is replaced', () => {
	it('asks in the native dialog in the desktop app', async () => {
		const ask = vi.fn().mockResolvedValue(true);
		g.__TAURI__ = { dialog: { ask, open: vi.fn(), save: vi.fn() } };
		expect(await askToReplace('Survey', 'other.hforge')).toBe(true);
		expect(ask.mock.calls[0][0]).toContain('Survey has unsaved changes');
	});

	it('asks with the browser\'s own confirm otherwise', async () => {
		const confirm = vi.fn().mockReturnValue(false);
		vi.stubGlobal('confirm', confirm);
		expect(await askToReplace('Survey', 'other.hforge')).toBe(false);
		expect(confirm).toHaveBeenCalledOnce();
	});
});

describe('opening over the open project', () => {
	beforeEach(() => {
		vi.stubGlobal('confirm', vi.fn().mockReturnValue(false));
	});

	it('keeps a modified project when the person says so, and sends nothing', async () => {
		const { project, sent } = state(true);
		expect(await project.open('/a/other.hforge')).toBe(false);
		expect(sent()).toEqual([]);
	});

	it('replaces a modified project when the person says to', async () => {
		vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
		const { project, sent } = state(true);
		expect(await project.open('/a/other.hforge')).toBe(true);
		expect(sent()).toEqual([['project.open', '/a/other.hforge']]);
	});

	it('does not ask when nothing would be lost', async () => {
		for (const modified of [false, null]) {
			const { project, sent } = state(modified);
			await project.open('/a/other.hforge');
			expect(sent()).toEqual([['project.open', '/a/other.hforge']]);
		}
		expect(globalThis.confirm).not.toHaveBeenCalled();
	});

	it('imports the files dropped beside a project into it, after it opens', async () => {
		const { project, sent } = state(false);
		await project.openPaths(['/a/x.unv', '/a/one.hforge', '/a/two.hforge']);
		expect(sent()).toEqual([
			['project.open', '/a/one.hforge'],
			['project.import', '/a/x.unv'],
			['project.import', '/a/two.hforge']
		]);
	});

	it('imports nothing when the person keeps working', async () => {
		const { project, sent } = state(true);
		await project.openPaths(['/a/one.hforge', '/a/x.unv']);
		expect(sent()).toEqual([]);
	});
});

describe('File > Import', () => {
	it('adds a project file to the open project instead of replacing it', async () => {
		const open = vi.fn().mockResolvedValue(['/a/other.hforge']);
		g.__TAURI__ = { dialog: { open, save: vi.fn() } };
		const { project, sent } = state(true);
		await project.fileAct('import');
		expect(sent()).toEqual([['project.import', '/a/other.hforge']]);
	});
});
