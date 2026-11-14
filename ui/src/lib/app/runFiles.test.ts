/**
 * Run folders as files a person manages: the data folder read from the service in a browser and
 * from the app on the desktop, changed only there; Delete Run asking with both paths named before
 * the service deletes anything; and a run's `run.json` opening as a project does.
 */

import { describe, expect, it } from 'vitest';
import { RequestError, ServiceClient } from '$lib/protocol/client.svelte';
import { createAnalysisServer } from '$lib/protocol/mock/analysis';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { RunDeletePlan } from '$lib/protocol/types';
import { isProjectFile, openingOrder, runRecordFolder } from '$lib/project/files';
import { deleteLine, deleteQuestion, RunFiles, type DataFolderView, type DesktopRunFiles, type RunFilesHost } from './runFiles.svelte';

const FOLDER = '/data/Hertzforge_Data/20261114-120000_random_7';
const PLAN: RunDeletePlan = { folder: FOLDER, project_file: `${FOLDER}/20261114-120000_random_7.hforge`, project_file_exists: true, deleted: false };

function host(answers: { refuseAt?: 'plan' | 'delete'; confirm?: boolean; desktop?: DesktopRunFiles | null } = {}) {
	const calls: { method: string; params: Record<string, unknown> }[] = [];
	const asked: string[] = [];
	const h: RunFilesHost = {
		request: async <R,>(method: string, params: Record<string, unknown>): Promise<R> => {
			calls.push({ method, params });
			if (method === 'runs.data_folder') return { path: '/srv/runs', source: 'environment', exists: true, writable: true } as R;
			const dry = params.dry_run === true;
			if ((dry && answers.refuseAt === 'plan') || (!dry && answers.refuseAt === 'delete')) {
				throw new RequestError('refused', `the run in ${FOLDER} is not complete: it is still going, or it did not end cleanly, so it is not deleted`, method);
			}
			return { ...PLAN, deleted: !dry } as R;
		},
		desktop: answers.desktop ?? null,
		confirm: async (text) => {
			asked.push(text);
			return answers.confirm ?? true;
		}
	};
	return { h, calls, asked };
}

describe('Delete Run', () => {
	it('asks with both paths named, then the service deletes the run', async () => {
		const { h, calls, asked } = host();
		const files = new RunFiles(h);
		expect(await files.deleteRun(FOLDER)).toBe(true);
		expect(calls.map((c) => [c.method, c.params.dry_run ?? false])).toEqual([
			['runs.delete', true],
			['runs.delete', false]
		]);
		expect(asked).toHaveLength(1);
		expect(asked[0]).toContain(PLAN.folder);
		expect(asked[0]).toContain(PLAN.project_file);
		expect(files.at(FOLDER)).toBe('done');
		expect(deleteLine('done', null)).toBe('Deleted the run folder and its project file.');
	});

	it('deletes nothing when the person does not confirm', async () => {
		const { h, calls } = host({ confirm: false });
		const files = new RunFiles(h);
		expect(await files.deleteRun(FOLDER)).toBe(false);
		expect(calls.filter((c) => c.params.dry_run !== true)).toEqual([]);
		expect(files.at(FOLDER)).toBe('idle');
	});

	it('says the service refusal and never asks when the check refuses', async () => {
		const { h, calls, asked } = host({ refuseAt: 'plan' });
		const files = new RunFiles(h);
		expect(await files.deleteRun(FOLDER)).toBe(false);
		expect(asked).toEqual([]);
		expect(calls).toHaveLength(1);
		expect(files.at(FOLDER)).toBe('refused');
		expect(deleteLine('refused', files.refusal)).toMatch(/^The run was not deleted\. the run in .* is not complete/);
	});

	it('says a refusal at the delete itself', async () => {
		const { h } = host({ refuseAt: 'delete' });
		const files = new RunFiles(h);
		expect(await files.deleteRun(FOLDER)).toBe(false);
		expect(files.at(FOLDER)).toBe('refused');
		expect(files.refusal).toContain('not complete');
	});

	it('names the missing project file of a run never opened in Analysis', () => {
		const text = deleteQuestion({ ...PLAN, project_file_exists: false });
		expect(text).toContain(FOLDER);
		expect(text).toContain('has no project file');
		expect(text).toContain('cannot be undone');
	});
});

