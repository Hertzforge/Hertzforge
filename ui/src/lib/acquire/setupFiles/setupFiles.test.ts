/**
 * The shared setup bar, over every test's real store against the mock control service and an
 * analysis service kept in memory: a setup saved from each test's draft opens back as the same
 * configuration; a setup from each golden run folder fills the draft only after every difference
 * the draft cannot hold, and every channel table difference, is named; a setup for another module
 * or protocol is refused before the draft is touched; opening sends nothing to the control service
 * but its dry runs; the service's refusals are shown in its words; and the origin the Arm verb
 * shows holds until the operator edits the draft.
 */
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { AppState } from '$lib/app/state.svelte';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ChannelRow, ProjectSummary } from '$lib/protocol/types';
import { AcquireState } from '../store.svelte';
import { SineStore } from '../sine/sineStore.svelte';
import { MixedStore } from '../mixed/mixedStore.svelte';
import { ShockStore } from '../shock/shockStore.svelte';
import { SrsStore } from '../srs/srsStore.svelte';
import { TwrStore } from '../twr/twrStore.svelte';
import { ExplorerStore } from '../explorer/explorerStore.svelte';
import { RecorderStore } from '../recorder/recorderStore.svelte';
import { MimoShockStore } from '../mimoshock/msStore.svelte';
import { MimoState } from '../mimo/mimoStore.svelte';
import { canonical, configDifferences, originWords, SETUP_MODULES, tableDifferences, vetSetup, type SetupModule, type TestSetupObject } from './setupFile';
import { SetupFiles, type AnalysisClient } from './setupFiles.svelte';
import { hostFor, type SetupHost } from './setupHosts';
import { knownOrigin, originFor, SetupOriginState } from './setupOrigin.svelte';

type Config = Record<string, unknown>;

const GOLDEN = [resolve(process.cwd(), '../schema/golden'), resolve(process.cwd(), 'schema/golden')].find((p) => existsSync(p))!;
const golden = (path: string) => JSON.parse(readFileSync(resolve(GOLDEN, path), 'utf8'));

/** Each module's configure request golden, and the member of its params that is the setup. */
const REQUESTS: Record<SetupModule, [string, string]> = {
	random: ['request_random_configure', 'config'],
	sine: ['request_sine_configure', 'config'],
	mixed: ['request_mixed_configure', 'config'],
	classical_shock: ['request_shock_configure', 'config'],
	srs: ['request_srs_configure', 'config'],
	twr: ['request_twr_configure', 'config'],
	explorer: ['request_explorer_configure', 'config'],
	recorder: ['request_recorder_configure', 'config'],
	mimo_random: ['request_mimo_random_setup', 'setup'],
	mimo_shock: ['request_mimo_shock_configure', 'config']
};

const GOLDEN_RUNS: SetupModule[] = ['random', 'sine', 'mixed', 'classical_shock', 'srs', 'twr', 'explorer', 'recorder', 'mimo_random'];

/** The analysis service's project, in memory: saved setups and run records. */
class FakeAnalysis implements AnalysisClient {
	isOpen = true;
	setups = new Map<string, TestSetupObject>();
	runs = new Map<string, TestSetupObject>();
	sent: { method: string; params: Record<string, unknown> }[] = [];
	summary: ProjectSummary = { title: 'p', file: null, modified: false, objects: [] } as unknown as ProjectSummary;

	async request<R>(method: string, params: Record<string, unknown> = {}): Promise<R> {
		this.sent.push({ method, params: structuredClone(params) });
		if (method === 'project.setups') {
			const setups = [...this.setups].map(([name, s]) => ({ name, module: s.module, title: s.title, protocol: s.protocol, origin: s.origin, holds: `${s.module} setup` }));
			const runs = [...this.runs].map(([name, s]) => ({ name, module: s.module, title: s.title, run_uid: 'uid', run_id: 'r-0001', outcome: 'completed', source: 'SIM', has_config: true, holds: '' }));
			return { setups, runs } as R;
		}
		if (method === 'project.save_setup') {
			const name = params.name as string;
			if (this.setups.has(name) && !params.replace) throw new Error(`the project already holds an object named '${name}'`);
			this.setups.set(name, structuredClone(params.setup) as TestSetupObject);
			return { name, project: { ...this.summary, modified: true } } as R;
		}
		if (method === 'project.open_setup') {
			const found = this.setups.get(params.name as string) ?? this.runs.get(params.name as string);
			if (!found) throw new Error(`the project has no object named '${params.name}'`);
			return { setup: structuredClone(found) } as R;
		}
		throw new Error(`unexpected ${method}`);
	}
}

