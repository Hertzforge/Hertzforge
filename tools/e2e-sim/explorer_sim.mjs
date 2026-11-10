// Explorer on the simulator, end to end: the real control service and the shell, driven through the
// installed Chrome the way an operator drives them, every check made on what the screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/explorer_sim.mjs [--shots <dir>] [--headed] [--mock]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs Explorer on the simulated station, and stops both. With `--mock` it starts
// only the shell, against its own mock control service, to try the flow without a build. The run
// takes a few minutes. Not part of `npm test`.
//
// 1. The chooser offers Explorer; its screen opens on Setup, and the screen takes control. The SIM
//    badge is checked at every step: a simulated run never shows LIVE.
// 2. Station & Channels: the table's one drive row and its control row are named; an abort limit
//    is set on the control row.
// 3. Tone & Drive: the ceiling (no default), the band, the start and the rate; the setup checks
//    clean and the service's derived numbers are shown beside it.
// 4. Proceed to Run applies; Start: the tone runs at 0 V and the level card says "Quiet".
// 5. The level raised in volts, answered by the service; the frequency steered with the panel's
//    buttons, a go-to and the keys (only while the panel holds focus, one step a press).
// 6. A sweep up, Hold, a sweep down: the tone moves on the service's own clock and stops where it is.
// 7. The survey line fills and its peak is marked; the spectra and time pages update; a page opened
//    in a window of its own follows the run.
// 8. A second tab observes and aborts.
// 9. The controlling tab is closed during a run: the run goes on, and it safe-stops with its ramp
//    after the countdown.
//
// Screenshots (1,440 by 900): `<prefix>_<name>.png`, the dark theme `<prefix>_dark_<name>.png`.

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-explorer'));
const HEADED = args.includes('--headed');
const MOCK = args.includes('--mock');
const PREFIX = flag('--prefix') ?? 'explorer';
/** Run seconds per real second: the mock runs at 4, the service in real time. */
const SPEED = MOCK ? 4 : 1;
fs.mkdirSync(SHOTS, { recursive: true });

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
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const attr = (page, id, name) => tid(page, id).getAttribute(name);
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);
const shots = [];

async function shot(page, name) {
	const file = path.join(SHOTS, `${PREFIX}_${name}.png`);
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await page.waitForTimeout(400);
	await page.screenshot({ path: file });
	const kb = Math.round(fs.statSync(file).size / 1024);
	shots.push({ file, kb });
	console.log(`      screenshot ${file} (${kb} kB)`);
}

async function fill(locator, value) {
	await locator.fill(String(value));
	await locator.blur();
}

/** A simulated run never shows LIVE: checked at every step of the flow. */
async function neverLive(page, where) {
	const live = await page.locator('[data-testid="live-badge"]').count();
	check(`no LIVE badge (${where})`, live === 0);
}