describe('the data folder', () => {
	it('is read from the analysis service in a browser and cannot be changed there', async () => {
		const { h, calls } = host();
		const files = new RunFiles(h);
		await files.loadDataFolder();
		expect(calls.map((c) => c.method)).toEqual(['runs.data_folder']);
		expect(files.dataFolder?.path).toBe('/srv/runs');
		expect(files.settable).toBe(false);
		expect(files.canShow).toBe(false);
		expect(await files.setDataFolder('/elsewhere')).toBe(false);
		expect(await files.showFolder(FOLDER)).toBe(false);
	});

	it('is the app\'s in the desktop app, and a refusal to change it is shown with its reason', async () => {
		const set: (string | null)[] = [];
		const shown: string[] = [];
		const view: DataFolderView = { path: '/home/me/Hertzforge_Data', source: 'default', settable: true, chosen: null, default: '/home/me/Hertzforge_Data' };
		const desktop: DesktopRunFiles = {
			dataFolder: async () => view,
			setDataFolder: async (path) => {
				set.push(path);
				if (path === '/locked') throw new Error('/locked cannot be written: Permission denied (os error 13)');
				return { ...view, source: 'default', chosen: path, next: path ?? undefined };
			},
			chooseFolder: async () => '/data/runs',
			showFolder: async (path) => {
				shown.push(path);
			}
		};
		const { h, calls } = host({ desktop });
		const files = new RunFiles(h);
		await files.loadDataFolder();
		expect(calls).toEqual([]);
		expect(files.settable).toBe(true);
		expect(await files.chooseDataFolder()).toBe(true);
		expect(files.dataFolder?.next).toBe('/data/runs');
		expect(await files.setDataFolder('/locked')).toBe(false);
		expect(files.dataError).toContain('cannot be written');
		expect(files.dataFolder?.next).toBe('/data/runs');
		expect(set).toEqual(['/data/runs', '/locked']);
		expect(await files.showFolder(FOLDER)).toBe(true);
		expect(shown).toEqual([FOLDER]);
	});

	it('is answered by the mock analysis service', async () => {
		const client = new ServiceClient({ service: 'analysis', url: 'ws://mock/', socketFactory: mockSocketFactory(createAnalysisServer(), 1) });
		client.connect();
		const files = new RunFiles({ request: (m, p) => client.request(m, p), desktop: null, confirm: async () => true });
		await files.loadDataFolder();
		expect(files.dataFolder).toMatchObject({ path: '~/Hertzforge_Data', source: 'default' });
		expect(await files.deleteRun(FOLDER)).toBe(false);
		expect(files.refusal).toContain('not inside the data folder');
		client.close();
	});
});

describe('opening a run', () => {
	it('opens a run folder by its run.json, as a project file opens', () => {
		expect(isProjectFile('/data/run/run.json')).toBe(true);
		expect(isProjectFile('C:\\data\\run\\run.json')).toBe(true);
		expect(isProjectFile('/data/run/setup.json')).toBe(false);
		expect(isProjectFile('/data/not-run.json')).toBe(false);
		expect(isProjectFile('/data/plate.hforge')).toBe(true);
		expect(openingOrder(['/a/x.unv', '/data/run/run.json'])).toEqual({ opens: '/data/run/run.json', imports: ['/a/x.unv'] });
	});

	it('offers a run record its folder only when one record is picked and its folder is there', () => {
		const objects = [
			{ name: 'Run Record', kind: 'run_record', run_folder: FOLDER },
			{ name: 'Run Record (2)', kind: 'run_record' },
			{ name: 'inputs', kind: 'time_history' }
		];
		expect(runRecordFolder(objects, ['Run Record'])).toBe(FOLDER);
		expect(runRecordFolder(objects, ['Run Record (2)'])).toBe(null);
		expect(runRecordFolder(objects, ['inputs'])).toBe(null);
		expect(runRecordFolder(objects, ['Run Record', 'inputs'])).toBe(null);
	});
});
