/**
 * The test-type chooser's rules: what it offers, what it remembers, and the live-run rule, first
 * on their own and then against the mock control service with a run driven from another screen.
 */
import { describe, expect, it } from 'vitest';
import { ServiceClient } from '$lib/protocol/client.svelte';
import { createControlServer } from '$lib/protocol/mock/control';
import { mockSocketFactory, type MockServer } from '$lib/protocol/mock/socket';
import { defaultSetup } from './setup';
import { AcquireState } from './store.svelte';
import { liveTestOf, TEST_TYPES, TestTypeView, type LiveTest, type TestType } from './testTypes.svelte';

const until = async (cond: () => boolean, ms = 8000) => {
	const t0 = Date.now();
	while (!cond()) {
		if (Date.now() - t0 > ms) throw new Error('timed out waiting');
		await new Promise((r) => setTimeout(r, 5));
	}
};

/** A view over a live answer the test sets by hand. */
function viewOf(initial: LiveTest = undefined, types?: readonly TestType[]) {
	const src = { live: initial as LiveTest };
	const view = new TestTypeView(() => src.live, types);
	return { src, view };
}

describe('the test types', () => {
	it('lists the seven test types in order, each with one plain line, and only Random can be chosen', () => {
		expect(TEST_TYPES.map((t) => t.title)).toEqual(['Random', 'Sine', 'Classical Shock', 'SRS', 'TWR', 'Recorder', 'Explorer']);
		expect(TEST_TYPES.filter((t) => t.ready).map((t) => t.id)).toEqual(['random']);
		for (const t of TEST_TYPES) {
			expect(t.summary).toMatch(/^[A-Z][^\n]{20,100}\.$/);
			// What the test is, never a note to developers.
			expect(t.summary).not.toMatch(/\b[QM]\d+\b|#\d+|todo|tbd|port|issue|milestone/i);
		}
		const { view } = viewOf(null);
		expect(TEST_TYPES.filter((t) => view.canChoose(t.id)).map((t) => t.id)).toEqual(['random']);
		expect(TEST_TYPES.filter((t) => !t.ready).every((t) => view.whyNot(t.id) === 'Coming')).toBe(true);
	});
});

describe('the chooser', () => {
	it('opens on the chooser once the service says nothing is live, and keeps the choice for the session', () => {
		const { src, view } = viewOf();
		// Opening, with no answer yet: neither screen is shown.
		view.open();
		expect(view.deciding).toBe(true);
		expect(view.screen).toBeNull();
		src.live = null;
		expect(view.screen).toBe('chooser');
		view.settle(null);
		expect(view.screen).toBe('chooser');
		expect(view.last).toBeNull();

		// A test type still to come is refused, and the chooser stays.
		expect(view.choose('sine')).toBe(false);
		expect(view.screen).toBe('chooser');
		expect(view.takeFocus()).toBe(false);

		// Random opens its screen, which takes the keyboard focus once.
		expect(view.choose('random')).toBe(true);
		expect(view.screen).toBe('random');
		expect(view.last).toBe('random');
		expect(view.takeFocus()).toBe(true);
		expect(view.takeFocus()).toBe(false);

		// Away to another workspace and back: still Random.
		view.open();
		expect(view.screen).toBe('random');
		expect(view.takeFocus()).toBe(false);

		// Back to the chooser: it remembers Random, and the focus goes to its card.
		expect(view.canLeave).toBe(true);
		expect(view.toChooser()).toBe(true);
		expect(view.screen).toBe('chooser');
		expect(view.last).toBe('random');
		expect(view.takeFocus()).toBe(true);
		view.open();
		expect(view.screen).toBe('chooser');
	});

	it('has no way back to the chooser from a live run, and opens straight to a live run', () => {
		const { src, view } = viewOf(null);
		view.settle(null);
		view.choose('random');
		src.live = 'random';
		expect(view.canLeave).toBe(false);
		expect(view.toChooser()).toBe(false);
		expect(view.screen).toBe('random');

		// The run ends: the way back returns.
		src.live = null;
		expect(view.toChooser()).toBe(true);
		expect(view.screen).toBe('chooser');

		// The workspace opens while a run is live: straight to the run.
		src.live = 'random';
		view.open();
		expect(view.screen).toBe('random');
		expect(view.toChooser()).toBe(false);

		// A screen that opens before the service answers goes to the run as soon as it is known.
		const fresh = viewOf();
		expect(fresh.view.deciding).toBe(true);
		fresh.src.live = 'random';
		expect(fresh.view.screen).toBe('random');
		fresh.view.settle('random');
		expect(fresh.view.screen).toBe('random');
		expect(fresh.view.last).toBe('random');
		expect(fresh.view.takeFocus()).toBe(false);
	});

	it('keeps a chooser on screen when a run goes live elsewhere, and returns to the run on request', () => {
		const { src, view } = viewOf(null);
		view.settle(null);
		src.live = 'random';
		view.settle('random');
		// No jump: the operator is on the chooser, and the run's card and the run pill return to it.
		expect(view.screen).toBe('chooser');
		expect(view.canChoose('random')).toBe(true);
		expect(view.showRun()).toBe(true);
		expect(view.screen).toBe('random');
		expect(view.takeFocus()).toBe(true);
		expect(view.toChooser()).toBe(false);
		src.live = null;
		expect(view.showRun()).toBe(false);
	});

	it('refuses another test type while a run is live, and says when it opens', () => {
		const types: TestType[] = TEST_TYPES.map((t) => ({ ...t, ready: t.id === 'random' || t.id === 'sine' }));
		const { src, view } = viewOf(null, types);
		view.settle(null);
		expect(view.choose('sine')).toBe(true);
		expect(view.toChooser()).toBe(true);
		src.live = 'random';
		expect(view.canChoose('sine')).toBe(false);
		expect(view.whyNot('sine')).toBe('Opens when the Random run ends.');
		expect(view.choose('sine')).toBe(false);
		expect(view.choose('random')).toBe(true);
		expect(view.screen).toBe('random');
	});

	it('reads a live run from the Random store, and nothing live from a service it cannot reach', () => {
		expect(liveTestOf({ run: null, syncError: null }, 'offline')).toBeUndefined();
		expect(liveTestOf({ run: null, syncError: null }, 'connecting')).toBeUndefined();
		expect(liveTestOf({ run: null, syncError: null }, 'open')).toBeUndefined();
		expect(liveTestOf({ run: null, syncError: 'refused' }, 'open')).toBeNull();
		expect(liveTestOf({ run: null, syncError: null }, 'retrying')).toBeNull();
		expect(liveTestOf({ run: null, syncError: null }, 'incompatible')).toBeNull();
	});
});

describe('the chooser against the control service', () => {
	async function screen(server: MockServer, name: string) {
		const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
		const acq = new AcquireState(client, { name, session: null, local: null, checkDelayMs: 0 });
		const view = new TestTypeView(() => liveTestOf(acq, client.status));
		const unwatch = view.watch();
		client.connect();
		await client.whenOpen();
		await acq.sync();
		return { client, acq, view, unwatch };
	}

	it('follows a run another screen drives: the chooser stays, the run opens, and a new screen goes straight to it', async () => {
		const server = createControlServer({ speed: 10 });
		const a = await screen(server, 'Shell on bench-pc');
		const b = await screen(server, 'Shell on laptop');
		await until(() => b.view.screen === 'chooser');
		expect(b.view.live).toBeNull();

		// A drives a run; B, on the chooser, sees it live and stays put.
		expect(a.view.choose('random')).toBe(true);
		expect(await a.acq.claim()).toBe(true);
		const d = defaultSetup();
		d.acquisition = { fs_hz: 8192, n_pts: 1024, n_avg: 2 };
		d.schedule = { mode: 'auto', levels: [{ db: 0, dwell_s: 0 }], full_level_s: 30 };
		d.control.review_after_sysid = false;
		a.acq.setup = d;
		expect(await a.acq.apply()).toBe(true);
		expect(await a.acq.start()).toBe(true);
		await until(() => b.acq.run?.state === 'running');
		await until(() => b.view.live === 'random');
		expect(b.view.screen).toBe('chooser');
		expect(a.view.toChooser()).toBe(false);
		expect(a.view.screen).toBe('random');

		// A third screen opens during the run: it never shows the chooser.
		const shown: (string | null)[] = [];
		const c = await (async () => {
			const client = new ServiceClient({ service: 'control', url: 'ws://mock/', socketFactory: mockSocketFactory(server, 1) });
			const acq = new AcquireState(client, { name: 'Shell on tablet', session: null, local: null, checkDelayMs: 0 });
			const view = new TestTypeView(() => liveTestOf(acq, client.status));
			const unwatch = view.watch();
			shown.push(view.screen);
			client.connect();
			await client.whenOpen();
			shown.push(view.screen);
			await acq.sync();
			shown.push(view.screen);
			return { client, acq, view, unwatch };
		})();
		await until(() => c.view.last === 'random');
		expect(shown[0]).toBeNull();
		expect(shown).not.toContain('chooser');
		expect(shown.at(-1)).toBe('random');
		expect(c.view.screen).toBe('random');
		expect(c.view.canLeave).toBe(false);

		// B returns to the run from the chooser.
		expect(b.view.choose('random')).toBe(true);
		expect(b.view.screen).toBe('random');

		// The run ends: every screen can go back to the chooser.
		expect(await b.acq.abort()).toBe(true);
		await until(() => a.acq.run?.state === 'finished' && c.acq.run?.state === 'finished');
		for (const s of [a, b, c]) {
			expect(s.view.canLeave).toBe(true);
			expect(s.view.toChooser()).toBe(true);
			s.unwatch();
			s.client.close();
		}
	});
});
