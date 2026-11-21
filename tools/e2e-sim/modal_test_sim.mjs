// The Modal Test on the simulator, end to end, driven through the installed Chrome the way an
// operator drives it, every check made on what the screen shows (and, against the real services,
// on the run folder the test left).
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/modal_test_sim.mjs [--mock] [--exe <sds-control>] [--python <python with hertzforge installed>]
//                                         [--data <dir>] [--shots <dir>] [--only impact|shaker] [--headed]
//
// With `--mock` it starts only the shell's dev server against the shell's own mock control service:
// the mock's numbers are made up so that every panel can be seen, and nothing it shows is a
// measurement. Without it, it runs the real control service (`--exe`, else it builds `sds-control`
// in release), the real analysis service (`python -m hertzforge serve`) and the shell's dev server,
// each on a free loopback port, and stops all three. Not part of `npm --prefix ui test`.
//
// 1. The chooser's Specialized section offers the Modal Test; it opens on Setup and takes control
//    on the simulator.
// 2. Station & Channels and DOFs: a hammer row (N, row 1) and two accelerometer rows (g, rows 3 and
//    4); in DOFs the hammer is the reference at 1Z+, the accelerometers responses at 2Z+ and 3Z+. A
//    setup with no reference is refused in DOFs, in the service's words.
// 3. Roving: row 4 roves over three points typed in; the plan comes from the service's check.
// 4. Impact: Positive polarity with the service's watches sentence, automatic accept, no
//    auto-advance, a force target the simulated hammer passes by a little.
// 5. Measure (impact): Space arms through Arming to Waiting for Hit; a simulated double hit is
//    rejected and flagged, its second impact marked on the force and worded with its ratio; an
//    overload is rejected; clean hits are accepted with a High flag; R rejects, A and Enter accept
//    again, U undoes; the point fills; the arrows change points; Clear Point asks, then clears; the
//    counter and tiles read the service after each. Double Hit and Overload are labeled inside the
//    force plot and Soft Tip in the force spectrum's header, at 1,440 by 900 and 1,280 by 800; every
//    legend is whole; every line of the Keys card and End Test are on screen at both sizes; End
//    Test ends it. The second (dark) run takes a 256-sample block: against the real services the
//    hit tile and the hit report say the response has not died away.
// 6. Against the real services: the run folder holds its setup and outcome; Open in Analysis opens
//    the run with its channel table, FRFs, coherence and frames; the modal fit opens on the FRFs and
//    one mode is found and confirmed; Estimate Again is offered on the frames with its glyph and
//    adds its FRFs and coherence.
// 7. Measure (shaker): choosing Shaker clears a plan that roves the hammer, in words. Two shakers
//    on rows 2 and 6, their force gauges on rows 1 and 5. A level over
//    the drive ceiling and pseudo random on two drives are each refused on screen in the service's
//    words; burst random checks clean; Start Driving; the averages build, each force, response and
//    drive drawn with its legend entry and every legend whole at both sizes; Stop Driving; End Test;
//    Open in Analysis brings the FRFs.
//
// `--only impact` or `--only shaker` runs one half. With MT_DEBUG set, a failure to find Estimate
// Again prints the page's errors and the analysis service's log.
// Screenshots (1,440 by 900): `modal_test_<name>.png`.

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
const MOCK = args.includes('--mock');
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-modal-test'));
const PYTHON = flag('--python') ?? process.env.HERTZFORGE_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3');
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });
const DATA_ROOT = MOCK ? null : flag('--data') ? path.resolve(flag('--data')) : fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-modal-test-data-'));
if (DATA_ROOT) fs.mkdirSync(DATA_ROOT, { recursive: true });

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

// ---- checks -----------------------------------------------------------------------------------
const results = [];
function check(name, ok, detail = '') {
	results.push({ name, ok, detail });
	console.log(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`);
	if (!ok) throw new Error(`check failed: ${name} ${detail}`);
}

const tid = (page, id) => page.locator(`[data-testid="${id}"]`).first();
const attr = (page, id, name) => tid(page, id).getAttribute(name);
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);
const field = (page, id) => page.locator(`input[data-testid="${id}"], [data-testid="${id}"] input`).first();
const measureState = (page) => attr(page, 'mt-measure', 'data-state');

async function shot(page, name) {
	const file = path.join(SHOTS, `modal_test_${name}.png`);
	const note = page.locator('[data-testid="note"] button[aria-label="Dismiss"]');
	if (await note.count()) await note.first().click().catch(() => {});
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await page.waitForTimeout(400);
	await page.screenshot({ path: file });
	console.log(`      screenshot ${file}`);
}

async function fill(locator, value) {
	await locator.fill(String(value));
	await locator.blur();
}

/** A switch or box set by its own click, wherever its panel has scrolled it. */
async function toggle(locator, on) {
	await locator.evaluate((el, want) => {
		if (el.checked !== want) el.click();
	}, on);
}

async function section(page, id) {
	if ((await attr(page, 'flow-setup', 'aria-current')) !== 'step' || !(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

async function neverLive(page, where) {
	check(`no LIVE badge (${where})`, (await page.locator('[data-testid="live-badge"]').count()) === 0);
}

/** A key press with nothing focused, as an operator with a hammer in one hand presses it. */
async function press(page, key) {
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await page.keyboard.press(key);
}

const tiles = (page) => page.locator('[data-testid="mt-tile"]').evaluateAll((els) => els.map((e) => ({ status: e.dataset.status, text: e.textContent.replace(/\s+/g, ' ').trim() })));

// ---- the services -------------------------------------------------------------------------------
const shellPort = await freePort();
let analysisLog = null;
/** With MT_DEBUG set: the analysis requests and replies after the fit, for a failure's report. */
const wire = [];
function listen(a) {
	if (!process.env.MT_DEBUG) return;
	a.on('pageerror', (e) => wire.push(`pageerror ${e.message}\n${(e.stack ?? '').slice(0, 1200)}`));
	a.on('console', (m) => m.type() === 'error' && wire.push(`console ${m.text().slice(0, 600)}`));
	a.on('websocket', (ws) => {
		if (!/analysis|:\d+\/$/.test(ws.url())) return;
		const keep = (dir) => (f) => {
			const t = typeof f.payload === 'string' ? f.payload : '';
			if (t.startsWith('{')) wire.push(`${dir} ${t.slice(0, 300)}`);
			if (wire.length > 400) wire.shift();
		};
		ws.on('framesent', keep('>'));
		ws.on('framereceived', keep('<'));
	});
}
let QUERY;
if (MOCK) {
	QUERY = '?mock=1&mock_speed=4&mock_sim=8x2';
} else {
	let exe = flag('--exe');
	if (!exe) {
		console.log('building the control service (release)');
		execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
		const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
		exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
	}
	const [controlPort, analysisPort] = [await freePort(), await freePort()];
	const env = { ...process.env, HERTZFORGE_DATA_ROOT: DATA_ROOT };
	const control = start('sds-control', exe, ['--port', String(controlPort)], { env });
	await waitFor('the control service', () => control.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
	analysisLog = start('analysis', PYTHON, ['-m', 'hertzforge', 'serve', '--port', String(analysisPort)], { env });
	await waitFor('the analysis service', () => new Promise((resolve) => {
		const s = net.connect(analysisPort, '127.0.0.1', () => {
			s.end();
			resolve(true);
		});
		s.on('error', () => resolve(false));
	}), 60_000);
	QUERY = `?control=ws://127.0.0.1:${controlPort}/&analysis=ws://127.0.0.1:${analysisPort}/`;
}
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], { shell: process.platform === 'win32' });
const URL_ = (theme) => `http://127.0.0.1:${shellPort}/${QUERY}&theme=${theme}#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`modal test: ${MOCK ? 'the mock control service' : `data ${DATA_ROOT}`}, shell ${URL_('light')}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
let failed = null;
let page = null;

