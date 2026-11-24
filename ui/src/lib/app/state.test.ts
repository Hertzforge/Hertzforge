import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AppState } from '$lib/app/state.svelte';
import { readConfig } from '$lib/app/config';
import { RequestError } from '$lib/protocol/client.svelte';
import type { ActionColumn, ActionRow, OperationDescriptor, ProjectSummary } from '$lib/protocol/types';

// The real state class with the service stubbed. `followPreview()` is called by hand where the
// `$effect` in Analyze.svelte would run it again: what it does given the state is proved here,
// that the effect runs it when the state changes is not.

const geometry = { name: 'geometry', label: 'Geometry', type: 'enum', options_from: 'objects:geometry' };
const addBlock = {
	id: 'project.add_solid_block',
	title: 'Add Block',
	workspace: 'geometry',
	modules: ['geometry'],
	previews: true,
	outputs: ['reading'],
	params: [geometry, { name: 'x', label: 'x', type: 'number', default: 0 }]
} as unknown as OperationDescriptor;
const editSolid = {
	id: 'project.edit_solid',
	title: 'Edit Solid',
	workspace: 'geometry',
	modules: ['geometry'],
	previews: true,
	outputs: ['reading'],
	params: [geometry, { name: 'solid', label: 'Solid', type: 'integer', default: 1 }, { name: 'length', label: 'Length', type: 'string', default: '' }]
} as unknown as OperationDescriptor;

const summary = {
	title: 't',
	file: null,
	modified: false,
	active_geometry: 'G',
	links: [],
	unresolved: [],
	objects: [
		{ name: 'G', kind: 'geometry', records: 0, groups: [], verbs: [] },
		{ name: 'H', kind: 'geometry', records: 0, groups: [], verbs: [] }
	]
} as unknown as ProjectSummary;

type Sent = { op?: string; params?: Record<string, unknown>; preview?: boolean };

function make() {
	const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=ws://127.0.0.1:1'));
	const runs: Sent[] = [];
	const answers = { refuse: '' };
	const request = vi.fn(async (method: string, params: Sent) => {
		if (method === 'op.run') {
			runs.push(params);
			if (answers.refuse) throw new RequestError('refused', answers.refuse, method);
			if (params.op === editSolid.id && !params.params?.length) throw new RequestError('refused', 'type what changes', method);
			return { views: [{ view: 'reading', title: params.preview ? 'PREVIEW' : 'REAL', rows: [] }], project: params.preview ? undefined : summary };
		}
		if (method === 'project.actions') return { table: { columns: [{ kinds: ['geometry'], current: true, rows: [] }] } };
		if (method === 'project.show') return { title: 'shown', views: [], note: '' };
		return {};
	});
	(app.analysis as unknown as { request: unknown }).request = request;
	app.catalog = { operations: [addBlock, editSolid] } as never;
	app.project.summary = summary;
	return { app, a: app.analyze, runs, answers };
}

const row = (op: OperationDescriptor) => ({ id: op.id, verb: op.title, kind: 'run', ready: true, fill: { geometry: 'G' } }) as unknown as ActionRow;
const column = { kinds: ['geometry'], current: true, rows: [] } as unknown as ActionColumn;
const said = (a: ReturnType<typeof make>['a']) => (a.reading?.views[0] as { title?: string } | undefined)?.title;
const rest = () => vi.advanceTimersByTimeAsync(400);