async function shell() {
	const server = createControlServer({ speed: 20 });
	const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
	const acq = new AcquireState(client, { session: null, local: null, checkDelayMs: 0 });
	client.connect();
	await client.whenOpen();
	await acq.sync();
	await acq.claim();
	const opts = { local: null, checkDelayMs: 0 };
	const stores = {
		sine: new SineStore(acq, opts),
		mixed: new MixedStore(acq, opts),
		shock: new ShockStore(acq, opts),
		srs: new SrsStore(acq, opts),
		twr: new TwrStore(acq, opts),
		explorer: new ExplorerStore(acq, opts),
		recorder: new RecorderStore(acq, opts),
		mimoShock: new MimoShockStore(acq, opts),
		mimo: new MimoState(acq, opts)
	};
	await Promise.all(Object.values(stores).map((s) => s.sync()));
	// what the control service is asked from here on: opening a setup must ask nothing but dry runs
	const asked: { method: string; params: Record<string, unknown> }[] = [];
	const request = client.request.bind(client);
	client.request = ((method: string, params?: Record<string, unknown>, options?: { timeoutMs?: number }) => {
		asked.push({ method, params: params ?? {} });
		return request(method, params, options);
	}) as typeof client.request;
	const analysis = new FakeAnalysis();
	const project = { summary: analysis.summary as ProjectSummary | null, apply(s: ProjectSummary) { this.summary = s; } };
	const app = { control: client, acquire: acq, ...stores, analysis, project, config: { popout: false } } as unknown as AppState;
	const bar = (module: SetupModule) => {
		const host = hostFor(app, module);
		return new SetupFiles(host, analysis, project, originFor(host.store, () => new SetupOriginState(() => host.key())));
	};
	return { app, acq, analysis, asked, bar };
}

/** Nothing but checks reached the control service: no configure that is not a dry run, no arm, no start. */
const onlyChecks = (asked: { method: string; params: Record<string, unknown> }[]) =>
	asked.filter((a) => /\.(arm|start|fire|record|proceed)$/.test(a.method) || ((/\.configure$/.test(a.method) || a.method === 'mimo_random.setup') && !a.params.dry_run));

function asSetup(module: SetupModule, config: Config, rows: ChannelRow[] | null, protocol = 'sds/1'): TestSetupObject {
	return { module, protocol, config, table: rows ? { rows, revision: 1 } : {}, title: String(config.title ?? ''), notes: '', origin: { kind: 'typed' } };
}