/** The chooser, the Modal Test screen, control, and the simulator in use. */
async function open(theme) {
	const a = (page = await ctx.newPage());
	listen(a);
	await a.goto(URL_(theme));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()), 60_000);
	await tid(a, 'test-type-modal_test').click();
	await until(a, 'the Modal Test screen', async (p) => tid(p, 'modal-test-screen').isVisible());
	// control taken here, or (after a page that released it) taken already
	const mine = async (p) => (await attr(p, 'session-role', 'data-role').catch(() => null)) === 'controller';
	await until(a, 'control free to take', async (p) => (await mine(p)) || tid(p, 'claim').isEnabled(), 30_000);
	if (!(await mine(a))) await tid(a, 'claim').click();
	// the last page's test ended: a new run from the same setup first
	if ((await attr(a, 'run-state', 'data-state').catch(() => null)) === 'finished') {
		await tid(a, 'flow-run').click();
		await tid(a, 'act-reset').click();
		await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) !== 'finished', 20_000);
	}
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	await section(a, 'station');
	if ((await attr(a, 'station-sim', 'aria-checked')) !== 'true') await tid(a, 'station-sim').click();
	if ((await a.locator('[data-testid="station-use"]').count()) > 0 && (await tid(a, 'station-use').isEnabled())) await tid(a, 'station-use').click();
	await until(a, 'the simulator in use', async (p) => /simulat/i.test(await text(p, 'station-current')));
	return a;
}

/**
 * The table: row 1 the hammer (or the shaker's force gauge) in N, row 2 the drive, rows 3 and 4
 * accelerometers in g, every other row off. Then each DOF in DOFs.
 */
async function table(a, shaker) {
	await section(a, 'station');
	await until(a, 'the channel table', async (p) => (await p.locator('[data-testid^="channel-on-"]').count()) >= 4);
	const states = await a.locator('[data-testid^="channel-on-"]').evaluateAll((els) => els.map((e) => [Number(e.dataset.testid.replace('channel-on-', '')), e.checked]));
	for (const [i, checked] of states) {
		const on = i <= 3 || (shaker && i <= 5);
		if (checked !== on) await toggle(a.locator(`[data-testid="channel-on-${i}"]`), on);
	}
	for (const i of [2, 3]) await tid(a, `channel-role-${i}`).selectOption('monitor');
	await fill(a.getByLabel('Channel 1 description', { exact: true }), shaker ? 'Force gauge A' : 'Hammer');
	await fill(a.getByLabel('Channel 1 units', { exact: true }), 'N');
	for (const [i, d] of [[3, 'Accel A'], [4, 'Accel B']]) {
		await fill(a.getByLabel(`Channel ${i} description`, { exact: true }), d);
		await fill(a.getByLabel(`Channel ${i} units`, { exact: true }), 'g');
	}
	if (shaker) {
		// a second shaker: its force gauge on row 5, its drive on row 6
		await tid(a, 'channel-role-4').selectOption('monitor');
		await fill(a.getByLabel('Channel 5 description', { exact: true }), 'Force gauge B');
		await fill(a.getByLabel('Channel 5 units', { exact: true }), 'N');
		await tid(a, 'channel-role-5').selectOption('drive');
		await fill(a.getByLabel('Channel 6 description', { exact: true }), 'Drive B');
		await fill(a.getByLabel('Channel 6 units', { exact: true }), 'V');
	}
	await a.waitForTimeout(300);
	// the button is there only while the table has edits
	if ((await a.locator('[data-testid="table-apply"]').count()) && (await tid(a, 'table-apply').isEnabled())) await tid(a, 'table-apply').click();
}

async function dof(a, row, node, role) {
	const r = a.locator(`[data-testid="mt-dof-row"][data-row="${row}"]`);
	await r.locator('[data-testid="mt-dof-role"]').selectOption(role);
	await fill(r.locator('[data-testid="mt-dof-node"]'), node);
	await r.locator('[data-testid="mt-dof-direction"]').selectOption('Z+');
}

async function toRun(a) {
	await until(a, 'the setup to check clean', async (p) => (await attr(p, 'mt-ready', 'data-ready')) === 'true', 30_000);
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(a, 'act-start').click();
	await until(a, 'the test at its first point', async (p) => ['idle', 'point_done'].includes(await measureState(p)), 30_000);
}

