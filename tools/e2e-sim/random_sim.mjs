// Closed-loop Random on the simulator, end to end: the real control service and the shell, driven
// through the installed Chrome the way an operator drives them, with every check made on what the
// screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/random_sim.mjs [--shots <dir>] [--headed]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs three Random runs on the simulator, and stops both. It takes about three
// minutes, because the simulator runs in real time. It is not part of `npm --prefix ui test`.
//
// 0. Acquire opens on the test-type chooser, in three sections (Single Output, MIMO, Data
//    Acquisition), with Random, Classical Shock and MIMO Random offered and the keyboard moving across the
//    sections; each screen picks Random there.
//    The workspace rail minimizes and stays minimized across a reload.
// 1. One screen takes control, selects the simulator, sets the channel table (a control input, a
//    drive and a monitored input with a response limit) and the bench loopback setup, sees the
//    service draw the profile, applies, proceeds to the Run step, arms, starts, holds for review
//    (identification at the reference -20 dB, read from the run log) and proceeds. During the run:
//    the level card, the display pages (Control, Limits with the limit line, Responses), a plot
//    added and removed and a page renamed, added and removed while the run goes on untouched, a
//    connection of its own asking only for some plots and getting only those, and the rail
//    minimized. Then it steps the level on the manual ladder and stops with the ramp, which starts
//    at once and walks down smoothly (the header's live level, sampled ten times a second).
//    After the run, "Test type" leads back to the chooser and Random comes back as it was left.
// 1b. A page opened in a window of its own before the start, kept open through the run: the
//    first window still asks for exactly its own page and plots each time it changes page and
//    adds a plot (three times over), the new plot fills, and the second window keeps its page.
// 2. A second browser (its own storage) observes: it shows the setup the service holds and its
//    numbers; a new run starts and the observer aborts it with Ctrl and period. Meanwhile a third
//    screen resting on the chooser sees the run go live and returns to it from the top bar, and a
//    screen opened during the run goes straight to it. The controlling screen opens a page in a
//    window of its own (for another monitor): an observer with its own page and subscription.
// 2b. A new run, aborted from that second window.
// 3. A new run starts, and the controlling screen is closed while it drives: the observer sees
//    control free and the countdown, and the run ends in the safe stop after the orphan window.

import { chromium } from 'playwright-core';
import { execFileSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const args = process.argv.slice(2);
const flag = (name) => {
	const i = args.indexOf(name);
	return i >= 0 ? args[i + 1] : undefined;
};
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-sim'));
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });

// ---- the bench plan's first run: profile and acquisition --------------------------------------
const TITLE = 'Bench loopback, flat 20 to 2000 Hz';
const PSD = '2e-5';

// ---- processes --------------------------------------------------------------------------------
const children = [];
const stopAll = () => {
	for (const c of children) if (c.exitCode === null) c.kill('SIGTERM');
};
process.on('exit', stopAll);
process.on('SIGINT', () => process.exit(130));

function freePort() {
	return new Promise((resolve, reject) => {
		const s = net.createServer();
		s.unref();
		s.on('error', reject);
		s.listen(0, '127.0.0.1', () => {
			const { port } = s.address();
			s.close(() => resolve(port));
		});
	});
}