describe('a form that previews', () => {
	beforeEach(() => void vi.useFakeTimers());
	afterEach(() => void vi.useRealTimers());

	it('draws what it would do once the typing rests', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		expect(a.reading?.title).toBe('Add Block, preview');
		expect(said(a)).toBe('PREVIEW');
	});

	it('runs nothing once the form is closed, whatever the selection does', async () => {
		const { app, a, runs } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.closeForm();
		const before = runs.length;
		app.project.clickObject('H', ['G', 'H']);
		await vi.advanceTimersByTimeAsync(0);
		a.followPreview();
		await rest();
		expect(a.formOpen).toBe(false);
		expect(runs.length).toBe(before);
		expect(a.reading).toBeNull();
	});

	it('keeps its preview on the stage while a new pick is taken, and drops it with the form', async () => {
		const { app, a } = make();
		const stub = app.analysis as unknown as { request: (method: string, params: Sent) => Promise<unknown> };
		const answer = stub.request;
		let offered = true;
		Object.defineProperty(app.analysis, 'isOpen', { get: () => true });
		stub.request = async (method, params) =>
			method === 'project.actions' ? { table: { columns: [{ ...column, rows: offered ? [row(addBlock)] : [] }] } } : answer(method, params);
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		const drawn = a.reading;
		expect(said(a)).toBe('PREVIEW');
		await a.showSelection();
		await vi.advanceTimersByTimeAsync(0);
		expect(a.formOpen).toBe(true);
		expect(a.reading).toBe(drawn);
		offered = false;
		await a.showSelection();
		await vi.advanceTimersByTimeAsync(0);
		expect(a.formOpen).toBe(false);
		expect(a.reading).toBeNull();
	});

	it('Back takes a preview off the stage', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.closeForm();
		expect(a.reading).toBeNull();
		expect(a.readingId).toBeNull();
	});

	it('Back leaves a reading that ran', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		await a.run();
		expect(said(a)).toBe('REAL');
		a.closeForm();
		expect(said(a)).toBe('REAL');
	});

	it('Back before the typing rests: nothing goes out', async () => {
		const { a, runs } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await vi.advanceTimersByTimeAsync(50);
		a.closeForm();
		await rest();
		expect(runs.length).toBe(0);
		expect(a.reading).toBeNull();
	});

	it('opened again, it draws again', async () => {
		const { a } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.closeForm();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		expect(said(a)).toBe('PREVIEW');
	});

	it('a Run inside the debounce is not overwritten by its own preview', async () => {
		const { a, runs } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		a.values = { ...a.values, [addBlock.id]: { ...a.currentValues, x: 5 } };
		a.followPreview();
		await vi.advanceTimersByTimeAsync(50);
		await a.run();
		a.followPreview();
		await rest();
		expect(said(a)).toBe('REAL');
		expect(runs.map((r) => r.preview ?? false)).toEqual([true, false]);
	});

	it('opening a form that waits for typing says nothing', async () => {
		const { a, runs } = make();
		await a.perform(row(editSolid), column);
		a.followPreview();
		await rest();
		expect(runs.length).toBe(1);
		expect(a.runError).toBeNull();
		expect(a.reading).toBeNull();
	});

	it('nor does the row filling in the part picked in the tree count as typing', async () => {
		const { a } = make();
		const picked = { ...row(editSolid), fill: { geometry: 'G', solid: '2' } } as ActionRow;
		await a.perform(picked, column);
		expect(a.currentValues.solid).toBe(2);
		a.followPreview();
		await rest();
		expect(a.runError).toBeNull();
	});

	it('what was typed being refused says why, and takes the last preview down', async () => {
		const { a, answers } = make();
		await a.perform(row(addBlock), column);
		a.followPreview();
		await rest();
		expect(said(a)).toBe('PREVIEW');
		answers.refuse = 'the width is above zero';
		a.values = { ...a.values, [addBlock.id]: { ...a.currentValues, x: -1 } };
		a.followPreview();
		await rest();
		expect(a.runError).toBe('the width is above zero');
		expect(a.reading).toBeNull();
	});
});

describe('a drag in a reading', () => {
	it('refused, draws the same views anew, so the handle goes back to where it is', async () => {
		const { a, answers } = make();
		await a.perform(row(addBlock), column);
		await a.run();
		const before = a.reading!.views[0];
		answers.refuse = 'not there';
		await a.pickInReading({ op: 'project.slide_coordinate_system', params: {} }, { distance: 1 });
		expect(a.runError).toBe('not there');
		expect(a.reading!.views[0]).not.toBe(before);
		expect(a.reading!.views[0]).toEqual(before);
	});
});

describe('a form with a Preview button', () => {
	// Brandon, 2026-11-11: a Mesh tickbox meant "the preview until ticked"; Apply should mean do it.
	const meshSolids = {
		id: 'project.mesh_solids',
		title: 'Mesh Solids',
		workspace: 'geometry',
		modules: ['geometry'],
		commits: 'keep',
		outputs: ['reading'],
		params: [geometry, { name: 'size', label: 'Size', type: 'number', default: 0.01 }, { name: 'keep', label: 'Mesh', type: 'boolean', default: false }]
	} as unknown as OperationDescriptor;

	function withMesh() {
		const made = make();
		made.app.catalog = { operations: [addBlock, editSolid, meshSolids] } as never;
		return made;
	}

	it('does not ask the go-ahead param', async () => {
		const { a } = withMesh();
		await a.perform(row(meshSolids), column);
		expect(a.formParams.map((p) => p.name)).toEqual(['size']);
	});

	it('Preview sends it false and Apply sends it true', async () => {
		const { a, runs } = withMesh();
		await a.perform(row(meshSolids), column);
		await a.run(true);
		expect(runs.at(-1)?.params?.keep).toBe(false);
		expect(a.reading?.title).toBe('Mesh Solids, preview');
		await a.run();
		expect(runs.at(-1)?.params?.keep).toBe(true);
		expect(runs.at(-1)?.preview).toBeUndefined();
	});

	it('opens on its preview when it is opens_previewed, and not otherwise', async () => {
		const fitSensors = { ...meshSolids, id: 'project.create_sensors', title: 'Fit Sensors', opens_previewed: true, params: [geometry, { name: 'keep', label: 'Create', type: 'boolean', default: false }] } as unknown as OperationDescriptor;
		const { a, runs, app } = withMesh();
		app.catalog = { operations: [addBlock, meshSolids, fitSensors] } as never;
		await a.perform(row(fitSensors), column);
		expect(a.formOpen).toBe(true);
		expect(runs.map((r) => [r.op, r.params?.keep])).toEqual([['project.create_sensors', false]]);
		expect(a.reading?.title).toBe('Fit Sensors, preview');
		a.closeForm();
		await a.perform(row(meshSolids), column);
		expect(runs.length).toBe(1);
	});

	it('Preview does nothing on a form without one', async () => {
		const { a, runs } = withMesh();
		await a.perform(row(addBlock), column);
		const before = runs.length;
		await a.run(true);
		expect(runs.length).toBe(before);
	});
});