describe('the shared setup bar on every test', () => {
	it('saves each test draft into the project and opens it back as the same configuration, filling the draft only', async () => {
		const { analysis, asked, bar, acq } = await shell();
		for (const module of SETUP_MODULES) {
			const files = bar(module);
			const host = files.host;
			if (module === 'mimo_random') {
				// MIMO Random's draft makes no setup before it has a reference: it starts from the protocol's
				const [name, member] = REQUESTS[module];
				analysis.setups.set('start', asSetup(module, golden(`${name}.json`).params[member], acq.server!.channels));
				await files.openSaved('start');
				if (files.pending) files.confirm();
			}
			const typed = host.config();
			expect(typed, module).not.toBeNull();
			await files.refresh();
			files.saveName = `${module} saved`;
			expect(await files.save(), module).toBe(true);
			const saved = analysis.setups.get(`${module} saved`)!;
			expect(saved.config, module).toEqual(typed);
			expect(saved.module).toBe(module);
			expect(saved.table.rows).toEqual(acq.server!.channels);
			expect(files.originWords).toBe(`Setup from the saved setup "${module} saved"`);

			// the operator changes the draft; the origin then reads as edited
			host.fill({ ...typed!, title: 'Changed here' });
			expect(files.origin.edited, module).toBe(true);
			expect(files.originWords).toBe(`Setup edited here, after opening the saved setup "${module} saved"`);
			expect(files.origin.recorded).toEqual({ kind: 'typed' });

			expect(await files.openSaved(`${module} saved`), module).toBe(true);
			expect(files.pending, module).toBeNull();
			expect(host.config(), module).toEqual(typed);
			expect(files.origin.edited).toBe(false);
		}
		expect(onlyChecks(asked)).toEqual([]);
	});

	it('opens the setup of each golden run only after naming every difference the draft cannot hold', async () => {
		const { analysis, asked, bar } = await shell();
		for (const module of GOLDEN_RUNS) {
			const files = bar(module);
			const host = files.host;
			const config = golden(`runs/${module}/setup.json`).config as Config;
			const table = golden(`runs/${module}/table.json`);
			analysis.runs.set(`${module} run`, { ...asSetup(module, config, table.rows), origin: { kind: 'run', run_uid: `golden-${module}`, run_id: 'r-0001', name: `${module} run` } });
			const before = host.key();
			const ours = host.roundTrip(config)!;
			const opened = await files.openRun(`${module} run`);
			expect(files.refused, module).toBeNull();
			const exact = canonical(ours) === canonical(config);
			const p = files.pending;
			if (!opened) {
				// waiting on the operator: the draft is untouched, and each difference is named
				expect(p, module).not.toBeNull();
				expect(host.key(), module).toBe(before);
				expect(p!.vetting.config.length > 0, module).toBe(!exact);
				expect(p!.vetting.config).toEqual(configDifferences(config, ours));
				expect(files.confirm()).toBe(true);
			} else expect(exact, module).toBe(true);
			expect(host.config(), module).toEqual(ours);
			expect(files.originWords).toBe('Setup from run r-0001');
		}
		expect(onlyChecks(asked)).toEqual([]);
	});

	it('refuses a setup for another module or another protocol before the draft is touched', async () => {
		const { analysis, bar } = await shell();
		const files = bar('random');
		const before = files.host.key();
		analysis.setups.set('a sweep', asSetup('sine', golden('request_sine_configure.json').params.config, null));
		expect(await files.openSaved('a sweep')).toBe(false);
		expect(files.refused).toBe('This setup is for Sine, not Random. Open it on the Sine screen.');
		const random = golden('request_random_configure.json').params.config;
		analysis.setups.set('older', asSetup('random', random, null, 'sds/0'));
		expect(await files.openSaved('older')).toBe(false);
		expect(files.refused).toBe('This setup was written under protocol sds/0; the control service speaks sds/1. It is not opened, since its members may not mean the same.');
		expect(files.host.key()).toBe(before);
		expect(files.originWords).toBe('Setup typed here');
	});

	it('flags a setup made with another channel table and opens it only when told', async () => {
		const { analysis, acq, bar } = await shell();
		const files = bar('random');
		const config = files.host.config()!;
		const rows = structuredClone(acq.server!.channels);
		rows[0].role = rows[0].role === 'control' ? 'monitor' : 'control';
		rows[0].sensitivity_mv_per_eu = 9.5;
		analysis.setups.set('other table', asSetup('random', { ...config, title: 'Other table' }, rows));
		const before = files.host.key();
		expect(await files.openSaved('other table')).toBe(false);
		expect(files.pending!.vetting.table).toEqual(tableDifferences(rows, acq.server!.channels));
		expect(files.pending!.vetting.table[0]).toMatch(/^Channel 1: role ".*" in the setup, ".*" here; sensitivity 9.5 in the setup, .* here$/);
		expect(files.host.key()).toBe(before);
		files.cancel();
		expect(files.host.key()).toBe(before);
		expect(await files.openSaved('other table')).toBe(false);
		expect(files.confirm()).toBe(true);
		expect(files.host.config()!.title).toBe('Other table');
	});
});

/** A host with no store behind it, for the bar's own rules. */
function stub(over: Partial<SetupHost> = {}): SetupHost & { filled: Config[] } {
	let config: Config = { title: 'Stub', level: 1 };
	const filled: Config[] = [];
	return {
		module: 'random',
		store: {},
		config: () => config,
		key: () => canonical(config),
		title: () => String(config.title),
		roundTrip: (c) => c,
		fill: (c) => {
			filled.push(c);
			config = c;
		},
		findings: () => [],
		checking: () => false,
		locked: () => null,
		rows: () => [],
		table: () => ({ rows: [] }),
		protocol: () => 'sds/1',
		filled,
		...over
	};
}

function barOver(host: SetupHost, analysis = new FakeAnalysis()) {
	const project = { summary: analysis.summary as ProjectSummary | null, apply(s: ProjectSummary) { this.summary = s; } };
	return { files: new SetupFiles(host, analysis, project, new SetupOriginState(() => host.key())), analysis, project };
}

