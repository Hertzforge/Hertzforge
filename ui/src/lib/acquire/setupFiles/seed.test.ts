/**
 * Seeds through the shared setup bar, over each test's real store against the mock control service:
 * the seed `project.seed` answers for each seeded module (`schema/golden/seeds/<module>.json`, made
 * by the analysis side from a project object) fills the draft only after every member it changes
 * is named, and only once each protective number it gives has been typed; the draft is then the
 * seeded setup the control service ran on the simulator; nothing but dry runs (and, for TWR, the
 * reference the seed stores) reaches the control service; and the Arm line says which object, by
 * its SHA-256, the setup was seeded from.
 */
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import type { AppState } from '$lib/app/state.svelte';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory } from '$lib/protocol/mock/socket';
import type { ProjectSummary } from '$lib/protocol/types';
import { AcquireState } from '../store.svelte';
import { SineStore } from '../sine/sineStore.svelte';
import { MixedStore } from '../mixed/mixedStore.svelte';
import { ShockStore } from '../shock/shockStore.svelte';
import { SrsStore } from '../srs/srsStore.svelte';
import { TwrStore } from '../twr/twrStore.svelte';
import { applySets, SEED_MODULES, typedForm, typedMatches, vetSeed, type SeedAnswer } from './seed';
import { canonical, type SetupModule } from './setupFile';
import { SetupFiles, type AnalysisClient } from './setupFiles.svelte';
import { hostFor } from './setupHosts';
import { knownOrigin, originFor, SetupOriginState } from './setupOrigin.svelte';

type Config = Record<string, unknown>;

const GOLDEN = [resolve(process.cwd(), '../schema/golden'), resolve(process.cwd(), 'schema/golden')].find((p) => existsSync(p))!;
const seeded = (module: SetupModule) => JSON.parse(readFileSync(resolve(GOLDEN, `seeds/${module}.json`), 'utf8')) as { config: Config; base: Config; answer: SeedAnswer };

/** The analysis service, answering each module's seed as the analysis side made it. */
class FakeAnalysis implements AnalysisClient {
	isOpen = true;
	summary = { title: 'p', file: null, modified: false, objects: [] } as unknown as ProjectSummary;