describe('a long operation', () => {
	// Brandon, 2026-11-11: a solve that takes a while shows how far it has got and can be stopped
	const solve = {
		id: 'project.solve_modes',
		title: 'Solve Modes',
		workspace: 'geometry',
		modules: ['geometry'],
		outputs: ['reading'],
		params: [geometry, { name: 'num_modes', label: 'Modes', type: 'integer', default: 20 }]
	} as unknown as OperationDescriptor;

	function slowly() {
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=ws://127.0.0.1:1'));
		const sent: string[] = [];
		let finish: (value: unknown) => void = () => {};
		const request = vi.fn((method: string) => {
			sent.push(method);
			if (method === 'op.run') return new Promise((resolve) => (finish = resolve));
			if (method === 'op.cancel') return Promise.resolve({ op: solve.id });
			if (method === 'project.actions') return Promise.resolve({ table: { columns: [{ kinds: ['geometry'], current: true, rows: [] }] } });
			return Promise.resolve({});
		});
		(app.analysis as unknown as { request: unknown }).request = request;
		const handlers = (app.analysis as unknown as { handlers: Map<string, Set<(data: unknown) => void>> }).handlers;
		const emit = (data: unknown) => handlers.get('op.progress')?.forEach((h) => h(data));
		app.catalog = { operations: [solve] } as never;
		app.project.summary = summary;
		return { a: app.analyze, sent, emit, finish: () => finish({ views: [{ view: 'reading', title: 'done', rows: [] }] }) };
	}

	it('follows its progress while it runs and forgets it after', async () => {
		const { a, emit, finish } = slowly();
		await a.perform(row(solve), column);
		const running = a.run();
		await Promise.resolve();
		emit({ op: solve.id, done: 0, total: -1 });
		expect(a.opProgress).toEqual({ done: 0, total: -1 });
		emit({ op: 'project.other', done: 5, total: 10 });
		expect(a.opProgress, 'another operation\'s progress is not ours').toEqual({ done: 0, total: -1 });
		emit({ op: solve.id, done: 3, total: 10 });
		expect(a.opProgress).toEqual({ done: 3, total: 10 });
		finish();
		await running;
		expect(a.opProgress).toBeNull();
	});

	it('Cancel asks the service once, and only while it runs', async () => {
		const { a, sent, finish } = slowly();
		await a.perform(row(solve), column);
		await a.cancel();
		expect(sent).not.toContain('op.cancel');
		const running = a.run();
		await Promise.resolve();
		await a.cancel();
		await a.cancel();
		expect(sent.filter((m) => m === 'op.cancel')).toHaveLength(1);
		expect(a.cancelling).toBe(true);
		finish();
		await running;
		expect(a.cancelling).toBe(false);
	});
});

describe('a value typed into an open form', () => {
	// Brandon, 2026-11-11: a mesh size typed, then Preview, then Apply meshed at the detected size
	const mesh = {
		id: 'project.mesh_solids',
		title: 'Mesh',
		workspace: 'geometry',
		modules: ['geometry'],
		outputs: ['reading'],
		commits: 'keep',
		params: [geometry, { name: 'size', label: 'Element size', type: 'number', default: 0 }, { name: 'apart', label: 'Apart', type: 'boolean', default: false }, { name: 'keep', label: 'Mesh', type: 'boolean', default: false }]
	} as unknown as OperationDescriptor;

	function meshing() {
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=ws://127.0.0.1:1'));
		const runs: Sent[] = [];
		const detected = { size: '0.01' };
		const request = vi.fn(async (method: string, params: Sent) => {
			if (method === 'op.run') {
				runs.push(params);
				return { views: [{ view: 'reading', title: 'r', rows: [] }], project: summary };
			}
			if (method === 'project.actions') {
				const rows = [{ id: mesh.id, verb: 'Mesh', kind: 'run', ready: true, fill: { geometry: 'G', size: detected.size } }];
				return { table: { columns: [{ kinds: ['geometry'], current: true, rows }] } };
			}
			if (method === 'project.show') return { title: 'shown', views: [], note: '' };
			return {};
		});
		(app.analysis as unknown as { request: unknown }).request = request;
		Object.defineProperty(app.analysis, 'isOpen', { get: () => true });
		app.catalog = { operations: [mesh] } as never;
		app.project.summary = summary;
		return { a: app.analyze, runs, detected };
	}

	const opened = (detected: { size: string }) =>
		({ id: mesh.id, verb: 'Mesh', kind: 'run', ready: true, fill: { geometry: 'G', size: detected.size } }) as unknown as ActionRow;

	it('stays through a Preview and the reload after it, and is what Apply sends', async () => {
		const { a, runs, detected } = meshing();
		await a.perform(opened(detected), column);
		expect(a.formOpen).toBe(true);
		expect(a.currentValues.size).toBe(0.01);
		a.values = { ...a.values, [mesh.id]: { ...a.currentValues, size: 0.005 } };
		await a.run(true);
		await a.loadActions();
		expect(a.currentValues.size, 'the detected size must not come back over the typed one').toBe(0.005);
		await a.run();
		expect(runs.map((r) => [r.params?.size, r.params?.keep])).toEqual([[0.005, false], [0.005, true]]);
	});

	it('follows the selection when what it would be filled with changes', async () => {
		const { a, detected } = meshing();
		await a.perform(opened(detected), column);
		a.values = { ...a.values, [mesh.id]: { ...a.currentValues, size: 0.005 } };
		detected.size = '0.02';
		await a.loadActions();
		expect(a.currentValues.size).toBe(0.02);
	});
});