/** Every line of the Keys card on screen, unclipped, with the page not scrolled. */
async function keysOnScreen(a) {
	return a.evaluate(() => {
		const lines = [...document.querySelectorAll('.keys dl > div')];
		const out = lines.map((el) => {
			const r = el.getBoundingClientRect();
			const at = document.elementFromPoint(r.left + Math.min(20, r.width / 2), r.top + r.height / 2);
			return { text: el.textContent.replace(/\s+/g, ' ').trim(), bottom: Math.round(r.bottom), seen: r.top >= 0 && r.bottom <= innerHeight && !!at && el.contains(at) };
		});
		return { lines: out, height: innerHeight, scrolled: (document.scrollingElement?.scrollTop ?? 0) > 0 };
	});
}

/** Record every wording of the state line from now on, so a short state is not missed. */
async function watchStateWords(a) {
	await a.evaluate(() => {
		const w = (window.__stateWords = []);
		const el = document.querySelector('[data-testid="mt-state-words"]');
		const keep = () => {
			const t = (el?.textContent ?? '').trim();
			if (w[w.length - 1] !== t) w.push(t);
		};
		keep();
		if (el) new MutationObserver(keep).observe(el, { subtree: true, childList: true, characterData: true });
		window.__stateTimer = setInterval(keep, 10);
	});
}
const stateWordsSeen = (a) => a.evaluate(() => (clearInterval(window.__stateTimer), window.__stateWords));

/** Each Measure plot's legend: its entries, and whether any is cut off by the plot's edge or its box. */
const legends = (a) => a.evaluate(() => [...document.querySelectorAll('.plots > .plot')].map((plot) => {
	const box = plot.getBoundingClientRect();
	const ul = plot.querySelector('ul.legend');
	const items = ul ? [...ul.querySelectorAll('li')] : [];
	const u = ul?.getBoundingClientRect();
	const cut = !!ul && (ul.scrollHeight > ul.clientHeight + 1 || items.some((li) => { const r = li.getBoundingClientRect(); return r.bottom > Math.min(box.bottom, u.bottom) + 0.5 || r.top < u.top - 0.5; }));
	return { title: plot.querySelector('.title')?.textContent?.trim() ?? '', entries: items.map((li) => li.querySelector('.name')?.textContent?.trim() ?? ''), cut };
}));
/** The Measure plots' titles cut short (an ellipsis): none should be. */
const titlesCut = (a) => a.evaluate(() => [...document.querySelectorAll('.plots > .plot .head .title')].filter((h) => h.scrollWidth > h.clientWidth + 1).map((h) => h.textContent.trim()));
const legendWords = (ls) => ls.map((l) => `${l.title}: ${l.entries.join(' + ')}${l.cut ? ' (cut)' : ''}`).join('; ');

/** The labels over the force trace: each inside the plot's drawing area, clear of the axes. */
const forceLabels = (a) => a.evaluate(() => [...document.querySelectorAll('[data-testid="mt-force-plot"] .area [role="note"]')].map((n) => {
	const r = n.getBoundingClientRect();
	const area = n.closest('.area').getBoundingClientRect();
	return { text: n.textContent.replace(/\s+/g, ' ').trim(), lines: r.height > parseFloat(getComputedStyle(n).fontSize) * 2.2 ? 2 : 1, inside: r.left >= area.left - 0.5 && r.right <= area.right + 0.5 && r.top >= area.top - 0.5 && r.bottom <= area.bottom + 0.5 };
}));

/** The shaker's time plots: the x span the screen sets, the last time a trace reaches, the burst's end and its marker and label. */
const timePlots = (a) => a.evaluate(() => ['mt-shaker-force-plot', 'mt-shaker-response-plot', 'mt-shaker-drive-plot'].map((id) => {
	const p = document.querySelector(`[data-testid="${id}"]`);
	if (!p) return { id, missing: true };
	const n = (k) => (p.dataset[k] === '' || p.dataset[k] === undefined ? null : Number(p.dataset[k]));
	const note = [...p.querySelectorAll('.area [role="note"]')].some((e) => {
		const r = e.getBoundingClientRect();
		const area = e.closest('.area').getBoundingClientRect();
		return /Burst\s*End/.test(e.textContent) && r.left >= area.left - 0.5 && r.right <= area.right + 0.5 && r.top >= area.top - 0.5 && r.bottom <= area.bottom + 0.5;
	});
	return { id, frame: n('frame'), end: n('xEnd'), last: n('xLast'), step: n('step'), burst: n('burst'), marker: !!p.querySelector('.marker'), note };
}));

/** Whether End Test is wholly on screen and on top where it is drawn. */
const endOnScreen = (a) => a.evaluate(() => {
	const b = document.querySelector('[data-testid="mt-end"]');
	if (!b) return { seen: false, at: 'none' };
	const r = b.getBoundingClientRect();
	const top = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
	return { seen: r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth && !!top && (top === b || b.contains(top)), at: `${Math.round(r.top)}-${Math.round(r.bottom)} of ${innerHeight}` };
});

const count = async (a) => Number(/^(\d+) of/.exec(await text(a, 'mt-count'))?.[1] ?? NaN);
const accepted = async (a) => (await tiles(a)).filter((t) => t.status === 'accepted').length;
const treeNames = (a) => a.locator('[data-testid="project-tree"] [data-testid^="object-"]').evaluateAll((els) => els.map((e) => e.dataset.testid.slice('object-'.length)));

/** Space to arm: one press, every time, even just after another key. */
async function arm(a) {
	const before = (await tiles(a)).length;
	await press(a, 'Space');
	const armed = await until(a, 'armed', async (p) => ['armed', 'arming'].includes(await measureState(p)) || (await tiles(p)).length > before, 3000).then(() => true, () => false);
	if (!armed) {
		lostSpace += 1;
		throw new Error(`a Space did not arm (the screen: ${await measureState(a)}; ${await text(a, 'mt-refusal').catch(() => 'no refusal')})`);
	}
}
let lostSpace = 0;

/** Arm, wait for `n` more tiles, disarm. */

async function hits(a, n, what) {
	const before = (await tiles(a)).length;
	await arm(a);
	await until(a, what, async (p) => (await tiles(p)).length >= before + n, 30_000).catch(async (e) => {
		throw new Error(`${e.message} (the screen: ${await measureState(a)}; ${await text(a, 'mt-refusal').catch(() => 'no refusal')})`);
	});
	await press(a, 'Space');
	await until(a, 'disarmed', async (p) => !['armed', 'arming'].includes(await measureState(p)), 10_000);
}