function start(name, cmd, argv, opts = {}) {
	const child = spawn(cmd, argv, { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'], ...opts });
	const log = [];
	const keep = (b) => {
		log.push(b.toString());
		if (log.length > 200) log.shift();
	};
	child.stdout.on('data', keep);
	child.stderr.on('data', keep);
	child.on('exit', (code) => {
		if (code !== 0 && code !== null) console.error(`${name} exited ${code}\n${log.join('')}`);
	});
	children.push(child);
	return { child, log };
}

async function waitFor(what, test, ms = 60_000) {
	const t0 = Date.now();
	for (;;) {
		try {
			if (await test()) return;
		} catch {
			/* not yet */
		}
		if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
		await new Promise((r) => setTimeout(r, 200));
	}
}

// ---- the service, directly ---------------------------------------------------------------------
/** One read-only request on a connection of its own (it observes; it never claims control). */
function serviceCall(port, method, params = {}) {
	return new Promise((resolve, reject) => {
		const ws = new WebSocket(`ws://127.0.0.1:${port}/`);
		const timer = setTimeout(() => {
			ws.close();
			reject(new Error(`no answer to ${method}`));
		}, 10_000);
		ws.onmessage = (m) => {
			if (typeof m.data !== 'string') return;
			const v = JSON.parse(m.data);
			if (v.type === 'hello') ws.send(JSON.stringify({ type: 'request', id: 1, method, params }));
			else if (v.type === 'response' && v.id === 1) {
				clearTimeout(timer);
				ws.close();
				if (v.error) reject(new Error(`${method}: ${v.error.message}`));
				else resolve(v.result);
			}
		};
		ws.onerror = () => reject(new Error(`cannot reach the service for ${method}`));
	});
}

/**
 * A connection of its own that subscribes to `plots` at `rate` and records every event frame for
 * `ms`: the topic and, for binary frames, the JSON header (arrays stay as references).
 */
function subscribeProbe(port, plots, rate, ms) {
	return new Promise((resolve, reject) => {
		const ws = new WebSocket(`ws://127.0.0.1:${port}/`);
		ws.binaryType = 'arraybuffer';
		const frames = [];
		let answer = null;
		let t0 = 0;
		ws.onmessage = (m) => {
			let v;
			if (typeof m.data === 'string') v = JSON.parse(m.data);
			else {
				const bytes = new Uint8Array(m.data);
				const len = new DataView(m.data).getUint32(4, true);
				v = JSON.parse(new TextDecoder().decode(bytes.subarray(8, 8 + len)));
			}
			if (v.type === 'hello') ws.send(JSON.stringify({ type: 'request', id: 1, method: 'random.subscribe', params: { rate_hz: rate, plots } }));
			else if (v.type === 'response' && v.id === 1) {
				if (v.error) return reject(new Error(`random.subscribe: ${v.error.message}`));
				answer = v.result;
				t0 = Date.now();
				setTimeout(() => {
					ws.close();
					resolve({ answer, frames, seconds: (Date.now() - t0) / 1000 });
				}, ms);
			} else if (v.type === 'event' && t0) frames.push({ topic: v.topic, data: v.data });
		};
		ws.onerror = () => reject(new Error('cannot reach the service for random.subscribe'));
	});
}

// ---- checks -----------------------------------------------------------------------------------
const results = [];
function check(name, ok, detail = '') {
	results.push({ name, ok, detail });
	console.log(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`);
	if (!ok) throw new Error(`check failed: ${name} ${detail}`);
}

const tid = (page, id) => page.locator(`[data-testid="${id}"]`).first();
const stateOf = (page) => tid(page, 'run-state').getAttribute('data-state');
const outcomeOf = (page) => tid(page, 'run-state').getAttribute('data-outcome');
const roleOf = (page) => tid(page, 'session-role').getAttribute('data-role');
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);

async function shot(page, name) {
	const file = path.join(SHOTS, `${name}.png`);
	await page.screenshot({ path: file });
	console.log(`      screenshot ${file}`);
}

/** Every check of the data source: the simulator is never shown as LIVE. */
async function simBadge(page, where) {
	const live = await page.locator('[data-testid="live-badge"]').count();
	const sim = await tid(page, 'sim-badge').isVisible();
	check(`${where}: the badge says SIM, never LIVE`, sim && live === 0, `sim ${sim}, live ${live}`);
}

async function fill(page, selector, value) {
	const el = page.locator(selector).first();
	await el.fill(String(value));
	await el.blur();
}

/** Click a run bar verb once it is enabled. */
async function verb(page, id, ms = 30_000) {
	const b = tid(page, id);
	await waitFor(`${id} enabled`, async () => (await b.isVisible()) && (await b.getAttribute('aria-disabled')) === 'false', ms);
	await b.click();
}

/** The test-type chooser, once the control service has answered. */
async function chooser(page, what) {
	await until(page, what, async (p) => (await tid(p, 'conn-control').getAttribute('data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
}

/** A setup section; from the Run step, back to the setup first. */
async function tab(page, id) {
	if ((await tid(page, 'flow-setup').getAttribute('aria-current')) !== 'step') await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

/** The page shown in a window's display. */
const pageName = async (page) => (await page.locator('[data-testid="page-tabs"] [role="tab"][aria-selected="true"]').first().innerText()).trim();
/** The pages' names, in the order of their tabs. */
const pageNames = async (page) => (await page.locator('[data-testid="page-tabs"] [role="tab"]').allInnerTexts()).map((t) => t.trim());
async function choosePage(page, name) {
	await page.locator('[data-testid="page-tabs"]').getByRole('tab', { name, exact: true }).click();
	await until(page, `the ${name} page`, async (p) => (await pageName(p)) === name);
}
/** An item of the page's menu, at the right of the page tabs. */
async function pageMenu(page, id) {
	await tid(page, 'page-menu').click();
	await tid(page, id).click();
}
/** A plot of a type added to the page, by its name in `+ Plot`. */
async function addPlot(page, words) {
	await tid(page, 'plot-add').click();
	await page.locator('[data-testid="plot-add-pop"]').getByRole('menuitem', { name: words, exact: true }).click();
}
/** The drawn panes of a plot, by its test id. */
const panes = (page, plotId) => page.locator(`[data-testid="plot-${plotId}"] [data-testid="plot-pane"] canvas`).count();
/** The plot ids on the page a window shows, in order. */
const plotsOnPage = (page) => page.locator('[data-testid="page-view"] [data-plot]').evaluateAll((els) => els.map((e) => e.dataset.plot));

/**
 * Every `random.subscribe` a page sends, as the list of plot ids it asks for, in order. Set up
 * before the page opens its connection.
 */
function subscriptionsOf(page) {
	const sent = [];
	page.on('websocket', (ws) =>
		ws.on('framesent', (f) => {
			if (typeof f.payload !== 'string') return;
			try {
				const v = JSON.parse(f.payload);
				if (v.method === 'random.subscribe') sent.push((v.params.plots ?? []).map((p) => p.id).join());
			} catch {
				/* not a request */
			}
		})
	);
	return sent;
}

/** The run bar's level, settled (no ramp "to" part), in dB. */
async function settledLevel(page) {
	const t = await text(page, 'run-level');
	if (/ to /.test(t)) return null;
	const m = t.match(/^([+-]?\d+(?:\.\d+)?) dB$/);
	return m ? Number(m[1]) : null;
}

// ---- the run ----------------------------------------------------------------------------------
console.log('building the control service (release)');
execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
const servicePort = await freePort();
const shellPort = await freePort();
const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
const service = start('sds-control', exe, ['--port', String(servicePort)]);
await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], {
	shell: process.platform === 'win32'
});
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${servicePort}/&theme=light#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`service ws://127.0.0.1:${servicePort}/, shell ${URL_}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const viewport = { width: 1440, height: 1000 };
const ctxA = await browser.newContext({ viewport });
const ctxB = await browser.newContext({ viewport });
let failed = null;

try {
	// ============ Run 1: the controller =========================================================
	const a = await ctxA.newPage();
	const subsA = subscriptionsOf(a);
	await a.goto(URL_);
	await chooser(a, 'the test-type chooser');
	const cards = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) =>
		els.map((e) => ({
			id: e.dataset.type,
			group: e.closest('[data-testid^="test-group-"]')?.dataset.testid.replace('test-group-', ''),
			offered: e.getAttribute('aria-disabled') === 'false',
			text: e.innerText.replace(/\s+/g, ' ')
		}))
	);
	const sections = await a.locator('[data-testid^="test-group-"] h2').allInnerTexts();
	// A test type still to come cannot be opened (forced past the browser's own refusal to click it).
	await tid(a, 'test-type-mimo_shock').click({ force: true });
	const stayed = await tid(a, 'test-type-chooser').isVisible();
	const inGroup = (g) => cards.filter((c) => c.group === g).map((c) => c.id).join();
	check(
		'Acquire opens on the test-type chooser: Single Output, MIMO and Data Acquisition sections, Random, Sine, Classical Shock, SRS, TWR, Explorer, MIMO Random and Recorder offered, the rest "Coming"',
		sections.map((t) => t.toLowerCase()).join('|') === 'single output|mimo|data acquisition' &&
			inGroup('single') === 'random,sine,shock,srs,twr,explorer' &&
			inGroup('mimo') === 'mimo_random,mimo_shock,mimo_sine,mimo_mixed,mimo_twr' &&
			inGroup('acquisition') === 'recorder' &&
			cards.filter((c) => c.offered).map((c) => c.id).join() === 'random,sine,shock,srs,twr,explorer,mimo_random,recorder' &&
			cards.filter((c) => !c.offered).every((c) => /\bcoming\b/i.test(c.text)) &&
			stayed,
		`${sections.join(' | ')}: ${cards.map((c) => `${c.id} ${c.offered ? 'offered' : 'coming'}`).join(', ')}`
	);
	// The keyboard moves across the sections: End to the last card, Home to the first, and the arrow
	// from the last Single Output card to the first MIMO card.
	await tid(a, 'test-type-random').focus();
	const focused = () => a.evaluate(() => document.activeElement?.getAttribute('data-type'));
	await a.keyboard.press('End');
	const atEnd = await focused();
	await a.keyboard.press('Home');
	const atHome = await focused();
	await tid(a, 'test-type-explorer').focus();
	await a.keyboard.press('ArrowRight');
	const across = await focused();
	await a.keyboard.press('ArrowLeft');
	const back = await focused();
	check('chooser keyboard: End, Home and the arrows move across the sections', atEnd === 'recorder' && atHome === 'random' && across === 'mimo_random' && back === 'explorer', `End ${atEnd}, Home ${atHome}, right of Explorer ${across}, back ${back}`);
	await shot(a, '00-test-type-chooser');

	// The workspace rail minimizes, giving the workspace the width, and stays so across a reload.
	const railWidth = (p) => p.evaluate(() => document.querySelector('nav.rail')?.getBoundingClientRect().width ?? 0);
	const wideBefore = await a.evaluate(() => document.querySelector('.workspace')?.getBoundingClientRect().width ?? 0);
	const railBefore = await railWidth(a);
	await tid(a, 'rail-toggle').click();
	const railGone = await railWidth(a);
	const wideAfter = await a.evaluate(() => document.querySelector('.workspace')?.getBoundingClientRect().width ?? 0);
	await a.reload();
	await chooser(a, 'the chooser after a reload');
	const keptGone = (await railWidth(a)) === 0 && (await tid(a, 'rail-toggle').getAttribute('aria-expanded')) === 'false';
	check('the rail minimizes, the workspace takes its width, and it stays minimized across a reload', railBefore > 0 && railGone === 0 && wideAfter > wideBefore && keptGone, `rail ${railBefore} to ${railGone} px, workspace ${wideBefore} to ${wideAfter} px, after reload ${keptGone ? 'still minimized' : 'shown'}`);
	await tid(a, 'rail-toggle').click();
	check('the rail comes back', (await railWidth(a)) === railBefore);
	await tid(a, 'test-type-random').click();
	await until(a, 'the shell to connect', async (p) => (await roleOf(p)) === 'observer');
	check('a new screen starts as an observer', (await roleOf(a)) === 'observer');
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await roleOf(p)) === 'controller');
	check('Take Control: this screen holds control', true);

	// Station: the simulator.
	await tab(a, 'station');
	await tid(a, 'station-sim').click();
	// "Use This Station" is offered only when the choice differs from the station in use.
	if ((await a.locator('[data-testid="station-use"]').count()) > 0) await tid(a, 'station-use').click();
	await until(a, 'the simulator in use', async (p) => /simulat/i.test(await text(p, 'station-current')));
	check('station: the simulator is in use', true, await text(a, 'station-current'));
	await simBadge(a, 'station');

	// Channel table: one control input (1 g reads as 1 V, abort at 1.0 g), one drive (1 V range).
	// A row for each of the simulator's eight inputs and its output: the service's control and drive rows on, the rest off.
	const shown = await a.locator('[data-testid^="channel-row-"]').count();
	const on = await a.locator('[data-testid^="channel-on-"]').evaluateAll((els) => els.map((e) => e.checked));
	check('channel table: a row per simulator channel, the control and drive rows on', shown === 9 && on.join() === [true, true, ...Array(7).fill(false)].join(), `${shown} rows, on ${on.join()}`);
	await fill(a, '#ch-0-sens', 1000);
	await fill(a, '#ch-0-range', 5);
	await fill(a, '#ch-1-range', 1);
	await tid(a, 'channel-more-0').click();
	const abortOff = a.locator('.nf', { has: a.locator('#ch-0-ab-g') }).locator('input[type="checkbox"]');
	if (await abortOff.isChecked()) await abortOff.uncheck();
	await fill(a, '#ch-0-ab-g', 1.0);
	// A monitored input, channel 3, for the response plots and a limit.
	await a.locator('[data-testid="channel-on-2"]').check({ force: true });
	await a.getByLabel('Channel 3 description').fill('Fixture');
	await fill(a, '#ch-2-sens', 1000);
	await fill(a, '#ch-2-range', 5);
	await until(a, 'the table check', async (p) => (await tid(p, 'table-apply').isEnabled()));
	await tid(a, 'table-apply').click();
	await until(a, 'the table applied', async (p) => /applied \(revision \d+\)/.test(await text(p, 'note')));
	check('channel table applied', true, await text(a, 'note'));
	await shot(a, '01-station-and-channels');

	// Profile and acquisition from the bench plan's first run; its schedule.
	await tab(a, 'test');
	await fill(a, '[data-testid="setup-title"]', TITLE);
	while ((await a.locator('[data-testid^="bp-row-"]').count()) > 2) {
		const n = await a.locator('[data-testid^="bp-row-"]').count();
		await a.getByRole('button', { name: `Remove row ${n}`, exact: true }).click();
	}
	await fill(a, '#bp-0-f', 20);
	await fill(a, '#bp-0-v', PSD);
	await tid(a, 'bp-1-psd').click();
	await fill(a, '#bp-1-f', 2000);
	await fill(a, '#bp-1-v', PSD);
	await fill(a, '#acq-fs', 12800);
	await a.locator('#acq-npts').selectOption('2048');
	await fill(a, '#acq-navg', 8);
	await tid(a, 'schedule-auto').click();
	await fill(a, '#lv-0-dw', 10);
	await fill(a, '#lv-1-dw', 10);
	await fill(a, '#lv-full', 30);

	// The control section: drive ceiling 0.3 V rms, abort 0.33, clip at 3 sigma, identification
	// at most 0.2 V rms over 4 averages, review after identification.
	await tab(a, 'control');
	await fill(a, '#ctl-drive_ceiling_vrms', 0.3);
	const driveAbortOff = tid(a, 'ctl-drive_abort_vrms-off');
	if (await driveAbortOff.isChecked()) await driveAbortOff.uncheck();
	await fill(a, '#ctl-drive_abort_vrms', 0.33);
	await fill(a, '#ctl-max_sigma', 3);
	await fill(a, '#ctl-pre-avg', 4);
	await fill(a, '#ctl-pre-max', 0.2);
	// A response limit on the monitored input, well above anything this run reaches.
	await tid(a, 'limit-add').click();
	await a.locator('#lim-0-ch').selectOption('3');
	await shot(a, '02-control-and-limits');

	// The profile, drawn by the service from the dry run, before anything is armed.
	// The editor checks a draft with the service a moment after the last change.
	await tab(a, 'test');
	await until(a, 'the dry run of the bench setup', async (p) => /Checked by the control service/.test(await text(p, 'setup-check')) && (await text(p, 'derived-grms')) === '0.199', 15_000);
	await until(a, 'the reference preview', async (p) => (await tid(p, 'reference-preview').getAttribute('data-source')) === 'dry-run');
	const plotted = await tid(a, 'reference-preview').locator('canvas, svg').count();
	check('Profile: the service draws the reference before arming', plotted > 0, `${plotted} drawn element(s), source dry run`);
	const grms = Number(await text(a, 'derived-grms'));
	check('derived: 0.199 g rms at full level', Math.abs(grms - 0.199) < 0.001, `${grms} g rms`);
	check('the run is not armed yet', ['unconfigured', 'configured'].includes(await stateOf(a)), await stateOf(a));
	await shot(a, '03-profile-preview-before-arming');

	await verb(a, 'act-apply');
	await until(a, 'applied', async (p) => (await tid(p, 'setup-status').getAttribute('data-status')) === 'applied');
	check('Apply Setup: the service holds this screen\'s setup', true, await text(a, 'setup-status'));
	check('after applying, the preview is the service\'s', (await tid(a, 'reference-preview').getAttribute('data-source')) === 'service');
	check('Setup step: the footer says the service holds this setup', /service holds this setup/.test(await text(a, 'flow-check')), await text(a, 'flow-check'));

	// Setup, then Run: Proceed to Run shows the run, and its Run card arms it; the Setup step
	// then shows the setup locked.
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the Run step', async (p) => (await tid(p, 'run-view').isVisible()) && (await tid(p, 'flow-run').getAttribute('aria-current')) === 'step');
	check('Proceed to Run: the Run step, on the Control page with its two plots', (await pageName(a)) === 'Control' && (await tid(a, 'page-view').getAttribute('data-plots')) === '2', `${await pageName(a)}, ${await tid(a, 'page-view').getAttribute('data-plots')} plots`);
	await verb(a, 'act-arm');
	await until(a, 'armed', async (p) => (await stateOf(p)) === 'armed');
	await simBadge(a, 'armed');
	await shot(a, '04-armed');
	await tid(a, 'flow-setup').click();
	await until(a, 'back at the setup', async (p) => (await tid(p, 'tab-station').isVisible()) && (await p.locator('[data-testid="run-view"]').count()) === 0);
	await tid(a, 'tab-test').click();
	const gridNote = await text(a, 'grid-note');
	check('Arm: armed, on the grid the station runs at', /Actual/.test(gridNote), gridNote);
	check('Setup step: armed, the footer says the setup is locked', /setup is locked/.test(await text(a, 'flow-check')), await text(a, 'flow-check'));
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the Run step again', async (p) => tid(p, 'run-view').isVisible());
	check('Setup and Run move between the steps', true);

	await verb(a, 'act-start');
	await until(a, 'the review hold', async (p) => (await stateOf(p)) === 'review', 90_000);
	check('Start: identification holds for review', await tid(a, 'review-hold').isVisible());
	// Identification excites relative to the reference: the run log states the control response
	// it reached, which must be the reference less 20 dB within the controller's 1 dB, and the
	// steps it took to get there from the starting drive.
	const sysLog = (await serviceCall(servicePort, 'random.log')).entries.map((e) => e.text);
	const steps = sysLog.filter((t) => t.startsWith('identification level step'));
	const at = sysLog.map((t) => t.match(/^identification at ([\d.]+) V rms: .* ([+-]\d+(?:\.\d+)?) dB against the reference/)).find(Boolean);
	check(
		'identification: the control response sits at the reference less 20 dB',
		at !== undefined && Math.abs(Number(at[2]) + 20) <= 1.0,
		at ? `${at[2]} dB at ${at[1]} V rms after ${steps.length} step(s)` : `no identification line in ${sysLog.length} log entries`
	);
	check('identification: the drive stayed within its 0.2 V rms limit', at !== undefined && Number(at[1]) <= 0.2, at ? `${at[1]} V rms` : '');
	await shot(a, '05-review-hold');
	await tid(a, 'review-proceed').click();

	await until(a, 'running', async (p) => (await stateOf(p)) === 'running');

	// The level card while the soft start ramps up to the first level, then holds it.
	const statuses = new Set();
	const t1 = Date.now();
	while (Date.now() - t1 < 8000) {
		statuses.add(await text(a, 'level-status'));
		if ([...statuses].some((t) => /^Controlling at -12\.0 dB/.test(t))) break;
		await a.waitForTimeout(150);
	}
	await until(a, 'the level card numbers', async (p) => /\d/.test(await text(p, 'level-target')) && /\d/.test(await text(p, 'level-control')) && /dB/.test(await text(p, 'level-error')));
	check(
		'level card: the soft start reads "Ramping to", then "Controlling at -12.0 dB"',
		[...statuses].some((t) => /^Ramping to -12\.0 dB/.test(t)) && [...statuses].some((t) => /^Controlling at -12\.0 dB/.test(t)),
		[...statuses].join(' | ')
	);
	check(
		'level card: the level, the target and control rms with their difference, and the time at the level against its dwell',
		/^-12\.0 dB$/.test(await text(a, 'level-now')) && (await tid(a, 'level-progress').isVisible()) && /of 10\.0 s at this level/.test(await text(a, 'level-progress')),
		`${await text(a, 'level-now')}; target ${await text(a, 'level-target')}, control ${await text(a, 'level-control')}, difference ${await text(a, 'level-error')}; ${await text(a, 'level-progress')}`
	);
	await until(a, 'the control plot drawn', async (p) => (await panes(p, 'pl-1')) > 0 && (await panes(p, 'pl-2')) > 0);
	// Every input reads OK, so the Inputs card says nothing of overload; its Range heading defines it.
	const inputsCard = await text(a, 'live-channels');
	check(
		'Inputs card: every input OK and no overload wording, the Range heading defining the word',
		/OK/.test(inputsCard) && !/overload/i.test(inputsCard) && /Overload once the input reaches the top of its range/.test((await tid(a, 'range-meaning').getAttribute('title')) ?? ''),
		inputsCard
	);
	await shot(a, '05a-run-control-page-and-level-card');

	// The manual ladder holds the level while the display is worked on.
	await tid(a, 'mode-manual').click();
	await until(a, 'the manual ladder', async (p) => (await tid(p, 'level-up').getAttribute('aria-disabled')) === 'false', 90_000);

	// A connection of its own asks for two plots only, at 10 per second, and gets only those.
	const probe = await subscribeProbe(servicePort, [{ id: 'lim', quantity: 'response', channels: 'limited' }, { id: 't1', quantity: 'time', channels: [1], points: 512 }], 10, 2000);
	const topics = new Set(probe.frames.map((f) => f.topic));
	const dataFrames = probe.frames.filter((f) => f.topic === 'random.data');
	const last = dataFrames.at(-1)?.data;
	const lim = last?.plots.find((x) => x.id === 'lim');
	const t1plot = last?.plots.find((x) => x.id === 't1');
	check(
		'subscription: only random.data, at about the rate asked, with exactly the plots asked for',
		!topics.has('random.live') && dataFrames.length >= 12 && dataFrames.length <= 26 && last?.plots.map((x) => x.id).join() === 'lim,t1',
		`${dataFrames.length} frames in ${probe.seconds.toFixed(1)} s, topics ${[...topics].join(', ')}`
	);
	check(
		'subscription: the limited channel with its limit line, and channel 1 as a time history',
		JSON.stringify(lim?.channels) === '[3]' &&
			lim.views[0].series.some((x) => x.role === 'limit' && x.name === 'Limit') &&
			JSON.stringify(t1plot?.channels) === '[1]' &&
			t1plot.views[0].x.label === 'Time',
		`limited ${JSON.stringify(lim?.channels)} series ${lim?.views[0]?.series.map((x) => x.role).join('/')}; time ${JSON.stringify(t1plot?.channels)} ${t1plot?.views[0]?.x.label}`
	);

	// The pages: Limits shows the limited channel against its limit; Responses every input.
	const runId = await serviceCall(servicePort, 'random.state').then((x) => x.run_id);
	await choosePage(a, 'Limits');
	await until(a, 'the limit plot', async (p) => (await panes(p, 'pl-3')) > 0 && /Limit/.test(await text(p, 'plot-pl-3')));
	check('Limits page: the limited channel drawn against its limit line', /Channel 3: Fixture/.test(await text(a, 'plot-pl-3')), (await text(a, 'plot-pl-3')).slice(0, 80));
	await shot(a, '05b-limits-page');
	await choosePage(a, 'Responses');
	await until(a, 'the response plots', async (p) => (await panes(p, 'pl-4')) === 2);
	check('Responses page: one pane per input (channels 1 and 3)', true);

	// A plot added during the run fills, and removed, goes; the run carries on untouched.
	const loopOf = async (p) => Number((await text(p, 'live-lines')).match(/Loop\s*(\d+)/)?.[1] ?? NaN);
	const loopBefore = await loopOf(a);
	await addPlot(a, 'Time History');
	await until(a, 'the added time plot', async (p) => (await tid(p, 'page-view').getAttribute('data-plots')) === '2' && (await panes(p, 'pl-5')) > 0);
	const midState = await serviceCall(servicePort, 'random.state');
	const loopAfter = await loopOf(a);
	check('a time-history plot, added during the run, fills at once; the run carries on', midState.run_id === runId && midState.state === 'running' && loopAfter > loopBefore, `${midState.run_id} ${midState.state}, loop ${loopBefore} to ${loopAfter}`);
	await shot(a, '05c-plot-added-during-run');
	await a.locator('[data-testid="plot-pl-5"] [data-testid="plot-remove"]').click();
	await until(a, 'the plot removed', async (p) => (await tid(p, 'page-view').getAttribute('data-plots')) === '1');
	const after = await serviceCall(servicePort, 'random.state');
	check('the plot removed during the run; the run carries on', after.run_id === runId && after.state === 'running', `${after.state}`);

	// Pages renamed, added (with a plot of its own) and removed during the run.
	await pageMenu(a, 'page-rename');
	await tid(a, 'page-name').fill('All Inputs');
	await tid(a, 'page-name').press('Enter');
	await until(a, 'the renamed page', async (p) => (await pageName(p)) === 'All Inputs');
	await tid(a, 'page-add').click();
	await tid(a, 'page-name').fill('Drive Detail');
	await tid(a, 'page-name').press('Enter');
	await until(a, 'the new page', async (p) => (await pageName(p)) === 'Drive Detail' && (await tid(p, 'page-empty').isVisible()));
	await addPlot(a, 'Drive');
	const newPlot = await a.locator('[data-testid="page-view"] [data-plot]').first().getAttribute('data-plot');
	await until(a, 'the drive plot on the new page', async (p) => (await panes(p, newPlot)) === 1);
	const pagesNow = await pageNames(a);
	check('pages: renamed, and a new one with a plot of its own, in the page tabs', pagesNow.join('|') === 'Control|Limits|All Inputs|Drive Detail', pagesNow.join(', '));
	await shot(a, '05d-new-page-with-a-plot');
	await pageMenu(a, 'page-remove');
	await tid(a, 'page-remove-yes').click();
	await until(a, 'the page removed', async (p) => (await pageNames(p)).length === 3);
	await choosePage(a, 'Control');
	check('the page removed; the run carries on', (await stateOf(a)) === 'running' && (await serviceCall(servicePort, 'random.state')).run_id === runId);

	// The rail minimized during the run gives the plots the screen.
	await tid(a, 'rail-toggle').click();
	await a.waitForTimeout(800);
	await shot(a, '05e-rail-minimized-during-run');
	await tid(a, 'rail-toggle').click();

	// The manual ladder: step the level.
	check('Manual ladder: the level steps are offered while driving', (await tid(a, 'level-up').getAttribute('aria-disabled')) === 'false');
	// At full level: down 1 dB, up 1 dB, then down to the -6 dB rung; every change ramps.
	await tid(a, 'level-goto').selectOption('0');
	await waitFor('full level', async () => (await settledLevel(a)) === 0, 30_000);
	check('Go to full level, 0 dB', true);
	await tid(a, 'level-down').click();
	await waitFor('one step down', async () => (await settledLevel(a)) === -1, 20_000);
	check('Level down 1 dB', true, '-1.0 dB');
	await tid(a, 'level-up').click();
	await waitFor('one step up', async () => (await settledLevel(a)) === 0, 20_000);
	check('Level up 1 dB', true, 'back to 0.0 dB');
	await tid(a, 'level-goto').selectOption('-6');
	await waitFor('-6 dB', async () => (await settledLevel(a)) === -6, 20_000);
	check('Go to level -6 dB', true);
	await a.waitForTimeout(4000); // a few control loops at the new level, for the picture
	await simBadge(a, 'driving');
	await shot(a, '06-manual-ladder-live');

	await tid(a, 'act-stop').click();
	// The live level from the click: the drive starts down at the next output block, not after the
	// control loop in progress (1.28 s here), and walks 40 dB over the 3 s ramp in small steps.
	const tStop = Date.now();
	const fall = [];
	while (Date.now() - tStop < 6000) {
		const m = (await text(a, 'run-level')).match(/^([+-]?\d+(?:\.\d+)?) dB/);
		const st = await stateOf(a);
		if (st === 'finished') break;
		if (m) fall.push([(Date.now() - tStop) / 1000, Number(m[1])]);
		await a.waitForTimeout(100);
	}
	const firstDown = fall.find(([, l]) => l < -6.5);
	check('Stop: the drive starts down within 0.6 s of the click', firstDown !== undefined && firstDown[0] < 0.6, firstDown ? `${firstDown[1]} dB at ${firstDown[0].toFixed(2)} s` : 'never below -6.5 dB');
	const levels = fall.map(([, l]) => l).filter((l, i, all) => i === 0 || l !== all[i - 1]);
	const worst = levels.slice(1).reduce((w, l, i) => Math.max(w, levels[i] - l), 0);
	check('Stop: the drive walks down in small steps, not loop-sized ones', levels.length >= 8 && worst < 8, `${levels.length} levels, largest step ${worst.toFixed(1)} dB, last ${levels.at(-1)} dB`);
	await until(a, 'stopping', async (p) => ['stopping', 'finished'].includes(await stateOf(p)), 10_000);
	const sawStopping = (await stateOf(a)) === 'stopping';
	await until(a, 'finished', async (p) => (await stateOf(p)) === 'finished', 30_000);
	check('Stop: ramps down, then ends by the operator', (await outcomeOf(a)) === 'operator_stop', `stopping seen ${sawStopping}, outcome ${await outcomeOf(a)}`);
	check('the end is shown in words', /Stopped by the Operator/.test(await text(a, 'run-end')));
	await shot(a, '07-stopped-ramped');

	// The run has ended: "Test type" leads back to the chooser, and Random comes back as it was left.
	await tid(a, 'test-type-back').click();
	await chooser(a, 'the chooser after the run');
	const lastUsed = /last used/i.test(await tid(a, 'test-type-random').innerText());
	await tid(a, 'test-type-random').click();
	await until(a, 'Random again', async (p) => (await stateOf(p)) === 'finished');
	check(
		'after the run, "Test type" returns to the chooser, and Random comes back as it was left',
		lastUsed && (await outcomeOf(a)) === 'operator_stop' && (await tid(a, 'run-view').isVisible()),
		`marked last used ${lastUsed}, outcome ${await outcomeOf(a)}`
	);

	// ============ Run 1b: a window opened before the start ======================================
	// Both windows keep the layout in this browser's storage, so an edit in one reaches the other.
	// Each must still ask the service for exactly the page it shows, whatever the other writes.
	await verb(a, 'act-reset');
	await until(a, 'a new run', async (p) => (await stateOf(p)) === 'configured');
	await choosePage(a, 'Limits');
	const [early] = await Promise.all([ctxA.waitForEvent('page'), pageMenu(a, 'page-popout')]);
	await early.setViewportSize({ width: 1280, height: 820 });
	await until(early, 'the window opened before the start', async (p) => (await tid(p, 'display-window').isVisible()) && (await roleOf(p)) === 'observer', 30_000);
	check(
		'a window opened on the Limits page before the start: an observer, with the run not yet started',
		(await text(early, 'display-window-page')) === 'Limits' && (await stateOf(early)) === 'configured',
		`${await text(early, 'display-window-page')}, ${await roleOf(early)}, ${await stateOf(early)}`
	);
	await verb(a, 'act-start');
	await until(a, 'the review hold', async (p) => (await stateOf(p)) === 'review', 90_000);
	await tid(a, 'review-proceed').click();
	await until(early, 'the window to draw its page during the run', async (p) => (await stateOf(p)) === 'running' && (await panes(p, 'pl-3')) > 0, 60_000);
	// The first window changes page and adds a time history, three times over; each time it asks
	// for its new page and plot at once, and the plot fills.
	for (let round = 1; round <= 3; round++) {
		const from = subsA.length;
		await choosePage(a, 'All Inputs');
		await addPlot(a, 'Time History');
		const onScreen = await plotsOnPage(a);
		const added = onScreen.at(-1);
		const filled = await until(a, 'the new page and plot asked for and drawn', async (p) => subsA.at(-1) === onScreen.join() && (await panes(p, added)) > 0, 5_000).then(
			() => true,
			() => false
		);
		check(
			`with the window open, change ${round} of 3: the first window asks for its new page and plot at once, and the plot fills`,
			filled && onScreen.length === 2,
			`on screen [${onScreen.join(', ')}]; asked for ${subsA.slice(from).map((s) => `[${s}]`).join(' then ') || 'nothing'}`
		);
		await a.locator(`[data-testid="plot-${added}"] [data-testid="plot-remove"]`).click();
		await choosePage(a, 'Limits');
		await until(a, 'the Limits page asked for again', async () => subsA.at(-1) === 'pl-3', 5_000);
	}
	check(
		'the window kept its own page and plot while the first window changed its own',
		(await text(early, 'display-window-page')) === 'Limits' && (await plotsOnPage(early)).join() === 'pl-3' && (await panes(early, 'pl-3')) > 0 && (await stateOf(early)) === 'running',
		`${await text(early, 'display-window-page')}, [${(await plotsOnPage(early)).join(', ')}], ${await stateOf(early)}`
	);
	await shot(early, '07b-window-opened-before-start');
	await verb(a, 'act-abort');
	await until(a, 'the run to end', async (p) => (await stateOf(p)) === 'finished', 10_000);
	await early.close();

	// ============ Run 2: a second browser observes, and aborts ==================================
	const b = await ctxB.newPage();
	await b.goto(URL_);
	await chooser(b, 'the chooser on the second screen');
	await tid(b, 'test-type-random').click();
	await until(b, 'the observer to connect', async (p) => (await roleOf(p)) === 'observer');
	// The observer line reads as one phrase: no gap before the comma (the role badge lays its
	// parts out with a gap, so the word and the holder must be one part of it).
	await until(b, 'the observer line to name the holder', async (p) => /has control/.test(await tid(p, 'session-role').innerText()));
	const seatLine = (await tid(b, 'session-role').innerText()).trim();
	check('observer: the seat line reads "Observing, <holder> has control"', /^Observing, \S.* has control$/.test(seatLine), JSON.stringify(seatLine));
	await tab(b, 'test');
	await until(b, 'the observer to read the setup back', async (p) => (await tid(p, 'setup-status').getAttribute('data-status')) === 'applied');
	check('second screen: shows the setup the service holds', (await b.locator('[data-testid="setup-title"]').inputValue()) === TITLE, await b.locator('[data-testid="setup-title"]').inputValue());
	check('second screen: the same derived numbers', Number(await text(b, 'derived-grms')) === grms, `${await text(b, 'derived-grms')} g rms`);
	check('second screen: the reference drawn by the service', (await tid(b, 'reference-preview').getAttribute('data-source')) === 'service');
	await shot(b, '08-observer-reads-setup');

	// A third screen rests on the test-type chooser while the next run goes live.
	const c = await ctxB.newPage();
	await c.goto(URL_);
	await chooser(c, 'the chooser on the third screen');

	await verb(a, 'act-reset');
	await until(a, 'a new run', async (p) => (await stateOf(p)) === 'configured');
	await verb(a, 'act-start');
	await until(a, 'the review hold', async (p) => (await stateOf(p)) === 'review', 90_000);
	await until(b, 'the observer to see the review hold', async (p) => (await stateOf(p)) === 'review');
	check('observer: Proceed is not offered to a screen without control', (await tid(b, 'review-proceed').isDisabled()));
	await tid(a, 'review-proceed').click();
	await until(b, 'the observer to see the run drive', async (p) => (await stateOf(p)) === 'running' && (await panes(p, 'pl-1')) > 0, 60_000);
	await simBadge(b, 'observer, driving');

	// The controlling screen opens its Limits page in a window of its own, for another monitor.
	await choosePage(a, 'Limits');
	const [w] = await Promise.all([ctxA.waitForEvent('page'), pageMenu(a, 'page-popout')]);
	await w.setViewportSize({ width: 1280, height: 820 });
	await until(w, 'the second window', async (p) => (await tid(p, 'display-window').isVisible()) && (await panes(p, 'pl-3')) > 0, 30_000);
	check(
		'second window: an observer showing the Limits page, with the run state, Stop and Abort',
		(await text(w, 'display-window-page')) === 'Limits' && (await roleOf(w)) === 'observer' && (await stateOf(w)) === 'running' && (await tid(w, 'act-stop').isVisible()) && (await tid(w, 'act-abort').getAttribute('aria-disabled')) === 'false',
		`${await text(w, 'display-window-page')}, ${await roleOf(w)}, ${await stateOf(w)}`
	);
	// Each window has its own page: the second goes to Control, the first stays on Limits.
	await choosePage(w, 'Control');
	await until(w, 'the control plot in the second window', async (p) => (await panes(p, 'pl-1')) > 0);
	await choosePage(a, 'All Inputs');
	await until(a, 'the first window on its own page', async (p) => (await panes(p, 'pl-4')) === 2);
	check('each window keeps its own page and plots', (await pageName(w)) === 'Control' && (await pageName(a)) === 'All Inputs');
	await shot(w, '08d-second-window-control-page');
	await shot(a, '08e-first-window-own-page');
	await choosePage(w, 'Limits');

	// The chooser stays on the third screen, with the run in reach from its card and the top bar.
	await until(c, 'the chooser to show the run', async (p) => (await tid(p, 'test-type-random').getAttribute('data-state')) === 'live' && (await tid(p, 'run-pill').isVisible()));
	check('a chooser on screen when a run goes live: the Random card and the top bar offer the run', await tid(c, 'test-type-chooser').isVisible(), `${await text(c, 'test-type-random')} | ${await text(c, 'run-pill')}`);
	await shot(c, '08b-chooser-during-live-run');
	await c.setViewportSize({ width: 720, height: 900 });
	await c.waitForTimeout(300); // the layout settles at the new size
	await shot(c, '08c-chooser-during-live-run-narrow');
	await tid(c, 'run-pill-go').click();
	await until(c, 'the run on the third screen', async (p) => (await stateOf(p)) === 'running');
	const count = (p, id) => p.locator(`[data-testid="${id}"]`).count();
	check(
		'the run pill returns to the live run, whose screen has no way back to the chooser',
		(await count(c, 'test-type-back')) === 0 && (await count(c, 'test-type-chooser')) === 0 && (await count(c, 'run-pill')) === 0
	);
	await c.close();

	// A screen opened during the live run goes straight to it: the chooser is never drawn.
	const d = await ctxB.newPage();
	await d.addInitScript(() => {
		window.__chooserSeen = false;
		new MutationObserver(() => {
			if (document.querySelector('[data-testid="test-type-chooser"]')) window.__chooserSeen = true;
		}).observe(document, { childList: true, subtree: true });
	});
	await d.goto(URL_);
	await until(d, 'the run on a screen opened during it', async (p) => (await stateOf(p)) === 'running');
	const seen = await d.evaluate(() => window.__chooserSeen);
	check('a screen opened during a live run goes straight to the run, never drawing the chooser', !seen && (await count(d, 'test-type-back')) === 0, `chooser drawn ${seen}`);
	await d.close();
	check('observer: Abort is offered', (await tid(b, 'act-abort').getAttribute('aria-disabled')) === 'false');
	await b.keyboard.press('Control+Period');
	await until(b, 'the abort', async (p) => (await stateOf(p)) === 'finished', 5_000);
	check('observer: Ctrl and period aborts at once', (await outcomeOf(b)) === 'operator_abort', await outcomeOf(b));
	await until(a, 'the controller to see the end', async (p) => (await stateOf(p)) === 'finished');
	const endMsg = await text(a, 'run-end-message');
	check('controller: the end names the observer', (await outcomeOf(a)) === 'operator_abort' && /observer/i.test(endMsg), endMsg);
	await shot(b, '09-observer-aborted');
	await shot(a, '10-controller-sees-observer-abort');

	// ============ Run 2b: the second window aborts ===============================================
	await verb(a, 'act-reset');
	await until(a, 'a new run', async (p) => (await stateOf(p)) === 'configured');
	await verb(a, 'act-start');
	await until(a, 'the review hold', async (p) => (await stateOf(p)) === 'review', 90_000);
	await tid(a, 'review-proceed').click();
	await until(w, 'the second window to see the run drive', async (p) => (await stateOf(p)) === 'running' && (await panes(p, 'pl-3')) > 0, 60_000);
	await until(w, 'its level card', async (p) => /^Controlling at|^Ramping to/.test(await text(p, 'level-status')));
	await shot(w, '09a-second-window-driving');
	await w.bringToFront();
	await w.keyboard.press('Control+Period');
	await until(w, 'the abort from the second window', async (p) => (await stateOf(p)) === 'finished', 5_000);
	await until(a, 'the first window to see it', async (p) => (await stateOf(p)) === 'finished');
	const wEnd = await text(a, 'run-end-message');
	check('second window: Ctrl and period aborts the run, and the end names it', (await outcomeOf(w)) === 'operator_abort' && /observer/i.test(wEnd), wEnd);
	await shot(w, '09b-second-window-aborted');
	await a.bringToFront();

	// ============ Run 3: the controlling screen goes away mid-run ===============================
	await verb(a, 'act-reset');
	await until(a, 'a new run', async (p) => (await stateOf(p)) === 'configured');
	await verb(a, 'act-start');
	await until(a, 'the review hold', async (p) => (await stateOf(p)) === 'review', 90_000);
	await tid(a, 'review-proceed').click();
	await until(b, 'the observer to see the run drive', async (p) => (await stateOf(p)) === 'running', 60_000);
	await a.close();
	await until(b, 'control free', async (p) => (await tid(p, 'claim').isEnabled()) && (await tid(p, 'orphan').isVisible()), 15_000);
	const orphan = await text(b, 'orphan');
	const left = Number(orphan.match(/in (\d+)\s*s/)?.[1]);
	check('observer: control is free and the countdown shows, within the 30 s window', left > 0 && left <= 30, orphan);
	await shot(b, '11-controller-gone-countdown');
	const t0 = Date.now();
	await until(b, 'the safe stop', async (p) => (await stateOf(p)) === 'finished', 60_000);
	const waited = (Date.now() - t0) / 1000;
	check('the run ends in the safe stop after the orphan window', (await outcomeOf(b)) === 'safe_stop', `outcome ${await outcomeOf(b)}, ${waited.toFixed(1)} s after the countdown showed`);
	check('the end is shown in words', /Nobody Held Control/.test(await text(b, 'run-end')));
	await simBadge(b, 'after the safe stop');
	await shot(b, '12-safe-stop');
} catch (err) {
	failed = err;
	console.error(err);
	for (const ctx of [ctxA, ctxB]) for (const p of ctx.pages()) await shot(p, `failure-${ctx === ctxA ? 'a' : 'b'}`).catch(() => {});
} finally {
	await browser.close();
	stopAll();
}

const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} checks passed${failed ? ', then one failed' : ''}; screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