describe('a reading drawn as its form opens', () => {
	// Brandon, 2026-11-22: Wavelet drawn at once and again after each change, with no Apply, and a
	// choice of record when the history is picked whole or several of its records are
	const wavelet = {
		id: 'project.wavelet',
		title: 'Wavelet',
		workspace: 'geometry',
		modules: ['geometry'],
		outputs: ['waterfall', 'matrix', 'spectrum', 'reading'],
		params: [
			{ name: 'history', label: 'Time history', type: 'enum', options_from: 'objects:time_history' },
			{ name: 'record', label: 'Record', type: 'integer', default: 1, min: 1 },
			{ name: 'per_octave', label: 'Per octave', type: 'integer', default: 12, min: 1 }
		]
	} as unknown as OperationDescriptor;

	function reading() {
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=ws://127.0.0.1:1'));
		const runs: Sent[] = [];
		const offer: { fill: Record<string, string>; labels: Record<string, string> } = { fill: { history: 'T' }, labels: { '1': '1Z+', '2': '2Z+', '3': '3Z+' } };
		const opened = () => ({ id: wavelet.id, verb: 'Wavelet', kind: 'run', ready: true, live: true, shows: true, fill: { ...offer.fill }, labels: { record: { ...offer.labels } } }) as unknown as ActionRow;
		const held = { ...summary, objects: [...summary.objects, { name: 'T', kind: 'time_history', records: 3, groups: [], verbs: [] }] } as unknown as ProjectSummary;
		const request = vi.fn(async (method: string, params: Sent) => {
			if (method === 'op.run') {
				runs.push(params);
				return { views: [{ view: 'reading', title: `record ${params.params?.record}`, rows: [] }], project: held };
			}
			if (method === 'project.actions') return { table: { columns: [{ kinds: ['time_history'], current: true, rows: [opened()] }] } };
			if (method === 'project.show') return { title: 'shown', views: [], note: '' };
			return {};
		});
		(app.analysis as unknown as { request: unknown }).request = request;
		Object.defineProperty(app.analysis, 'isOpen', { get: () => true });
		app.catalog = { operations: [wavelet] } as never;
		app.project.summary = held;
		return { a: app.analyze, runs, offer, opened };
	}
	const records = (runs: Sent[]) => runs.map((r) => r.params?.record);

	it('is drawn as the form opens, with no Apply to press', async () => {
		const { a, runs, opened } = reading();
		await a.perform(opened(), column);
		expect(a.formOpen).toBe(true);
		expect(records(runs)).toEqual([1]);
		expect(said(a)).toBe('record 1');
	});

	it('asks the record by the names its row gives, valued as the record is typed', async () => {
		const { a, opened } = reading();
		await a.perform(opened(), column);
		const record = a.formParams.find((p) => p.name === 'record');
		expect(record?.type).toBe('enum');
		expect(record?.options).toEqual([
			{ value: 1, label: '1Z+' },
			{ value: 2, label: '2Z+' },
			{ value: 3, label: '3Z+' }
		]);
	});

	it('is drawn again when a change of picks fills it differently, and not when nothing changed', async () => {
		const { a, runs, offer, opened } = reading();
		await a.perform(opened(), column);
		await a.loadActions();
		expect(records(runs), 'the same picks run nothing again').toEqual([1]);
		offer.fill = { history: 'T', record: '3' };
		offer.labels = { '3': '3Z+', '2': '2Z+' };
		await a.loadActions();
		await vi.waitFor(() => expect(records(runs)).toEqual([1, 3]));
		expect(said(a)).toBe('record 3');
	});

	it('gives way to a record still offered when the one it holds is no longer picked', async () => {
		const { a, opened, offer } = reading();
		await a.perform(opened(), column);
		a.values = { ...a.values, [wavelet.id]: { ...a.currentValues, record: 2 } };
		offer.labels = { '1': '1Z+', '3': '3Z+' };
		await a.loadActions();
		expect(a.currentValues.record).toBe(1);
	});

	it('keeps the reading when the 3D/2D toggle is set', async () => {
		const { a, opened } = reading();
		await a.perform(opened(), column);
		a.setPref('flat', !a.prefs.flat);
		expect(said(a)).toBe('record 1');
		a.setPref('flat', !a.prefs.flat);
		expect(said(a)).toBe('record 1');
	});
});