/** The impact test up to its shots; `full` runs every check, otherwise only what the shots need. */
async function impact(theme, full) {
	const a = await open(theme);
	check('the Modal Test opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');
	await table(a, false);

	await section(a, 'dofs');
	if (full) {
		await until(a, 'the check', async (p) => (await p.locator('[data-testid="mt-said"] [data-reason="no_reference"]').count()) > 0 || (await tid(p, 'mt-todo-dofs').count()) > 0, 20_000);
		check('a setup with no reference is refused in DOFs', (await tid(a, 'mt-todo-dofs').count()) === 1, await text(a, 'mt-summary').catch(() => ''));
	}
	await dof(a, 1, 1, 'reference');
	await dof(a, 3, 2, 'response');
	await dof(a, 4, 3, 'response');
	if (full) check('the DOFs wait in the table until it is applied', (await tid(a, 'mt-dofs-dirty').count()) === 1 || (await tid(a, 'mt-summary-table').count()) === 1);
	if (full) await shot(a, 'setup_dofs');

	// the second run takes a block too short for the response to die away in: the decay check warns
	await section(a, 'acquisition');
	await tid(a, 'mt-block').selectOption(full ? '2048' : '256');
	await section(a, 'roving');
	await toggle(tid(a, 'mt-rov-on'), true);
	await a.locator('[data-testid="mt-rov-row"][data-row="4"]').setChecked(true);
	await fill(tid(a, 'mt-rov-text'), '3 Z+\n4 Z+\n5 Z+');
	check('the typed points are read', (await text(a, 'mt-rov-count')) === '3 points read.', await text(a, 'mt-rov-count'));

	await section(a, 'excitation');
	await tid(a, 'mt-polarity-positive').click();
	await toggle(tid(a, 'mt-advance'), false);
	await toggle(tid(a, 'mt-target-on'), true);
	// the simulated hammer peaks at 20 in the row's unit: a target of 18 flags each clean hit High
	await fill(field(a, 'mt-target-peak'), 18);
	await fill(field(a, 'mt-target-tol'), 5);

	// Apply sends the table with its DOFs first, then the setup: the plan comes from the service.
	await until(a, 'the setup to check', async (p) => (await attr(p, 'mt-ready', 'data-ready')) === 'true' || (await tid(p, 'mt-summary-table').count()) === 1, 30_000);
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	if (full) {
		await section(a, 'roving');
		await until(a, 'the plan', async (p) => tid(p, 'mt-plan').isVisible(), 20_000);
		check('the plan has the three points from the service', (await a.locator('[data-testid="mt-plan"] tbody tr').count()) === 3);
		await shot(a, 'setup_roving_plan');
		await section(a, 'excitation');
		await until(a, 'the service windows', async (p) => tid(p, 'mt-fw-derived').isVisible(), 20_000);
		check("the polarity shows the service's watches sentence", /^The trigger watches .+\.$/.test(await text(a, 'mt-watches')), await text(a, 'mt-watches'));
		await shot(a, 'setup_impact');
		await tid(a, 'flow-run').click();
	}
	await tid(a, 'act-start').click();
	await until(a, 'the test at its first point', async (p) => (await measureState(p)) === 'idle', 30_000);
	check('the counter reads the service: 0 of 5 at point 1 of 3', (await text(a, 'mt-count')) === '0 of 5' && (await text(a, 'run-point')) === '1 of 3', `${await text(a, 'mt-count')}; ${await text(a, 'run-point')}`);

	// A double hit: rejected by the service, flagged, its second impact marked and worded.
	await tid(a, 'mt-sim-double').click();
	if (full) await watchStateWords(a);
	await hits(a, 1, 'the double hit');
	if (full) {
		const seen = await stateWordsSeen(a);
		const i = seen.indexOf('Arming');
		check('arming reads Arming, then Waiting for Hit', i >= 0 && seen.indexOf('Waiting for Hit') > i, seen.join(' > '));
	}
	const first = (await tiles(a))[0];
	check('the double hit is rejected, its flag in words', first.status === 'rejected' && /Double Hit/.test(first.text), first.text);
	check('the verdict says why in words', /Hit 1 Rejected: Double Hit/.test(await text(a, 'mt-verdict')), await text(a, 'mt-verdict'));
	if (full) {
		await until(a, 'the hit drawn', async (p) => (await p.locator('[data-testid="mt-force-plot"] .marker').count()) > 0, 10_000).catch(() => {});
		check('the second impact is marked on the force', (await a.locator('[data-testid="mt-force-plot"] .marker').count()) > 0);
		check('the second impact is worded with its ratio', /^Second impact at .+ s into the block, .+: [\d.]+% of the peak\.$/.test(await text(a, 'mt-second-words')), await text(a, 'mt-second-words').catch(() => 'none'));
		const labels = await forceLabels(a);
		check('Double Hit is labeled inside the force plot on one line, clear of its axes', labels.some((l) => l.text === 'Double Hit' && l.inside && l.lines <= 1), JSON.stringify(labels));
		const head = (await a.locator('[data-testid="mt-fspec-plot"] .head').innerText()).replace(/\s+/g, ' ');
		if (/Tip/.test(first.text)) check("Soft Tip is said in the force spectrum's header beside the tip roll-off", /Tip roll-off: [^,]+, Soft Tip/.test(head), head);
		check('the force window is drawn from the service', /window/i.test(await a.locator('[data-testid="mt-force-plot"]').innerText()), (await a.locator('[data-testid="mt-force-plot"]').innerText()).replace(/\s+/g, ' ').slice(0, 160));
		check('the force spectrum gives its tip roll-off', /Tip roll-off/.test(await a.locator('[data-testid="mt-fspec-plot"]').innerText()), (await a.locator('[data-testid="mt-fspec-plot"]').innerText()).replace(/\s+/g, ' ').slice(0, 160));
		const tc900 = await titlesCut(a);
		check('no plot title is cut short at 1440 by 900 (impact)', tc900.length === 0, tc900.join(', '));
		const l900 = await legends(a);
		check('every legend on the Measure screen is whole at 1440 by 900 (impact)', l900.length >= 3 && l900.every((l) => !l.cut), legendWords(l900));
		await shot(a, 'measure_impact');
		await a.setViewportSize({ width: 1280, height: 800 });
		await a.waitForTimeout(600);
		const l800 = await forceLabels(a);
		check('Double Hit stays inside the force plot at 1280 by 800', l800.some((l) => l.text === 'Double Hit') && l800.every((l) => l.inside), JSON.stringify(l800));
		const tc800 = await titlesCut(a);
		check('no plot title is cut short at 1280 by 800 (impact)', tc800.length === 0, tc800.join(', '));
		const g800 = await legends(a);
		check('every legend is whole at 1280 by 800 (impact)', g800.every((l) => !l.cut), legendWords(g800));
		await a.screenshot({ path: path.join(SHOTS, 'modal_test_measure_impact_1280x800.png') });
		await a.setViewportSize({ width: 1440, height: 900 });
		await a.waitForTimeout(400);

		// An overload: rejected, flagged.
		await tid(a, 'mt-sim-overload').click();
		await hits(a, 1, 'the overload');
		const second = (await tiles(a))[1];
		check('the overload is rejected, flagged in words inside the force plot', second.status === 'rejected' && /Overload/.test(second.text) && (await forceLabels(a)).some((l) => l.text === 'Overload' && l.inside), `${second.text}; ${JSON.stringify(await forceLabels(a))}`);
		check('the rejected count is the service count', (await text(a, 'mt-rejected')) === '2 rejected', await text(a, 'mt-rejected'));
	}

	// Clean hits: accepted, a High flag with its percent.
	await hits(a, 2, 'two clean hits');
	// the simulator's hammer peaks just over the target; the mock's are made up, so any target flag
	const flagRe = MOCK ? /High \+\d+%|Low −\d+%|On Target/ : /High \+\d+%/;
	check(`clean hits are accepted with ${MOCK ? 'a target flag' : 'a High flag and its percent'}`, (await tiles(a)).filter((t) => t.status === 'accepted' && flagRe.test(t.text)).length >= 2, (await tiles(a)).map((t) => t.text).join(' | '));
	check('the counter is the service count of accepted hits', (await count(a)) === (await accepted(a)), `${await text(a, 'mt-count')} with ${await accepted(a)} accepted tiles`);
	check('the target flag is shown with its percent', /High|Low|On Target/.test(await text(a, 'mt-target-flag').catch(() => '')), await text(a, 'mt-target-flag').catch(() => 'none'));
	if (!full && !MOCK) {
		// the mock does not make the decay check; the control service does
		const t = await tiles(a);
		check('a 256-sample block: the hit tile says the response has not died away', t.some((x) => /Not Died Away/.test(x.text)), t.map((x) => x.text).join(' | '));
		const rep = await text(a, 'mt-report');
		check('and the hit report names the row and how loud the block ends', /Row \d+ has not died away by the end of the block: it ends at [\d.]+% of its loudest\./.test(rep), rep);
	}
	if (!full) {
		await shot(a, 'dark_measure_impact');
		await tid(a, 'mt-end').click();
		await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-state')) === 'finished', 30_000);
		return a;
	}

	// The keys override the service's verdict on the latest hit; U undoes.
	const n = await count(a);
	const verdict = (s) => until(a, `the latest hit ${s}`, async (p) => (await attr(p, 'mt-verdict', 'data-status')) === s, 10_000);
	await press(a, 'r');
	await verdict('rejected');
	check('R rejects the latest hit', (await count(a)) === n - 1, await text(a, 'mt-count'));
	await press(a, 'a');
	await verdict('accepted');
	check('A accepts it again', (await count(a)) === n, await text(a, 'mt-count'));
	await press(a, 'r');
	await verdict('rejected');
	await press(a, 'Enter');
	await verdict('accepted');
	check('Enter accepts it again', (await count(a)) === n, await text(a, 'mt-count'));
	await hits(a, 1, 'one more clean hit');
	await press(a, 'r');
	await verdict('rejected');
	check('R rejects the next one, and it stays out', (await count(a)) === n && (await tiles(a)).at(-1).status === 'rejected', `${await text(a, 'mt-count')}; ${(await tiles(a)).at(-1).text}`);
	await press(a, 'u');
	await until(a, 'the undo', async (p) => (await count(p)) === n - 1, 10_000);
	check('U undoes the last accepted hit, its tile marked undone', (await tiles(a)).some((t) => t.status === 'undone'), (await tiles(a)).map((t) => t.status).join(', '));
	check('the counter matches the accepted tiles after each', (await count(a)) === (await accepted(a)), `${await text(a, 'mt-count')} with ${await accepted(a)}`);

	// The point fills with clean hits; the arrows change points; Clear Point asks first.
	await arm(a);
	await until(a, 'point 1 measured', async (p) => (await text(p, 'mt-count')) === '5 of 5', 60_000);
	await until(a, 'point 1 done', async (p) => (await measureState(p)) === 'point_done', 10_000);
	check('point 1 is measured at 5 of 5', /Measured/.test(await a.locator('[data-testid="mt-points-list"] li').first().innerText()));
	await press(a, 'ArrowRight');
	await until(a, 'the next point', async (p) => (await text(p, 'run-point')) === '2 of 3', 10_000);
	check('Right moves to the next point', (await text(a, 'mt-count')) === '0 of 5', await text(a, 'mt-count'));
	await hits(a, 2, 'two hits at point 2');
	const at2 = await count(a);
	await tid(a, 'mt-clear').click();
	check('Clear Point asks first, in words', /(Take all \d+ accepted hits|Take the accepted hit) at point 2 out\?/.test(await text(a, 'mt-clear-ask')), await text(a, 'mt-clear-ask').catch(() => 'none'));
	await tid(a, 'mt-clear-no').click();
	check('Keep Them keeps them', (await count(a)) === at2 && (await tid(a, 'mt-clear-ask').count()) === 0, await text(a, 'mt-count'));
	await tid(a, 'mt-clear').click();
	await tid(a, 'mt-clear-yes').click();
	await until(a, 'the point cleared', async (p) => (await count(p)) === 0, 10_000);
	check('Clear Point takes every accepted hit out and keeps the tiles, undone', (await tiles(a)).length >= 2 && (await accepted(a)) === 0, (await tiles(a)).map((t) => t.status).join(', '));
	await press(a, 'ArrowLeft');
	await until(a, 'the previous point', async (p) => (await text(p, 'run-point')) === '1 of 3', 10_000);
	check('Left moves back to the measured point', (await text(a, 'mt-count')) === '5 of 5', await text(a, 'mt-count'));

	// Every key line on screen without scrolling at 1440 by 900; then at 1280 by 800, as it is.
	const k900 = await keysOnScreen(a);
	check('every line of the Keys card is on screen at 1440 by 900', k900.lines.length >= 5 && k900.lines.every((l) => l.seen) && !k900.scrolled, k900.lines.map((l) => `${l.text}@${l.bottom}${l.seen ? '' : ' (cut)'}`).join('; '));
	await a.setViewportSize({ width: 1280, height: 800 });
	await a.waitForTimeout(400);
	const k800 = await keysOnScreen(a);
	check('every line of the Keys card is on screen at 1280 by 800', k800.lines.length >= 5 && k800.lines.every((l) => l.seen), k800.lines.map((l) => `${l.text}@${l.bottom}${l.seen ? '' : ' (cut)'}`).join('; '));
	const e800 = await endOnScreen(a);
	check('End Test is on screen at 1280 by 800', e800.seen, e800.at);
	await a.screenshot({ path: path.join(SHOTS, 'modal_test_keys_1280x800.png') });
	await a.setViewportSize({ width: 1440, height: 900 });

	await tid(a, 'mt-end').click();
	await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-state')) === 'finished', 30_000);
	check('End Test ends the test in words', (await tid(a, 'mt-ended').count()) === 1 && /\w/.test(await text(a, 'mt-ended')), await text(a, 'mt-ended').catch(() => ''));
	await neverLive(a, 'after the test');
	await shot(a, 'measure_ended');
	if (MOCK) return a;

	const run = latestRun('impact');
	check("the run's folder holds the test's setup and outcome", !!run && fs.existsSync(path.join(run, 'end.json')), run ?? 'none');
	await analysis(a, 'impact');
	await shot(a, 'in_analysis');

	// The modal fit on the run's FRFs: fit.start accepted, one mode found and confirmed.
	const frf = (await treeNames(a)).find((x) => /frf/i.test(x));
	await tid(a, `object-${frf}`).click();
	await until(a, 'the fit row', async (p) => tid(p, 'action-self.fit').isVisible(), 20_000);
	await tid(a, 'action-self.fit').click();
	await until(a, 'the fit panel', async (p) => tid(p, 'fit-panel').isVisible(), 60_000);
	check('the fit opens on the FRFs (fit.start accepted)', (await tid(a, 'fit-error').count()) === 0, frf);
	await tid(a, 'fit-find').click();
	await until(a, 'the cursor on a mode', async (p) => (await tid(p, 'fit-find').isEnabled()) && !(await p.locator('.fit .status [role="status"]').count()), 60_000);
	await tid(a, 'fit-confirm').click();
	await until(a, 'a confirmed mode', async (p) => (await tid(p, 'fit-mode-0').count()) === 1 || (await tid(p, 'fit-error').count()) === 1, 60_000);
	check('one mode is found', (await tid(a, 'fit-mode-0').count()) === 1, (await tid(a, 'fit-mode-0').count()) ? await text(a, 'fit-mode-0') : await text(a, 'fit-error'));
	await shot(a, 'fit_open');

	// Estimate Again on the first measurement's frames: its row with its glyph, then a new pair.
	const frames = (await treeNames(a)).find((x) => /frames/.test(x));
	const before = await treeNames(a);
	const objects = before.length;
	// a click in the tree leaves the fit; then the frames are picked on their own
	const row = a.locator('[data-testid="action-project.reprocess_modal_test"]').first();
	await tid(a, `object-${frames}`).click();
	await until(a, 'the fit left', async (p) => (await tid(p, 'fit-panel').count()) === 0, 20_000);
	const picks = [];
	await until(a, 'the Estimate Again row', async (p) => {
		if (await row.isVisible()) return true;
		await p.waitForTimeout(3000);
		if (await row.isVisible()) return true;
		await tid(p, 'object-Channel Table').click();
		await p.waitForTimeout(1500);
		await tid(p, `object-${frames}`).click();
		await p.waitForTimeout(2500);
		picks.push(await p.locator('[data-testid="action-column"]').first().innerText().catch(() => '').then((t) => t.replace(/\s+/g, ' ').slice(0, 80)));
		return row.isVisible();
	}, 45_000).catch((e) => {
		console.log(`      the action table after each pick: ${picks.join(' | ')}`);
		if (process.env.MT_DEBUG) {
			console.log(wire.filter((w) => /^(pageerror|console)/.test(w)).join('\n'));
			console.log((analysisLog?.log ?? []).join('').slice(-3000));
		}
		throw e;
	});
	if (picks.length) console.log(`      the Estimate Again row came after ${picks.length} more picks: ${picks.join(' | ')}`);
	check('Estimate Again is offered on the frames, with its glyph', /Estimate Again/.test(await row.innerText()) && (await row.locator('svg').count()) > 0, `${frames}: ${await row.innerText()}`);
	await row.click();
	await until(a, 'its form', async (p) => tid(p, 'run-op').isVisible(), 20_000);
	await tid(a, 'run-op').click();
	await until(a, 'the estimate', async (p) => (await treeNames(p)).length > objects, 120_000);
	const added = (await treeNames(a)).filter((x) => !before.includes(x));
	check('Estimate Again adds its FRFs and coherence beside the frames', added.length === 2, added.join(', '));
	return a;
}