	async request<R>(method: string, params: Record<string, unknown> = {}): Promise<R> {
		const module = params.module as SetupModule;
		if (method === 'project.seeds') {
			if (!SEED_MODULES.includes(module)) return { seeds: [] } as R;
			const a = seeded(module).answer;
			const tones = a.origin.also?.[0]?.object;
			return { seeds: [{ name: a.origin.object, kind: 'Specification', fills: a.origin.fills, holds: '', channels: ['1Z+'], ...(tones ? { tones } : {}) }] } as R;
		}
		if (method === 'project.seed') {
			const a = seeded(module).answer;
			if (params.name !== a.origin.object) throw new Error(`the project has no object named '${params.name}'`);
			if (a.origin.also && params.tones !== a.origin.also[0].object) throw new Error('the floor is read without its tones');
			return structuredClone(a) as R;
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
	const stores = { sine: new SineStore(acq, opts), mixed: new MixedStore(acq, opts), shock: new ShockStore(acq, opts), srs: new SrsStore(acq, opts), twr: new TwrStore(acq, opts) };
	await Promise.all(Object.values(stores).map((s) => s.sync()));
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
	return { asked, bar };
}

/** Anything that could configure, arm or start a run. */
const notChecks = (asked: { method: string; params: Record<string, unknown> }[]) =>
	asked.filter((a) => /\.(arm|start|fire|record|proceed)$/.test(a.method) || (/\.configure$/.test(a.method) && !a.params.dry_run));

const withoutReference = (c: Config | null) => {
	const out = structuredClone(c ?? {});
	delete out.reference;
	return out;
};

describe('a project object seeds a test setup', () => {
	for (const module of SEED_MODULES) {
		it(`${module}: fills the draft only after naming every change and the typed protective numbers`, async () => {
			const { asked, bar } = await shell();
			const files = bar(module);
			const host = files.host;
			const { base, config, answer } = seeded(module);
			host.fill(base);
			const before = host.key();
			expect(files.seedable).toBe(true);
			await files.refreshSeeds();
			expect(files.seedOffers!.map((o) => o.name)).toEqual([answer.origin.object]);

			const offer = files.seedOffers![0];
			expect(await files.chooseSeed(offer.name, offer.tones ? { tones: offer.tones } : {}), files.refused ?? '').toBe(true);
			const p = files.seedPending!;
			expect(host.key(), 'choosing a seed changes nothing').toBe(before);
			expect(p.vetting.changes.length).toBeGreaterThan(0);
			// the screen may spell out a default the seed leaves to the service; it drops and changes nothing
			expect(p.vetting.lost.filter((l) => !/: not in the setup; this screen adds /.test(l)), 'the screen holds the seed').toEqual([]);
			// every member the seed changes is named before anything changes
			const [was, will] = [host.roundTrip(base)!, host.roundTrip(config)!];
			for (const s of [...answer.sets, ...answer.protective]) {
				const top = s.path.split('.')[0];
				if (canonical(was[top]) !== canonical(will[top])) expect(p.vetting.changes.some((c) => c.startsWith(top)), `${module} names ${s.path}`).toBe(true);
			}

			if (answer.protective.length) {
				// not taken until each protective number is typed as given
				expect(await files.takeSeed()).toBe(false);
				expect(host.key()).toBe(before);
				for (const n of answer.protective) p.typed[n.path] = '99';
				expect(files.untyped.length).toBe(answer.protective.length);
				expect(await files.takeSeed()).toBe(false);
				for (const n of answer.protective) p.typed[n.path] = typedForm(n.value);
				expect(files.untyped).toEqual([]);
			}
			expect(await files.takeSeed(), files.refused ?? '').toBe(true);

			// the draft is the seeded setup the control service ran on the simulator
			expect(canonical(withoutReference(host.config()))).toBe(canonical(withoutReference(host.roundTrip(config))));
			if (module === 'twr') {
				const stored = asked.filter((a) => a.method === 'twr.reference.import');
				expect(stored.length).toBe(1);
				expect((host.config()!.reference as { sha256: string }).sha256).toMatch(/^[0-9a-f]{64}$/);
			} else expect(asked.some((a) => a.method === 'twr.reference.import')).toBe(false);
			expect(notChecks(asked)).toEqual([]);

			// the Arm line names the object and its SHA-256
			const words = `Setup with ${answer.origin.fills} seeded from "${answer.origin.object}" (SHA-256 ${answer.origin.checksum.slice(0, 12)})`;
			expect(files.originWords).toBe(words);
			expect(knownOrigin(host.store)!.words).toBe(words);
			expect(files.origin.recorded).toEqual(answer.origin);
		});
	}

	it('refuses a seed while the control service is not connected, or for another module', () => {
		const answer = seeded('random').answer;
		const ctx = { module: 'random' as SetupModule, protocol: 'sds/1', config: { title: 't' }, roundTrip: (c: Config) => c, canImport: false };
		expect(vetSeed(answer, { ...ctx, protocol: null }).refused).toMatch(/^Connect to the control service first/);
		expect(vetSeed(answer, { ...ctx, module: 'sine' }).refused).toMatch(/not this screen/);
		expect(vetSeed(answer, { ...ctx, config: null }).refused).toMatch(/makes no setup yet/);
		expect(vetSeed(seeded('twr').answer, { ...ctx, module: 'twr' }).refused).toMatch(/cannot store/);
		expect(vetSeed(answer, ctx).refused).toBeNull();
	});

	it('offers no seed on a test no object fills', () => {
		for (const module of ['explorer', 'recorder', 'mimo_shock'] as SetupModule[]) expect(SEED_MODULES.includes(module)).toBe(false);
	});
});

describe('the seed parts', () => {
	it('puts each value at its path whole, as the analysis side does', () => {
		expect(
			applySets({ a: { b: [1, { c: 2 }] }, k: 1 }, [
				{ path: 'a.b.1.c', value: 3 },
				{ path: 'x.y', value: [1] },
				{ path: 'k', value: { z: 1 } }
			])
		).toEqual({ a: { b: [1, { c: 3 }] }, k: { z: 1 }, x: { y: [1] } });
	});

	it('takes a protective number only as it is given', () => {
		expect(typedForm([4, 5])).toBe('4, 5');
		expect(typedMatches('4, 5', [4, 5])).toBe(true);
		expect(typedMatches('4 5', [4, 5])).toBe(true);
		expect(typedMatches('5, 4', [4, 5])).toBe(false);
		expect(typedMatches('4', [4, 5])).toBe(false);
		expect(typedMatches('', [4, 5])).toBe(false);
		expect(typedMatches('6.0', 6)).toBe(true);
		expect(typedMatches('6.01', 6)).toBe(false);
	});
});