describe('a finished run opened in Analysis', () => {
	const runObjects = ['Channel Table', 'inputs', 'drives', 'Specification', 'Run Record'].map((name) => ({ name, kind: 'time_history', records: 1, groups: ['run'], verbs: [] }));
	function runApp(start: ProjectSummary | null) {
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=ws://127.0.0.1:1'));
		const sent: [string, Record<string, unknown>][] = [];
		const answers = { refuse: '' };
		const request = vi.fn(async (method: string, params: Record<string, unknown>) => {
			if (method === 'project.open' || method === 'project.import') {
				sent.push([method, params]);
				if (answers.refuse) throw new RequestError('refused', answers.refuse, method);
				const before = method === 'project.import' ? (app.project.summary?.objects ?? []) : [];
				return { added: runObjects.map((o) => o.name), project: { ...summary, title: 'run', file: null, objects: [...before, ...runObjects] } };
			}
			if (method === 'project.actions') return { table: { columns: [] } };
			return {};
		});
		(app.analysis as unknown as { request: unknown }).request = request;
		app.project.summary = start;
		app.workspace = 'acquire';
		return { app, sent, answers };
	}

	it('opens the folder as the project when the project holds nothing, and shows the analysis', async () => {
		const { app, sent } = runApp({ ...summary, objects: [], active_geometry: null } as unknown as ProjectSummary);
		expect(await app.runOpen.open('/data/20261113-180000_random_r-0001')).toBe(true);
		expect(sent).toEqual([['project.open', { path: '/data/20261113-180000_random_r-0001' }]]);
		expect(app.workspace).toBe('signals');
		expect(app.project.selection.objects).toEqual(runObjects.map((o) => o.name));
		expect(app.project.busy).toBeNull();
	});

	it('imports it into a project that holds objects, keeping them', async () => {
		const { app, sent } = runApp(summary);
		expect(await app.runOpen.open('/data/run')).toBe(true);
		expect(sent.map(([m]) => m)).toEqual(['project.import']);
		expect(app.project.summary?.objects.map((o) => o.name)).toEqual(['G', 'H', ...runObjects.map((o) => o.name)]);
		expect(app.project.selection.objects).toEqual(runObjects.map((o) => o.name));
		expect(app.workspace).toBe('signals');
	});

	it('stays on the run screen and says why when the analysis service refuses', async () => {
		const { app, answers } = runApp(summary);
		answers.refuse = 'The folder has no run.json.';
		expect(await app.runOpen.open('/data/run')).toBe(false);
		expect(app.workspace).toBe('acquire');
		expect(app.runOpen.status).toBe('refused');
		expect(app.runOpen.refusal?.message).toBe('The folder has no run.json.');
		expect(app.project.busy).toBeNull();
	});
});

describe('deleting parts', () => {
	it('says "all of" an object whose every part is picked, which deletes the object', () => {
		const { app } = make();
		app.project.children = {
			Run: { kind: 'grid', rows: [], columns: [], cells: [[0, 1], [2, -1]], icons: [], undefined: [] },
			Modes: { kind: 'modes', modes: [{ index: 0, frequency: 10, damping: 0.01, description: '' }] }
		} as never;
		expect(app.project.partsSaid({ Run: [0, 2] })).toBe('2 of Run');
		expect(app.project.partsSaid({ Run: [0, 1, 2], Modes: [0] })).toBe('all of Run, all of Modes');
		expect(app.project.partsSaid({ Closed: [0] }), 'an object not open is counted as picked').toBe('1 of Closed');
	});
});

describe('the fit cursor dragged', () => {
	it('sends where it is quietly, with no busy line under the hand, and a button press still says so', async () => {
		// the pending mode follows the cursor as it is dragged (Brandon, 2026-11-14)
		const { app } = make();
		const seen: (string | null)[] = [];
		let release!: () => void;
		(app.analysis as unknown as { request: unknown }).request = vi.fn(
			(method: string) =>
				new Promise((resolve) => {
					release = () => resolve({ fit: { method } });
				})
		);
		const quiet = app.fit.move(440, true);
		seen.push(app.fit.busy);
		release();
		expect(await quiet).toBe(true);
		const pressed = app.fit.fit();
		seen.push(app.fit.busy);
		release();
		await pressed;
		expect(seen).toEqual([null, 'Fitting the mode']);
		expect(app.fit.busy).toBeNull();
	});
});

describe('the force of a modal FRF response moved', () => {
	// each force's stage comes back at its own strongest line unless a line is asked for
	const strongest: Record<string, number> = { '101Z+': 120, '505Z+': 310 };
	beforeEach(() => void vi.useFakeTimers());
	afterEach(() => void vi.useRealTimers());
	function driven() {
		const made = make();
		const asked: { drive: string | null; line: number | null }[] = [];
		(made.app.analysis as unknown as { request: unknown }).request = vi.fn(async (method: string, params: { prefs?: { drive: string | null; line: number | null } }) => {
			if (method !== 'project.show') return { table: { columns: [] } };
			const prefs = params.prefs!;
			asked.push({ drive: prefs.drive, line: prefs.line });
			const frequency_hz = prefs.line ?? strongest[prefs.drive ?? '101Z+'];
			return { title: 'shown', note: '', views: [{ view: 'shape3d', cursor: 'frequency', modes: [{ frequency_hz }] }] };
		});
		made.app.project.selection = { objects: ['Modal FRFs', 'Two', 'Plate'], records: {} } as never;
		return { ...made, asked };
	}

	it('keeps the line shown when the cursor was never moved', async () => {
		const { a, asked } = driven();
		a.setPref('drive', null);
		await a.showSelection();
		a.setPref('drive', '505Z+');
		await rest();
		expect(asked.at(-1)).toEqual({ drive: '505Z+', line: 120 });
		a.setPref('drive', '101Z+');
		await rest();
		expect(asked.at(-1)).toEqual({ drive: '101Z+', line: 120 });
	});

	it('keeps the line the cursor was put on', async () => {
		const { a, asked } = driven();
		await a.showSelection();
		a.moveLine(0);
		await rest();
		a.setPref('drive', '505Z+');
		await rest();
		expect(asked.at(-1)).toEqual({ drive: '505Z+', line: 0 });
	});

	it('starts a fresh selection at its strongest line again', async () => {
		const { app, a, asked } = driven();
		a.setPref('drive', null);
		await a.showSelection();
		a.setPref('drive', '505Z+');
		await rest();
		app.project.selection = { objects: ['Plate'], records: {} } as never;
		await a.showSelection();
		app.project.selection = { objects: ['Modal FRFs', 'Two', 'Plate'], records: {} } as never;
		await a.showSelection();
		expect(asked.at(-1)).toEqual({ drive: '505Z+', line: null });
	});
});