/** Release control, so the next page can take it, and close the page. */
async function done(a) {
	if ((await a.locator('[data-testid="rail-acquire"][aria-current="page"]').count()) === 0) await tid(a, 'rail-acquire').click();
	const release = a.locator('[data-testid="release"]:visible').first();
	await until(a, 'the Release button', async () => (await release.count()) > 0 && release.isEnabled(), 20_000);
	await release.click();
	await until(a, 'control released', async (p) => (await attr(p, 'session-role', 'data-role')) !== 'controller', 20_000);
	await a.close();
}

/** The newest run folder of this mode under the data folder. */
function latestRun(mode) {
	const runs = fs.readdirSync(DATA_ROOT).filter((f) => /^modal_test/.test(f)).map((f) => path.join(DATA_ROOT, f))
		.filter((d) => { try { return JSON.parse(fs.readFileSync(path.join(d, 'setup.json'), 'utf8')).config?.mode === mode; } catch { return false; } });
	return runs.sort((x, y) => fs.statSync(y).mtimeMs - fs.statSync(x).mtimeMs)[0] ?? null;
}

/** Open in Analysis: the Analysis workspace, and the run's objects in the tree. */
async function analysis(a, mode) {
	await tid(a, 'act-open-analysis').click();
	const current = (p) => p.locator('[data-testid^="rail-"][aria-current="page"]').first().innerText().catch(() => '');
	await until(a, 'the Analysis workspace', async (p) => /Analysis/.test(await current(p)), 180_000);
	check(`Open in Analysis opens the run in the Analysis workspace (${mode})`, true);
	await until(a, 'the project tree', async (p) => (await treeNames(p)).length > 0, 30_000);
	const names = await treeNames(a);
	const has = (re) => names.some((x) => re.test(x));
	check(`the run arrives with its channel table, FRFs, coherence and time data (${mode})`, has(/channel table/i) && has(/frf/i) && has(/coherence/i) && has(/frames/), names.join(', '));
}