/** A setup section, from the Setup step. */
async function section(page, id) {
	if (!(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
	await page.locator('#explorer-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

/** A run page by its name. */
async function showPage(page, name) {
	const sel = page.locator('[data-testid="explorer-page-select"]');
	const value = await sel.locator('option', { hasText: name }).first().getAttribute('value');
	await sel.selectOption(value);
	await until(page, `the ${name} page`, async (p) => (await p.locator('[data-testid="explorer-page-view"]').getAttribute('data-page')) === value);
}

const runState = (page) => attr(page, 'explorer-run-view', 'data-state');
const runPhase = (page) => attr(page, 'explorer-run-view', 'data-phase');
const num = async (page, name) => Number((await attr(page, 'explorer-run-view', name)) || NaN);
/** The tone's frequency and level and the service's station time, read together from the run view. */
const reading = async (page) => ({ hz: await num(page, 'data-hz'), vpk: await num(page, 'data-vpk'), t: await num(page, 'data-t') });
const near = (a, b, tol) => Math.abs(a - b) <= tol;

// ---- the run ----------------------------------------------------------------------------------
let servicePort = null;
if (!MOCK) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	servicePort = await freePort();
	const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
	const service = start('sds-control', exe, ['--port', String(servicePort)]);
	await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
}
const shellPort = await freePort();
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], {
	shell: process.platform === 'win32'
});
const QUERY = MOCK ? `?mock=1&mock_speed=${SPEED}` : `?control=ws://127.0.0.1:${servicePort}/`;
const URL_ = (theme) => `http://127.0.0.1:${shellPort}/${QUERY}&theme=${theme}#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`shell ${URL_('light')}${MOCK ? ' (mock control service)' : `, service ws://127.0.0.1:${servicePort}/`}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
let failed = null;

async function openExplorer(page, claim) {
	await page.goto(URL_('light'));
	await until(page, 'the chooser or the screen', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && ((await tid(p, 'test-type-chooser').isVisible()) || (await tid(p, 'explorer-screen').isVisible())));
	if (await tid(page, 'test-type-chooser').isVisible()) await tid(page, 'test-type-explorer').click();
	await until(page, 'the Explorer screen', async (p) => tid(p, 'explorer-screen').isVisible());
	if (claim) {
		await tid(page, 'claim').click();
		await until(page, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	}
}

/** Raise the level in volts and wait until the drive holds it. */
async function raise(page, volts) {
	await fill(tid(page, 'explorer-volts'), volts);
	await tid(page, 'explorer-set-volts').click();
	await until(page, `the drive at ${volts} V pk`, async (p) => (await runPhase(p)) === 'steady' && near(await num(p, 'data-vpk'), volts, 1e-4), 60_000);
}

try {
	const a = await ctx.newPage();
	await a.goto(URL_('light'));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers Explorer', offered.includes('explorer'), offered.join());
	await openExplorer(a, true);
	check('Explorer opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- Station & Channels: the drive row, the control row, an abort limit -----------------------
	await section(a, 'station');
	const rowsWords = await text(a, 'explorer-run-channels');
	check('the run names its one drive row and its control row', /drive row \(channel \d+\)/.test(rowsWords) && /\(channel \d+\)\. Every enabled input/.test(rowsWords), rowsWords);
	await tid(a, 'channel-more-0').click();
	const limit = a.locator('#ch-0-ab-g');
	const off = a.locator('.nf', { has: limit }).locator('input[type="checkbox"]');
	if (await off.isChecked()) await off.uncheck();
	await fill(limit, 5);
	if (await tid(a, 'table-apply').isVisible()) {
		await tid(a, 'table-apply').click();
		await until(a, 'the table applied', async (p) => !(await tid(p, 'table-edited').isVisible().catch(() => false)), 20_000);
	}
	check('an abort limit on the control row', true, '5 g');
	await shot(a, 'station');

	// ---- Tone & Drive: the ceiling first, then the band and the start ----------------------------
	await section(a, 'test');
	await until(a, 'the ceiling asked for', async (p) => /no default/i.test(await text(p, 'explorer-drive')), 10_000);
	check('the drive ceiling has no default: the setup asks for it', /no default/i.test(await text(a, 'explorer-drive')), await text(a, 'explorer-drive'));
	await fill(tid(a, 'explorer-ceiling'), 0.5);
	await fill(tid(a, 'explorer-band-lo'), 20);
	await fill(tid(a, 'explorer-band-hi'), 2000);
	await fill(tid(a, 'explorer-start'), 250);
	const fsOff = tid(a, 'explorer-fs-off');
	if (await fsOff.isChecked()) await fsOff.uncheck();
	await fill(tid(a, 'explorer-fs'), 12800);
	await tid(a, 'explorer-nfft-4096').click();
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'explorer-ready', 'data-ready')) === 'true', 30_000);
	const pipe = await text(a, 'explorer-derived-pipeline');
	check("the setup checks clean, with the service's derived numbers beside it", /\d+ frames, [\d.]+ ms/.test(pipe) && /6\.667 s/.test(await text(a, 'explorer-derived-stop')), `${pipe}; ${await text(a, 'explorer-derived-stop')}`);
	check('the rate and block are the service\'s for the selected station', /12800 Hz, 256 frames a block/.test(await text(a, 'explorer-derived-grid')), await text(a, 'explorer-derived-grid'));
	await shot(a, 'setup');

	// ---- Run: the quiet start --------------------------------------------------------------------
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	check('Proceed to Run applies the setup: the run is configured', true);
	check('there is no Arm and no level in dB on the run bar', (await tid(a, 'act-arm').count()) === 0 && (await tid(a, 'run-level').count()) === 0);
	await tid(a, 'act-start').click();
	await until(a, 'the tone at 0 V', async (p) => (await runState(p)) === 'running' && (await runPhase(p)) === 'quiet', 30_000);
	check('Start runs the tone at 0 V', (await text(a, 'explorer-level-status')) === 'Quiet: the tone is at 0 V' && (await num(a, 'data-vpk')) === 0, await text(a, 'explorer-level-status'));
	check('a dB step waits for a level to step from', (await attr(a, 'explorer-level-fine-up', 'aria-disabled')) === 'true');
	await neverLive(a, 'quiet');

	// ---- The level, in volts, answered by the service -------------------------------------------
	await fill(tid(a, 'explorer-volts'), 0.9);
	await tid(a, 'explorer-set-volts').click();
	await until(a, 'the answer', async (p) => /clamped to the ceiling/.test(await text(p, 'explorer-answer')), 10_000);
	check('a level over the ceiling is answered at the ceiling', /^Answered 0\.500 V pk, clamped to the ceiling$/.test(await text(a, 'explorer-answer')), await text(a, 'explorer-answer'));
	await until(a, 'the rise', async (p) => /^Rising to 0\.500 V pk$/.test(await text(p, 'explorer-level-status')), 10_000);
	check('the level card says the drive is rising', true, await text(a, 'explorer-level-status'));
	await raise(a, 0.2);
	check('the level card says what the tone holds', /^Holding 250\.0 Hz at 0\.200 V pk$/.test(await text(a, 'explorer-level-status')), await text(a, 'explorer-level-status'));
	await shot(a, 'holding');

	// ---- The frequency: the panel's buttons, a go-to, the keys -----------------------------------
	const f0 = await num(a, 'data-hz');
	await tid(a, 'explorer-freq-coarse-up').click();
	await until(a, 'a coarse step up', async (p) => near(await num(p, 'data-hz'), f0 * 2 ** (1 / 6), 0.01), 10_000);
	check('a coarse step moves the tone 1/6 octave', true, `${f0} to ${await num(a, 'data-hz')} Hz`);
	await fill(tid(a, 'explorer-goto-hz'), 5000);
	await tid(a, 'explorer-goto').click();
	await until(a, 'the go-to', async (p) => near(await num(p, 'data-hz'), 2000, 0.01), 10_000);
	check('a go-to past the band is answered at its edge', /^Answered 2000\.0 Hz, clamped to the band$/.test(await text(a, 'explorer-answer')), await text(a, 'explorer-answer'));
	await fill(tid(a, 'explorer-goto-hz'), 250);
	await tid(a, 'explorer-goto').click();
	await until(a, 'back at 250 Hz', async (p) => near(await num(p, 'data-hz'), 250, 0.01), 10_000);
	// keys act only while the steering panel holds focus
	await a.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await a.keyboard.press('ArrowRight');
	await a.waitForTimeout(400);
	check('a steering key does nothing while the panel does not hold focus', near(await num(a, 'data-hz'), 250, 0.01), `${await num(a, 'data-hz')} Hz`);
	await tid(a, 'explorer-steer').focus();
	await a.keyboard.press('ArrowRight');
	await until(a, 'a fine step from the key', async (p) => near(await num(p, 'data-hz'), 250 * 2 ** (1 / 96), 0.01), 10_000);
	check('Right steps the frequency one fine step while the panel holds focus', true, `${await num(a, 'data-hz')} Hz`);
	await a.keyboard.press('Shift+ArrowLeft');
	await until(a, 'a coarse step from the key', async (p) => near(await num(p, 'data-hz'), 250 * 2 ** (1 / 96 - 1 / 6), 0.01), 10_000);
	check('Shift+Left steps it down one coarse step', true, `${await num(a, 'data-hz')} Hz`);
	await a.keyboard.press('ArrowUp');
	await until(a, 'a fine level step from the key', async (p) => /^Answered 0\.21\d V pk$/.test(await text(p, 'explorer-answer')), 10_000);
	check('Up steps the level one fine step in dB', true, await text(a, 'explorer-answer'));
	await raise(a, 0.2);

	// ---- A sweep up, Hold, a sweep down ----------------------------------------------------------
	await fill(tid(a, 'explorer-goto-hz'), 100);
	await tid(a, 'explorer-goto').click();
	await until(a, 'at 100 Hz', async (p) => near(await num(p, 'data-hz'), 100, 0.01), 10_000);
	await fill(tid(a, 'explorer-sweep-rate'), 30);
	await tid(a, 'explorer-sweep-up').click();
	await until(a, 'the sweep', async (p) => (await runPhase(p)) === 'sweeping', 10_000);
	const s0 = await reading(a);
	await until(a, 'the tone to rise', async (p) => (await num(p, 'data-hz')) > s0.hz * 1.2, 30_000);
	const s1 = await reading(a);
	check('the sweep moves the tone up on the service clock', s1.hz > s0.hz && s1.t > s0.t, `${s0.hz} Hz at ${s0.t} s, then ${s1.hz} Hz at ${s1.t} s`);
	check('the level card says it sweeps', /^Sweeping up at 30 oct\/min$/.test(await text(a, 'explorer-level-status')), await text(a, 'explorer-level-status'));
	await tid(a, 'act-hold').click();
	await until(a, 'the hold', async (p) => (await runPhase(p)) === 'steady' && (await attr(p, 'explorer-run-view', 'data-sweep')) === '', 10_000);
	const h0 = await reading(a);
	await until(a, 'the station clock to run on', async (p) => (await num(p, 'data-t')) > h0.t + 2, 20_000);
	const h1 = await reading(a);
	check('Hold leaves the tone where it is while the station runs on', near(h1.hz, h0.hz, 1e-3), `${h0.hz} Hz at ${h0.t} s, ${h1.hz} Hz at ${h1.t} s`);
	await tid(a, 'explorer-sweep-down').click();
	await until(a, 'the sweep down', async (p) => (await runPhase(p)) === 'sweeping' && (await num(p, 'data-hz')) < h1.hz * 0.9, 30_000);
	check('a sweep down moves the tone down', true, `${await num(a, 'data-hz')} Hz`);
	await tid(a, 'explorer-hold').click();
	await until(a, 'the second hold', async (p) => (await runPhase(p)) === 'steady', 10_000);

	// ---- The pages: the survey line and its peak, the spectra, the time history ------------------
	await showPage(a, 'Survey');
	await until(a, 'the survey line', async (p) => Number(await attr(p, 'explorer-survey-plot', 'data-points')) > 5, 30_000);
	const marks = await text(a, 'explorer-survey-marks');
	check('the survey line fills, with the tone and its peak marked', /is the tone; the thin line at [\d.]+ Hz is the highest point/.test(marks), `${await attr(a, 'explorer-survey-plot', 'data-points')} points; ${marks}`);
	check('and the level card names the peak', /^Survey peak [\d.]+ Hz, [\d.]+ g$/.test(await text(a, 'explorer-peak')), await text(a, 'explorer-peak'));
	await neverLive(a, 'survey');
	await shot(a, 'survey');
	await showPage(a, 'Spectra');
	await until(a, 'the spectra', async (p) => /Channel 1/.test(await text(p, 'explorer-page-view')), 20_000);
	check('the spectra page draws every acquired channel', true);
	await shot(a, 'spectra');
	await showPage(a, 'Time');
	await until(a, 'the time history', async (p) => /Time/.test(await text(p, 'explorer-page-view')) && !/drawn after/.test(await text(p, 'explorer-page-view')), 20_000);
	check('the time page draws the control channel', true);
	await shot(a, 'time');
	await showPage(a, 'Survey');

	// A page in a window of its own follows the run.
	const [win] = await Promise.all([ctx.waitForEvent('page'), tid(a, 'explorer-page-popout').click()]);
	await until(win, 'the display window', async (p) => tid(p, 'explorer-run-view').isVisible(), 30_000);
	await until(win, 'the window to read the run', async (p) => Number.isFinite(await num(p, 'data-t')), 30_000);
	const w0 = await num(win, 'data-t');
	await until(win, 'the window to follow the run', async (p) => (await num(p, 'data-t')) > w0 + 1, 30_000);
	check('a page opened in a window of its own follows the run', true, `station time ${w0} then ${await num(win, 'data-t')} s`);
	check('the window has no steering panel', (await tid(win, 'explorer-steer').count()) === 0);
	await win.close();

	// ---- An observer aborts; the controlling tab is closed -----------------------------------------
	// Both need one service shared by two tabs: the shell's mock lives in each tab, so `--mock` skips them.
	if (MOCK) {
		console.log('skip  the observer abort and the closed controlling tab: they need the control service');
		await tid(a, 'act-stop').click();
		await until(a, 'the stop', async (p) => (await runState(p)) === 'stopping', 10_000);
		check('Stop ramps the drive down', /^Stopping: the drive is ramping down$/.test(await text(a, 'explorer-level-status')), await text(a, 'explorer-level-status'));
		check('and the steering waits', (await attr(a, 'explorer-freq-fine-up', 'aria-disabled')) === 'true');
		await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'operator_stop', 30_000);
		check('the run ends with the operator stop, its ramp on the station clock', /ramped down in [\d.]+ s of station time/.test(await text(a, 'explorer-end')), await text(a, 'explorer-end'));
	} else {
		const b = await ctx.newPage();
		await openExplorer(b, false);
		await tid(b, 'flow-run').click().catch(() => {});
		await until(b, 'the observer sees the run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'running', 30_000);
		check('a second tab observes', (await attr(b, 'session-role', 'data-role')) === 'observer');
		check('the observer is told to take control to steer', (await attr(b, 'explorer-freq-fine-up', 'aria-disabled')) === 'true');
		await tid(b, 'act-abort').click();
		await until(a, 'the abort', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'operator_abort', 20_000);
		check('an observer tab aborts the run', true);

		// ---- The controlling tab is closed ------------------------------------------------------------
		await tid(a, 'act-reset').click();
		await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured', 20_000);
		await tid(a, 'act-start').click();
		await until(a, 'the tone at 0 V', async (p) => (await runPhase(p)) === 'quiet', 30_000);
		await raise(a, 0.1);
		await a.close();
		await until(b, 'the countdown', async (p) => tid(p, 'orphan').isVisible(), 20_000);
		check('with nobody in control the observer sees the countdown', true, await text(b, 'orphan'));
		const t0 = await num(b, 'data-t');
		await until(b, 'the run to go on', async (p) => (await num(p, 'data-t')) > t0 + 2, 20_000);
		check('the run goes on with nobody in control', true);
		await shot(b, 'orphan');
		await until(b, 'the safe stop', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'safe_stop', 90_000);
		check('the run safe-stopped with its ramp', /ramped down in [\d.]+ s of station time/.test(await text(b, 'explorer-end')), await text(b, 'explorer-end'));
		await neverLive(b, 'after the safe stop');
	}

	// ---- The dark theme ---------------------------------------------------------------------------
	const d = await ctx.newPage();
	await d.goto(URL_('dark'));
	await until(d, 'the screen', async (p) => (await tid(p, 'explorer-screen').isVisible()) || (await tid(p, 'test-type-chooser').isVisible()));
	if (await tid(d, 'test-type-chooser').isVisible()) await tid(d, 'test-type-explorer').click();
	await until(d, 'the Explorer screen', async (p) => tid(p, 'explorer-screen').isVisible());
	await section(d, 'test');
	await shot(d, 'dark_setup');
	await tid(d, 'flow-run').click();
	await until(d, 'the run view', async (p) => tid(p, 'explorer-run-view').isVisible(), 20_000);
	await d.waitForTimeout(1000);
	await shot(d, 'dark_run');
} catch (err) {
	failed = err;
	console.error(err);
} finally {
	await browser.close();
	stopAll();
}

const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} of ${results.length} checks passed; ${shots.length} screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