describe('a shell with no control service (`?control=none`)', () => {
	// `hertzforge` with no command opens the shell from the Python package, where no control
	// service runs: the workspaces that need one are not offered and nothing connects to it.
	it('offers the analysis and the report alone, and opens the analysis for a control address', () => {
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=none'));
		expect(app.config.control).toBe(false);
		expect(app.workspaces.map((w) => w.id)).toEqual(['signals', 'report']);
		app.go('acquire');
		expect(app.workspace).toBe('signals');
		app.go('simulate');
		expect(app.workspace).toBe('signals');
		app.go('report');
		expect(app.workspace).toBe('report');
	});

	it('offers every workspace when a control service is named or assumed', () => {
		for (const search of ['?analysis=ws://127.0.0.1:1&control=ws://127.0.0.1:1', '']) {
			const app = new AppState(readConfig(search));
			expect(app.config.control).toBe(true);
			expect(app.workspaces.map((w) => w.id)).toEqual(['acquire', 'simulate', 'signals', 'report']);
			app.go('simulate');
			expect(app.workspace).toBe('simulate');
		}
	});
});

describe("a report's Print", () => {
	const g = globalThis as { __TAURI__?: unknown };
	afterEach(() => {
		delete g.__TAURI__;
	});

	// The desktop app's web view does not print; there the saved HTML file is opened in a browser
	// instead, so the app offers no Print and asks the service for no printable page.
	it('is not offered in the desktop app, and asks for nothing there', async () => {
		g.__TAURI__ = { core: { invoke: vi.fn() }, dialog: { open: vi.fn(), save: vi.fn() } };
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=none'));
		const request = vi.spyOn(app.analysis, 'request').mockResolvedValue({ html: '<p>page</p>', owed: [], index: [] } as never);
		app.report.name = 'Random';
		expect(app.report.canPrint).toBe(false);
		await app.report.print();
		expect(request.mock.calls.filter(([method]) => method === 'report.print')).toEqual([]);
	});

	it('is offered in a browser, and prints the page in place', async () => {
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=none'));
		const request = vi.spyOn(app.analysis, 'request').mockResolvedValue({ html: '<p>page</p>', owed: [], index: [] } as never);
		app.report.name = 'Random';
		expect(app.report.canPrint).toBe(true);
		await app.report.print();
		const sent = request.mock.calls.filter(([method]) => method === 'report.print');
		expect(sent.map(([, params]) => params)).toEqual([{ name: 'Random' }]);
		expect(app.report.printing).toBe('<p>page</p>');
	});
});

describe('open, import and save in a browser', () => {
	function browser(answer: (method: string, params: Record<string, unknown>) => unknown) {
		const app = new AppState(readConfig('?analysis=ws://127.0.0.1:1&control=none'));
		const sent: [string, Record<string, unknown>][] = [];
		(app.analysis as unknown as { request: unknown }).request = vi.fn(async (method: string, params: Record<string, unknown>) => {
			sent.push([method, params]);
			return answer(method, params);
		});
		app.project.summary = { ...summary, title: 'Plate', file: null, modified: false } as never;
		return { app, sent };
	}

	it('opens, imports and saves the paths picked in the dialog the service shows', async () => {
		const { app, sent } = browser((method, params) => {
			if (method === 'files.choose') return { paths: params.purpose === 'import' ? ['/d/a.uff', '/d/b.csv'] : params.purpose === 'save' ? ['/d/Plate.hforge'] : ['/d/run.hforge'] };
			return { project: summary, added: [] };
		});
		// Save As first: the open that follows replaces the project, and its title with it
		await app.project.fileAct('save-as');
		await app.project.fileAct('import');
		await app.project.fileAct('open');
		expect(sent.filter(([m]) => m === 'files.choose').map(([, p]) => p)).toEqual([{ purpose: 'save', name: 'Plate.hforge' }, { purpose: 'import' }, { purpose: 'open' }]);
		expect(sent.filter(([m]) => m !== 'files.choose' && m.startsWith('project.') && m !== 'project.actions').map(([m, p]) => [m, p.path])).toEqual([
			['project.save', '/d/Plate.hforge'],
			['project.import', '/d/a.uff'],
			['project.import', '/d/b.csv'],
			['project.open', '/d/run.hforge']
		]);
		expect(app.project.asking).toBeNull();
	});

	it('does nothing when the dialog is cancelled', async () => {
		const { app, sent } = browser(() => ({ paths: [] }));
		await app.project.fileAct('open');
		expect(sent.map(([m]) => m)).toEqual(['files.choose']);
		expect(app.project.asking).toBeNull();
	});

	it('asks for a typed path where the service shows no dialog, and does not ask it again', async () => {
		const { app, sent } = browser((method) => {
			if (method === 'files.choose') throw new RequestError('refused', 'This service shows no file dialogs; type the path instead.', method);
			return {};
		});
		await app.project.fileAct('import');
		expect(app.project.asking).toBe('import');
		await app.project.fileAct('open');
		expect(app.project.asking).toBe('open');
		expect(sent.filter(([m]) => m === 'files.choose')).toHaveLength(1);
	});
});