async function shaker(theme, full) {
	const a = await open(theme);
	await table(a, true);
	if (full) {
		// a roving hammer plan, left from an impact test, cannot stay in shaker mode: it is cleared, in words
		await section(a, 'acquisition');
		if ((await attr(a, 'mt-mode-impact', 'aria-checked')) !== 'true') await tid(a, 'mt-mode-impact').click();
		await section(a, 'dofs');
		await dof(a, 1, 1, 'reference');
		await section(a, 'roving');
		await toggle(tid(a, 'mt-rov-on'), true);
		for (const r of await a.locator('[data-testid="mt-rov-row"]').all()) await r.setChecked((await r.getAttribute('data-row')) === '1');
		await fill(tid(a, 'mt-rov-text'), '1\n2\n3');
	}
	await section(a, 'acquisition');
	await tid(a, 'mt-mode-shaker').click();
	if (full) {
		const note = await text(a, 'mt-roving-cleared').catch(() => 'none');
		check('choosing Shaker clears a plan that roves the hammer, and says so', /^The roving plan was cleared: it moved row 1, a reference/.test(note), note);
		await section(a, 'roving');
		check('the roving plan is off, the note in Roving too', !(await tid(a, 'mt-rov-on').isChecked()) && (await tid(a, 'mt-roving-cleared-here').count()) === 1);
		await section(a, 'acquisition');
	}
	await fill(field(a, 'mt-averages'), 12);
	await section(a, 'dofs');
	await dof(a, 1, 1, 'reference');
	await dof(a, 3, 2, 'response');
	await dof(a, 4, 3, 'response');
	await dof(a, 5, 4, 'reference');
	await section(a, 'excitation');
	for (const r of [2, 6]) await a.locator(`[data-testid="mt-drive-row"][data-row="${r}"]`).setChecked(true);
	const said = (p) => p.locator('[data-testid="mt-said"]').innerText().catch(() => '');
	if (full) {
		// A level over the ceiling, then pseudo random on two drives: each refused in the service's words.
		await tid(a, 'mt-source').selectOption('burst_random');
		await fill(a.locator('[data-testid="mt-drive-level"] input, input[data-testid="mt-drive-level"]').first(), 0.9);
		// the DOFs are table edits: Apply Setup sends the table, then the service checks the setup
		await tid(a, 'act-apply').click();
		await until(a, 'the ceiling refusal', async (p) => /not inside the drive ceiling|over the [\d.]+ V ceiling/.test(await said(p)), 30_000).catch(() => {});
		check('a level over the ceiling is refused on screen', /not inside the drive ceiling|over the [\d.]+ V ceiling/.test(await said(a)), (await said(a)).replace(/\s+/g, ' ').slice(0, 300));
		check('Proceed to Run waits while it is refused', (await attr(a, 'mt-ready', 'data-ready')) !== 'true');
		await fill(a.locator('[data-testid="mt-drive-level"] input, input[data-testid="mt-drive-level"]').first(), 0.05);
		await tid(a, 'mt-source').selectOption('pseudo_random');
		await tid(a, 'act-apply').click();
		await until(a, 'the pseudo random refusal', async (p) => /drives one shaker only/.test(await said(p)), 30_000).catch(() => {});
		check('pseudo random on two drives is refused on screen', /drives one shaker only/.test(await said(a)), (await said(a)).replace(/\s+/g, ' ').slice(0, 300));
	}
	await tid(a, 'mt-source').selectOption('burst_random');
	await until(a, 'the setup to check', async (p) => (await attr(p, 'mt-ready', 'data-ready')) === 'true', 30_000).catch(() => {});
	if (full) check('two shakers on burst random check clean', (await attr(a, 'mt-ready', 'data-ready')) === 'true', (await said(a)).replace(/\s+/g, ' ').slice(0, 300));
	if (full) {
		// the refusal of the last Apply Setup stays until dismissed, though the setup now checks clean
		const refusal = a.locator('[data-testid="refusal-message"]');
		if (await refusal.count()) await refusal.locator('xpath=../..').locator('button[aria-label="Dismiss"]').first().click();
		await shot(a, 'setup_shaker');
	}
	// the frame's length as set up: the block over the sample rate
	await section(a, 'acquisition');
	const frameS = Number(await tid(a, 'mt-block').inputValue()) / Number(await field(a, 'mt-fs').inputValue());
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(a, 'act-start').click();
	await until(a, 'the test at its first point', async (p) => (await measureState(p)) === 'idle', 30_000);
	await tid(a, 'mt-drive_start').click();
	await until(a, 'frames mid-average', async (p) => (await count(p)) >= 6, 90_000);
	check('the averages build from the service, mid-average', /^\d+ of 12$/.test(await text(a, 'mt-count')), await text(a, 'mt-count'));
	check('the drive phase is in words', /Drive:/.test(await text(a, 'mt-drive-phase')), await text(a, 'mt-drive-phase'));
	const ls = await legends(a);
	const entries = (re) => ls.find((l) => re.test(l.title))?.entries ?? [];
	check(`every reference, response and drive has its trace and legend entry (${theme})`, entries(/^Force$/).length === 2 && entries(/^Drive$/).length === 2 && entries(/^Reference Autospectra$/).length === 2, legendWords(ls));
	check(`every legend is whole at 1440 by 900 (shaker, ${theme})`, ls.every((l) => !l.cut), legendWords(ls));
	await shot(a, theme === 'dark' ? 'dark_measure_shaker' : 'measure_shaker');
	// Stop Driving first, before the frames run out: the small window is checked with the drive stopped
	let stopped = false;
	const n = await count(a);
	if (full && (await measureState(a)) === 'measuring') {
		await tid(a, 'mt-drive_stop').click();
		await until(a, 'the drive stopped', async (p) => (await measureState(p)) !== 'measuring', 30_000);
		check('Stop Driving stops the drive and keeps the averages', (await count(a)) >= n, `${n} to ${await text(a, 'mt-count')}`);
		stopped = true;
	} else if (full && !MOCK) {
		check('Stop Driving is reached while the shakers drive', false, await measureState(a));
	}
	// the time plots draw the latest whole frame, from its start, with the burst's end marked
	// (checked once the drive is stopped in the light run, so the point does not fill meanwhile)
	const tp = await timePlots(a);
	const span = (p) => p && Math.abs(p.end - frameS) < 1e-9 * frameS && p.last <= p.end + 1e-12 && p.last >= p.end - 2 * p.step;
	check(`the time plots span the whole frame, ${frameS} s, from its start (${theme})`, tp.length === 3 && tp.every((p) => p.frame && span(p)), JSON.stringify(tp));
	const [tf, tr, td] = tp;
	check(`the burst's end is marked on every time plot, the drives no later than the inputs (${theme})`, tp.every((p) => p.burst !== null && p.burst > 0 && p.burst < p.end && p.marker && p.note) && tf.burst === tr.burst && td.burst <= tf.burst, JSON.stringify(tp.map((p) => [p.id, p.burst, p.marker, p.note])));
	const fd = await text(a, 'mt-frame-decay').catch(() => '');
	const fdFit = await a.evaluate(() => {
		const d = document.querySelector('[data-testid="mt-frame-decay"]');
		const h = d?.closest('.plot')?.querySelector('.head .title');
		if (!d || !h) return { whole: false, clear: false };
		const words = document.createRange();
		words.selectNodeContents(h);
		return { whole: d.scrollWidth <= d.clientWidth + 1, clear: words.getBoundingClientRect().right <= d.getBoundingClientRect().left, title: d.title };
	});
	check(`the frame's decay words are whole and clear of the Response title at 1440 by 900 (${theme})`, fdFit.whole && fdFit.clear, JSON.stringify(fdFit));
	check(`the frame's decay is beside the Response plot, with its row and percent (${theme})`, /^(Not Died Away)?Row \d+, [\d.]+%$/.test(fd), fd);
	await a.setViewportSize({ width: 1280, height: 800 });
	await a.waitForTimeout(600);
	const tcs = await titlesCut(a);
	check(`no plot title is cut short at 1280 by 800 (shaker, ${theme})`, tcs.length === 0, tcs.join(', '));
	const ls800 = await legends(a);
	check(`every legend is whole at 1280 by 800 (shaker, ${theme})`, ls800.every((l) => !l.cut), legendWords(ls800));
	const e800 = await endOnScreen(a);
	check(`End Test is on screen at 1280 by 800 (shaker, ${theme})`, e800.seen, e800.at);
	const tp800 = await timePlots(a);
	check(`the burst's end label stays inside each time plot at 1280 by 800 (${theme})`, tp800.every((p) => p.note && p.marker), JSON.stringify(tp800.map((p) => [p.id, p.burst, p.marker, p.note])));
	await a.screenshot({ path: path.join(SHOTS, `modal_test_${theme === 'dark' ? 'dark_' : ''}measure_shaker_1280x800.png`) });
	await a.setViewportSize({ width: 1440, height: 900 });
	await a.waitForTimeout(400);
	if (!full || !stopped) {
		// the dark run, and the mock, whose frames can fill the point before Stop Driving is reached
		await tid(a, 'mt-end').click();
		await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-state')) === 'finished', 30_000);
		return a;
	}
	// a point's FRFs are saved once it has its averages: drive on to 12 of 12
	await tid(a, 'mt-drive_start').click();
	await until(a, 'the point measured', async (p) => (await text(p, 'mt-count')) === '12 of 12' && (await measureState(p)) !== 'measuring', 120_000);
	check('Start Driving again finishes the point at 12 of 12', true, await text(a, 'mt-count'));
	await tid(a, 'mt-end').click();
	await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-state')) === 'finished', 30_000);
	check('End Test ends the shaker test in words', (await tid(a, 'mt-ended').count()) === 1, await text(a, 'mt-ended').catch(() => ''));
	await neverLive(a, 'after the shaker test');
	if (MOCK) return a;
	await analysis(a, 'shaker');
	return a;
}

try {
	const only = flag('--only');
	if (only !== 'shaker') {
		await done(await impact('light', true));
		await done(await impact('dark', false));
	}
	if (only !== 'impact') {
		await done(await shaker('light', true));
		await done(await shaker('dark', false));
	}
} catch (err) {
	failed = err;
	console.error(err);
	if (page) await shot(page, 'stopped').catch(() => {});
} finally {
	await browser.close();
	stopAll();
}

check('every Space armed on its first press', lostSpace === 0, `${lostSpace} missed`);
const passed = results.filter((r) => r.ok).length;
console.log(`\nmodal test: ${passed} of ${results.length} checks passed${failed ? `; stopped: ${failed.message}` : ''}`);
console.log(`screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
