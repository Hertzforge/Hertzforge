import { afterEach, describe, expect, it, vi } from 'vitest';
import { baseName, canDragOut, chooseFiles, chooseObjectFile, discardUnsaved, fileNameOf, formatForPath, hasNativeFiles, isProjectFile, listenForDrops, startDragOut } from './files';

const g = globalThis as Record<string, unknown>;

afterEach(() => {
	delete g.__TAURI__;
	delete g.window;
});

describe('which files open and which import', () => {
	it('opens project formats and imports everything else', () => {
		for (const path of ['/a/run.hforge', 'C:\\tests\\Old.VDYN', '/a/b.mat', '/a/b.h5', '/a/b.hdf5', '/a/b.escdf']) {
			expect(isProjectFile(path), path).toBe(true);
		}
		for (const path of ['/a/modal.nc4', '/a/geometry.unv', '/a/hforge', '/a/run.hforge.bak']) {
			expect(isProjectFile(path), path).toBe(false);
		}
	});

	it('names a file by the last part of its path', () => {
		expect(baseName('/Users/someone/tests/modal.nc4')).toBe('modal.nc4');
		expect(baseName('C:\\tests\\run.hforge')).toBe('run.hforge');
	});
});

describe('in the desktop app', () => {
	it('picks files in the native dialog, one or many', async () => {
		const open = vi.fn().mockResolvedValueOnce('/a/run.hforge').mockResolvedValueOnce(['/a/x.unv', '/a/y.nc4']).mockResolvedValueOnce(null);
		g.__TAURI__ = { dialog: { open, save: vi.fn() } };
		expect(hasNativeFiles()).toBe(true);
		expect(await chooseFiles('open')).toEqual(['/a/run.hforge']);
		expect(open.mock.calls[0][0].multiple).toBe(false);
		expect(await chooseFiles('import')).toEqual(['/a/x.unv', '/a/y.nc4']);
		expect(await chooseFiles('import')).toBeNull();
	});

	it('hands over the paths of files dropped on the window, and stops on request', async () => {
		let handler: ((e: { payload: { type: string; paths?: string[] } }) => void) | null = null;
		const unlisten = vi.fn();
		g.__TAURI__ = {
			webview: {
				getCurrentWebview: () => ({
					onDragDropEvent: (h: typeof handler) => {
						handler = h;
						return Promise.resolve(unlisten);
					}
				})
			}
		};
		const got: (string[] | null)[] = [];
		const stop = listenForDrops((paths) => got.push(paths));
		await Promise.resolve();
		handler!({ payload: { type: 'over' } });
		handler!({ payload: { type: 'drop', paths: ['/a/modal.nc4'] } });
		expect(got).toEqual([['/a/modal.nc4']]);
		stop();
		expect(unlisten).toHaveBeenCalled();
	});
});

describe('in a browser', () => {
	it('has no dialog, and a drop of files says it has no paths', async () => {
		expect(hasNativeFiles()).toBe(false);
		expect(await chooseFiles('open')).toBeNull();
		const target = new EventTarget();
		g.window = target;
		const got: (string[] | null)[] = [];
		const stop = listenForDrops((paths) => got.push(paths));
		const drop = (types: string[]) => {
			const e = new Event('drop', { cancelable: true }) as Event & { dataTransfer: { types: string[] } };
			e.dataTransfer = { types };
			target.dispatchEvent(e);
			return e;
		};
		expect(drop(['text/plain']).defaultPrevented).toBe(false);
		expect(got).toEqual([]);
		expect(drop(['Files']).defaultPrevented).toBe(true);
		expect(got).toEqual([null]);
		stop();
		drop(['Files']);
		expect(got).toEqual([null]);
	});
});