describe('a projection, whose distance its row fills in', () => {
	const project = {
		id: 'project.project_onto_basis',
		title: 'Project onto Basis DOFs',
		workspace: 'modal',
		modules: ['modal'],
		outputs: ['shape3d'],
		params: [
			{ name: 'source', label: 'Shape set', type: 'enum', options_from: 'verb:project_onto_basis' },
			{ name: 'onto', label: 'Onto', type: 'enum', options_from: 'objects:shape_set|channel_table|geometry' },
			{ name: 'distance', label: 'Node match tolerance', type: 'number', default: 0, unit: 'm', length: true }
		]
	} as unknown as OperationDescriptor;
	const groups = (defined: boolean) =>
		({
			...summary,
			active_geometry: null,
			objects: [
				{ name: 'Model', kind: 'geometry', records: 0, groups: ['A'], verbs: [], units_defined: defined },
				{ name: 'Modes', kind: 'shape_set', records: 0, groups: ['A'], verbs: ['project_onto_basis'] },
				{ name: 'Test', kind: 'geometry', records: 0, groups: ['B'], verbs: [], units_defined: defined }
			]
		}) as unknown as ProjectSummary;
	const filled = { id: project.id, verb: 'Project', kind: 'run', ready: true, fill: { source: 'Modes', onto: 'Test', distance: '2.5' } } as unknown as ActionRow;

	function projecting(defined = true) {
		const made = make();
		made.app.catalog = { operations: [project] } as never;
		made.app.project.summary = groups(defined);
		made.app.units.length = { unit: 'mm', to_si: 0.001 } as never;
		return made;
	}

	it('opens its form on the distance rather than running at once', async () => {
		const { a, runs } = projecting();
		await a.perform(filled, column);
		expect(a.formOpen).toBe(true);
		expect(runs).toHaveLength(0);
		expect(a.currentValues.distance).toBe(2.5);
		expect(a.formParams.map((p) => p.name)).toEqual(['distance']);
	});

	it('shows the display unit when its geometries have units, and none when they have not', async () => {
		const shown = projecting();
		await shown.a.perform(filled, column);
		expect(shown.a.formParams[0].unit).toBe('mm');
		const own = projecting(false);
		await own.a.perform(filled, column);
		expect(own.a.formParams[0].unit).toBeUndefined();
	});
});

describe('the display system changed', () => {
	beforeEach(() => void vi.useFakeTimers());
	afterEach(() => void vi.useRealTimers());
	// the service draws in the system it was last set to, as the real one does
	function unitsMade() {
		const made = make();
		let system = 'mm-kg-N-s';
		const shows: { line: number | null; projection: boolean }[] = [];
		const runs: Sent[] = [];
		(made.app.analysis as unknown as { request: unknown }).request = vi.fn(async (method: string, params: Sent & { name?: string; prefs?: { line: number | null; projection: boolean } }) => {
			if (method === 'units.system') {
				if (params.name) system = params.name;
				return { name: system, systems: ['mm-kg-N-s', 'in-slinch-lbf-s'] };
			}
			if (method === 'project.show') {
				shows.push({ line: params.prefs!.line, projection: params.prefs!.projection });
				return { title: `shown in ${system}`, note: '', views: [] };
			}
			if (method === 'op.run') {
				runs.push(params);
				return { views: [{ view: 'reading', title: `${params.preview ? 'PREVIEW' : 'REAL'} in ${system}`, rows: [] }] };
			}
			return { table: { columns: [] } };
		});
		made.app.project.selection = { objects: ['Test Modes', 'FEM Modes'], records: {} } as never;
		return { ...made, shows, runs };
	}

	it('draws the selection again in the new units, keeping its line, picks and Projection', async () => {
		const { app, a, shows } = unitsMade();
		await app.units.sync();
		await a.showSelection();
		a.setPref('projection', true);
		a.moveLine(12);
		a.pickCell({ row: 1, column: 2 });
		await rest();
		expect(a.shown?.title).toBe('shown in mm-kg-N-s');
		await app.units.choose('in-slinch-lbf-s');
		await rest();
		expect(a.shown?.title).toBe('shown in in-slinch-lbf-s');
		expect(shows.at(-1)).toEqual({ line: 12, projection: true });
		expect(a.picks).toEqual([{ row: 1, column: 2 }]);
	});

	it('keeps a reading that was applied, and draws a preview again', async () => {
		const { app, a, runs } = unitsMade();
		await app.units.sync();
		await a.perform(row(addBlock), column);
		await a.run();
		await app.units.choose('in-slinch-lbf-s');
		await rest();
		expect(said(a)).toBe('REAL in mm-kg-N-s');
		expect(runs.filter((r) => !r.preview).length).toBe(1);
		a.followPreview();
		await rest();
		expect(said(a)).toBe('PREVIEW in in-slinch-lbf-s');
		await app.units.choose('mm-kg-N-s');
		await rest();
		expect(said(a)).toBe('PREVIEW in mm-kg-N-s');
		expect(a.reading?.title).toBe('Add Block, preview');
	});
});