describe('the setup bar on its own', () => {
	it('shows the service words while the opened setup is as opened, and none once edited', async () => {
		let words = ['The profile needs at least two breakpoints.'];
		const host = stub({ findings: () => words });
		const { files, analysis } = barOver(host);
		expect(files.serviceWords).toEqual([]);
		analysis.setups.set('s', asSetup('random', { title: 'Opened', level: 2 }, []));
		expect(await files.openSaved('s')).toBe(true);
		expect(files.serviceWords).toEqual(['The profile needs at least two breakpoints.']);
		host.fill({ title: 'Opened', level: 3 });
		expect(files.serviceWords).toEqual([]);
		words = [];
	});

	it('opens nothing while the setup is locked or no project or service is there', async () => {
		const host = stub({ locked: () => 'The run is live; the setup is locked until it ends.' });
		const { files, analysis } = barOver(host);
		analysis.setups.set('s', asSetup('random', { title: 'x' }, []));
		expect(await files.openSaved('s')).toBe(false);
		expect(files.refused).toBe('The run is live; the setup is locked until it ends.');
		expect(host.filled).toEqual([]);
		expect(analysis.sent).toEqual([]);

		const off = stub({ protocol: () => null });
		const two = barOver(off);
		two.analysis.setups.set('s', asSetup('random', { title: 'x' }, []));
		expect(await two.files.openSaved('s')).toBe(false);
		expect(two.files.refused).toMatch(/^Connect to the control service first/);
		expect(off.filled).toEqual([]);

		const none = barOver(stub());
		none.project.summary = null;
		expect(none.files.openBlocked).toBe('Open a project first: setups are opened from it.');
		expect(none.files.saveBlocked).toBe('Open or start a project first: a setup is saved into it.');
		expect(await none.files.save()).toBe(false);
	});

	it('asks before replacing a saved setup of the same name', async () => {
		const { files, analysis } = barOver(stub());
		files.saveName = 'Mine';
		expect(await files.save()).toBe(true);
		await files.refresh();
		expect(await files.save()).toBe(false);
		expect(files.replacing).toBe('Mine');
		expect(await files.save(true)).toBe(true);
		expect(analysis.sent.filter((s) => s.method === 'project.save_setup').map((s) => s.params.replace ?? false)).toEqual([false, true]);
	});

	it('names a member the draft would drop, add or change', () => {
		expect(configDifferences({ a: 1, b: { c: 2 }, d: [1] }, { a: 1, b: { c: 3 }, e: true })).toEqual([
			'b.c: 2 in the setup; this screen would send 3',
			'd: in the setup ([1]); this screen does not keep it',
			'e: not in the setup; this screen adds true'
		]);
		const v = vetSetup(asSetup('random', { a: 1 }, null), { module: 'random', protocol: 'sds/1', rows: [], roundTrip: () => ({ a: 2 }) });
		expect(v).toEqual({ refused: null, table: ['The setup does not say which channel table it was made with.'], config: ['a: 1 in the setup; this screen would send 2'] });
		expect(vetSetup(asSetup('random', { a: 1 }, []), { module: 'random', protocol: 'sds/1', rows: [], roundTrip: () => { throw new Error('no profile'); } }).refused).toBe('This screen cannot read the setup: no profile.');
	});
});

describe('the origin the Arm verb shows', () => {
	it('says typed, a saved setup by name, a run by its id, or a seed, until the operator edits', () => {
		let key = 'a';
		const o = new SetupOriginState(() => key);
		expect(o.words).toBe('Setup typed here');
		o.set({ kind: 'setup', name: 'Qualification' }, 'a');
		expect(o.words).toBe('Setup from the saved setup "Qualification"');
		o.set({ kind: 'run', run_uid: 'u-1', run_id: 'r-0007' }, 'a');
		expect(o.words).toBe('Setup from run r-0007');
		key = 'b';
		expect(o.edited).toBe(true);
		expect(o.words).toBe('Setup edited here, after opening run r-0007');
		expect(originWords({ kind: 'seed', object: 'Spec 1', checksum: 'ab', fills: 'profile' }, false)).toBe('Setup profile seeded from "Spec 1"');
	});

	it('is found from the store the run verbs are given, and kept across a reload', () => {
		const store = {};
		const memory = new Map<string, string>();
		const storage = { getItem: (k: string) => memory.get(k) ?? null, setItem: (k: string, v: string) => void memory.set(k, v) } as unknown as Storage;
		expect(knownOrigin(store)).toBeNull();
		const made = originFor(store, () => new SetupOriginState(() => 'k', storage, 'origin'));
		expect(knownOrigin(store)).toBe(made);
		made.set({ kind: 'setup', name: 'Kept' }, 'k');
		expect(new SetupOriginState(() => 'k', storage, 'origin').words).toBe('Setup from the saved setup "Kept"');
		expect(new SetupOriginState(() => 'other', storage, 'origin').words).toBe('Setup edited here, after opening the saved setup "Kept"');
	});
});