describe('the save window for an object', () => {
	const formats = [
		{ value: 'hforge', label: 'Hertzforge object (.hforge)', suffix: '.hforge' },
		{ value: 'unv', label: 'Universal file (.unv)', suffix: '.unv' },
		{ value: 'unv_binary', label: 'Universal file, binary 58b (.unv)', suffix: '.unv' },
		{ value: 'photos', label: 'Photographs (a folder of images)', suffix: '' }
	];

	it('takes the format from the suffix, the first of two sharing it, or else the first of all', () => {
		expect(formatForPath('/a/b.UNV', formats)).toBe('unv');
		expect(formatForPath('/a/b.hforge', formats)).toBe('hforge');
		expect(formatForPath('/a/b', formats)).toBe('hforge');
	});

	it('names the file for the object without the characters a path gives a meaning to', () => {
		expect(fileNameOf('Run 3/a:b\\c')).toBe('Run 3-a-b-c');
	});

	it('lists the formats that have a suffix as the file types where the plugin draws the window', async () => {
		const save = vi.fn(async () => '/a/plate.unv');
		g.__TAURI__ = { dialog: { save } };
		await expect(chooseObjectFile('Plate', formats)).resolves.toEqual({ path: '/a/plate.unv', format: 'unv' });
		expect(save).toHaveBeenCalledWith({
			title: 'Export Plate',
			defaultPath: 'Plate.hforge',
			filters: [
				{ name: 'Hertzforge object (.hforge)', extensions: ['hforge'] },
				{ name: 'Universal file (.unv)', extensions: ['unv'] },
				{ name: 'Universal file, binary 58b (.unv)', extensions: ['unv'] }
			]
		});
	});

	it('answers null when cancelled and when there is no native window', async () => {
		await expect(chooseObjectFile('Plate', formats)).resolves.toBeNull();
		g.__TAURI__ = { dialog: { save: vi.fn(async () => null) } };
		await expect(chooseObjectFile('Plate', formats)).resolves.toBeNull();
	});
});

describe('letting unsaved changes go', () => {
	it('asks in the native dialog in the desktop app, and takes its answer', async () => {
		const asked: { text: string; options: unknown }[] = [];
		g.__TAURI__ = {
			dialog: {
				ask: (text: string, options: unknown) => {
					asked.push({ text, options });
					return Promise.resolve(false);
				}
			}
		};
		expect(await discardUnsaved('Plate')).toBe(false);
		expect(asked[0].text).toBe('Plate has unsaved changes. Discard them?');
		expect(asked[0].options).toMatchObject({ okLabel: 'Discard', cancelLabel: 'Cancel' });
	});

	it("asks with the browser's own question elsewhere", async () => {
		const confirm = vi.fn(() => true);
		vi.stubGlobal('window', { confirm });
		expect(await discardUnsaved('Plate')).toBe(true);
		expect(confirm).toHaveBeenCalledWith('Plate has unsaved changes. Discard them?');
		vi.unstubAllGlobals();
	});
});

describe('dragging out of the window', () => {
	// Brandon, 2026-11-11: the project dragged to a folder is saved there
	class Channel {
		onmessage: (message: unknown) => void = () => {};
	}

	it('hands the system the files the service wrote, as a copy, with a picture', async () => {
		const invoke = vi.fn(async () => undefined);
		g.__TAURI__ = { core: { invoke, Channel }, dialog: {} };
		expect(canDragOut()).toBe(true);
		expect(await startDragOut(['/tmp/hertzforge-drag-x/a/Project.hforge'], 'Project.hforge')).toBe(true);
		const [command, args] = invoke.mock.calls[0] as unknown as [string, Record<string, unknown>];
		expect(command).toBe('plugin:drag|start_drag');
		expect(args.item).toEqual(['/tmp/hertzforge-drag-x/a/Project.hforge']);
		expect(args.options).toEqual({ mode: 'copy' });
		expect(String(args.image)).toMatch(/^data:image\/png;base64,/);
		expect(args.onEvent).toBeInstanceOf(Channel);
	});

	it('is not offered in a browser', async () => {
		expect(canDragOut()).toBe(false);
		expect(await startDragOut(['/x.hforge'], 'x')).toBe(false);
	});

	it('a drag let go over the window itself imports nothing, and other files still do', async () => {
		let handler: ((e: { payload: { type: string; paths?: string[] } }) => void) | null = null;
		g.__TAURI__ = {
			core: { invoke: vi.fn(async () => undefined), Channel },
			dialog: {},
			webview: {
				getCurrentWebview: () => ({
					onDragDropEvent: (h: typeof handler) => {
						handler = h;
						return Promise.resolve(vi.fn());
					}
				})
			}
		};
		await startDragOut(['/tmp/hertzforge-drag-x/b/Run.hforge'], 'Run.hforge');
		const got: (string[] | null)[] = [];
		listenForDrops((paths) => got.push(paths));
		await Promise.resolve();
		handler!({ payload: { type: 'drop', paths: ['/tmp/hertzforge-drag-x/b/Run.hforge'] } });
		handler!({ payload: { type: 'drop', paths: ['/tmp/hertzforge-drag-x/b/Run.hforge', '/a/modal.nc4'] } });
		expect(got).toEqual([['/a/modal.nc4']]);
	});
});