describe("Project's direction, worded by its row", () => {
	const project = {
		id: 'project.project_onto_basis',
		title: 'Project onto Basis DOFs',
		workspace: 'modal',
		modules: ['modal'],
		outputs: ['shape3d'],
		previews: true,
		params: [
			{ name: 'source', label: 'Shape set', type: 'enum', options_from: 'objects:shape_set' },
			{ name: 'onto', label: 'Onto', type: 'enum', options_from: 'objects:shape_set|channel_table|geometry' },
			{ name: 'direction', label: 'Direction', type: 'enum', default: 'named', options: [{ value: 'named', label: 'Shape set onto Onto' }, { value: 'reversed', label: 'Onto onto Shape set' }] }
		]
	} as unknown as OperationDescriptor;
	const sets = {
		...summary,
		objects: [
			{ name: 'Test', kind: 'shape_set', records: 0, groups: [], verbs: [] },
			{ name: 'Model', kind: 'shape_set', records: 0, groups: [], verbs: [] }
		]
	} as unknown as ProjectSummary;
	const labels = { direction: { named: 'Test onto Model (not possible: Test has 2 nodes measured in fewer than three directions)', reversed: 'Model onto Test' } };
	const row = (direction: string) => ({ id: project.id, verb: 'Project', kind: 'run', ready: true, fill: { source: 'Test', onto: 'Model', direction }, labels }) as unknown as ActionRow;

	function directed() {
		const made = make();
		made.app.catalog = { operations: [project] } as never;
		made.app.project.summary = sets;
		return made;
	}

	it('names each way after the two sets, the one that cannot be made with why', async () => {
		const { a } = directed();
		await a.perform(row('reversed'), column);
		const direction = a.formParams.find((p) => p.name === 'direction');
		expect(direction?.options?.map((o) => o.label)).toEqual([labels.direction.named, 'Model onto Test']);
		expect(a.currentValues.direction).toBe('reversed');
		expect(a.errors.direction).toBeUndefined();
	});

	it('offers only the ways its row names', async () => {
		const { a } = directed();
		const one = { ...row('named'), labels: { direction: { named: 'Test at the nodes of Model' } } } as unknown as ActionRow;
		await a.perform(one, column);
		const direction = a.formParams.find((p) => p.name === 'direction');
		expect(direction?.options?.map((o) => [o.value, o.label])).toEqual([['named', 'Test at the nodes of Model']]);
	});

	it('says why when the way chosen is the one that cannot be made', async () => {
		vi.useFakeTimers();
		try {
			const { a, answers } = directed();
			await a.perform(row('reversed'), column);
			answers.refuse = 'Test cannot be projected onto Model: Test has 2 nodes measured in fewer than three directions';
			a.values = { ...a.values, [project.id]: { ...a.currentValues, direction: 'named' } };
			a.followPreview();
			await vi.advanceTimersByTimeAsync(400);
			expect(a.runError).toBe(answers.refuse);
		} finally {
			vi.useRealTimers();
		}
	});

	it('asks which way when its row chose neither, and runs nothing until one is chosen', async () => {
		const { a, runs } = directed();
		await a.perform(row(''), column);
		expect(a.formOpen).toBe(true);
		expect(a.errors.direction).toBe('Choose one of the options.');
		await a.run();
		expect(runs).toHaveLength(0);
	});
});

describe('the quantity a picked object draws', () => {
	beforeEach(() => void vi.useFakeTimers());
	afterEach(() => void vi.useRealTimers());
	it('is asked of the service, and a new launch starts at the largest again', async () => {
		const made = make();
		const asked: (string | null)[] = [];
		(made.app.analysis as unknown as { request: unknown }).request = vi.fn(async (method: string, params: { prefs?: { quantity: string | null } }) => {
			if (method !== 'project.show') return { table: { columns: [] } };
			asked.push(params.prefs!.quantity);
			return { title: 'shown', note: '', views: [] };
		});
		made.app.project.selection = { objects: ['Run'], records: {} } as never;
		await made.a.showSelection();
		made.a.setPref('quantity', 'voltage|');
		await rest();
		expect(asked).toEqual([null, 'voltage|']);
		expect(make().a.prefs.quantity).toBeNull();
	});
});

describe('a row picked in a table the selection shows', () => {
	it('runs its select and puts the views answered beside the table, the table kept', async () => {
		const made = make();
		const plot = (title: string) => ({ view: 'spectrum', title, x: {}, y: {}, series: [] });
		const table = { view: 'table', title: 'scores', columns: ['Station'], rows: [['1'], ['2']], select: { op: 'project.time_comparison', params: { comparison: 'C' }, param: 'station', column: 0 } };
		const sent: unknown[] = [];
		(made.app.analysis as unknown as { request: unknown }).request = vi.fn(async (method: string, params: unknown) => {
			sent.push([method, params]);
			return { views: [plot('station 2'), { ...table, rows: [['other']] }, { view: 'reading', title: 'summary 2', rows: [] }] };
		});
		made.a.shown = { title: 'C', note: '', views: [plot('station 1'), table, { view: 'reading', title: 'summary 1', rows: [] }] } as never;
		await made.a.pickShownRow(table.select, '2');
		expect(sent).toEqual([['op.run', { op: 'project.time_comparison', params: { comparison: 'C', station: 2 } }]]);
		const views = (made.a.shown as unknown as { views: { title: string; rows?: unknown }[] }).views;
		expect(views.map((v) => v.title)).toEqual(['station 2', 'scores', 'summary 2']);
		expect(views[1].rows).toEqual([['1'], ['2']]);
	});
});
