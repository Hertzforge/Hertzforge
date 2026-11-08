// SRS shock on the simulator, end to end: the real control service and the shell, driven through
// the installed Chrome the way an operator drives them, every check made on what the screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/srs_sim.mjs [--shots <dir>] [--headed] [--law <file.wasm>] [--mock]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs SRS shock on the simulated station, and stops both. With `--mock` it starts
// only the shell, against its own mock control service, to try the flow without a build. The law
// imported in step 4 is `--law`, else the SRS law file `scripts/build_laws.sh` builds (built first
// when it is missing). The run takes a few minutes. Not part of `npm test`.
//
// 1. The chooser offers SRS; its screen opens on Setup, and the screen takes control. The SIM badge
//    is checked at every step: a simulated run never shows LIVE.
// 2. Limits: the drive ceiling and abort, which have no default.
// 3. Target, near 1 g: the service draws the target on its band grid and the offline synthesis
//    against it, a computation and never a measurement; Synthesis draws the synthesized pulse with
//    its velocity and displacement. A target that stops short of the grid's last band is named on
//    its table.
// 4. Control: the SRS laws from the service; a law file imported by its path shows its parameters
//    from its manifest; the built-in law is chosen again for the run.
// 5. Proceed to Run applies; Start identifies; the review hold brings the identified gain and the
//    coherence forward, with the burst's peak against its cap; Proceed.
// 6. Pulses fill the table and the SRS page follows; the level card's words; pulse starts are at
//    least the time between pulses apart; a page opened in a window of its own follows the run;
//    two pulses picked in the table lay over the SRS page; the run completes.
// 7. Manual: Fire, a 1 dB step, Go to a level; a second Fire too early is refused in words.
// 8. A second tab observes and aborts.
// 9. The controlling tab is closed during a run: firing pauses and the run safe-stops after the
//    countdown.
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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-srs'));
const HEADED = args.includes('--headed');
const MOCK = args.includes('--mock');
const PREFIX = flag('--prefix') ?? 'srs';
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
	await page.locator('#rk-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

/** A run page by its name. */
async function showPage(page, name) {
	const sel = page.locator('[data-testid="srs-page-select"]');
	const value = await sel.locator('option', { hasText: name }).first().getAttribute('value');
	await sel.selectOption(value);
	await until(page, `the ${name} page`, async (p) => (await p.locator('[data-testid="srs-page-view"]').getAttribute('data-page')) === value);
}

const runState = (page) => attr(page, 'srs-run-view', 'data-state');
const runPhase = (page) => attr(page, 'srs-run-view', 'data-phase');
const pulsesFired = async (page) => Number((await attr(page, 'srs-run-view', 'data-pulses')) ?? '0');

// ---- the run ----------------------------------------------------------------------------------
let servicePort = null;
let LAW = flag('--law') ?? null;
if (!MOCK) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	if (!LAW) {
		LAW = path.join(TARGET, 'wasm32-wasip2', 'release', 'hertzforge_law_shock_srs_ratio.wasm');
		if (!fs.existsSync(LAW)) {
			console.log('building the law files');
			execFileSync('bash', [path.join(ROOT, 'scripts', 'build_laws.sh')], { cwd: ROOT, stdio: 'inherit' });
		}
	}
	servicePort = await freePort();
	const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
	const service = start('sds-control', exe, ['--port', String(servicePort)]);
	await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
} else {
	LAW ??= path.join(os.tmpdir(), 'srs-template.wasm');
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

async function openSrs(page, claim) {
	await page.goto(URL_('light'));
	await until(page, 'the chooser or the screen', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && ((await tid(p, 'test-type-chooser').isVisible()) || (await tid(p, 'srs-screen').isVisible())));
	if (await tid(page, 'test-type-chooser').isVisible()) await tid(page, 'test-type-srs').click();
	await until(page, 'the SRS screen', async (p) => tid(p, 'srs-screen').isVisible());
	if (claim) {
		await tid(page, 'claim').click();
		await until(page, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	}
}

/** Apply, start, proceed past the review hold when there is one. */
async function runFromSetup(page) {
	await tid(page, 'proceed-to-run').click();
	await until(page, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(page, 'act-start').click();
	await until(page, 'identification done', async (p) => ['review', 'running', 'finished'].includes(await runState(p)) && (await runPhase(p)) !== 'sysid', 120_000);
	if ((await runState(page)) === 'review') await tid(page, 'act-proceed').click();
}

try {
	const a = await ctx.newPage();
	await a.goto(URL_('light'));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers SRS', offered.includes('srs'), offered.join());
	await openSrs(a, true);
	check('SRS opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- Limits first: with no drive ceiling and abort the setup is not checked by the service ------
	await section(a, 'limits');
	await fill(tid(a, 'srs-drive-ceiling'), 2);
	await fill(tid(a, 'srs-drive-abort'), 3);
	await shot(a, 'limits');

	// ---- Target and synthesis -------------------------------------------------------------------
	// A target near 1 g: the simulated station needs about 2 V a g, so the full-level drive stays
	// under the 3 V drive abort set above.
	await section(a, 'target');
	for (const [i, g] of [0.4, 1, 1].entries()) await fill(a.locator(`[data-testid="srs-target-points-${i}-accel_g"]`), g);
	await until(a, 'the target on its grid', async (p) => (await attr(p, 'srs-target-preview', 'data-state')) === 'ok', 30_000);
	check('the service draws the target on its grid and the offline synthesis, as a computation', (await a.locator('[data-testid^="srs-target-view-"]').count()) === 2 && /never a measurement/.test(await text(a, 'srs-target-preview')), await text(a, 'srs-band-count'));
	const legend = await text(a, 'srs-target-view-0');
	check('the target carries its alarm and abort bands', /Target/.test(legend) && /Alarm/.test(legend) && /Abort/.test(legend), legend.slice(0, 120));
	await shot(a, 'target');
	// A target that stops short of the grid's last band is named on its table.
	const lastF = a.locator('[data-testid="srs-target-points-2-freq_hz"]');
	await fill(lastF, 1500);
	await until(a, 'the target named short of the grid', async (p) => /cover/i.test(await text(p, 'srs-target-points')), 20_000);
	check('a target short of the band grid is named on its table, in words', true, (await text(a, 'srs-target-points')).slice(-140));
	check('and the service draws no synthesis for it', /cover/i.test(await text(a, 'srs-target-preview')), (await text(a, 'srs-target-preview')).slice(0, 140));
	await fill(lastF, 2500);
	await until(a, 'the target covering the grid again', async (p) => !/cover/i.test(await text(p, 'srs-target-points')) && (await attr(p, 'srs-target-preview', 'data-state')) === 'ok', 20_000);
	await section(a, 'synthesis');
	await until(a, 'the synthesized pulse', async (p) => (await attr(p, 'srs-synthesis-preview', 'data-state')) === 'ok', 30_000);
	check('the synthesized pulse with its velocity and displacement', (await a.locator('[data-testid^="srs-synthesis-view-"]').count()) === 3, await text(a, 'srs-synthesis-facts'));
	await shot(a, 'synthesis');

	// ---- Control: the catalog, an imported law, back to the built-in --------------------------------
	await section(a, 'control');
	await until(a, 'the law catalog', async (p) => (await p.locator('[data-testid="law-list"] [role="radio"]').count()) > 0);
	check('the SRS laws come from the service, the built-in first', (await tid(a, 'law-srs_ratio').count()) === 1);
	await tid(a, 'law-import-open').click();
	await fill(tid(a, 'law-path'), LAW);
	await tid(a, 'law-import-go').click();
	await until(a, 'the imported law', async (p) => (await tid(p, 'law-imported').count()) > 0 && (await attr(p, 'law-imported', 'aria-checked')) === 'true', 30_000);
	const params = await a.locator('[data-testid="law-params"] [data-testid^="law-param-"]').count();
	check('an imported law shows its parameters from its manifest', params > 0, `${params} parameters`);
	await shot(a, 'control_imported');
	await tid(a, 'law-srs_ratio').click();
	await shot(a, 'control');

	// ---- Run: identification, the review hold, pulses -------------------------------------------
	await section(a, 'target');
	await until(a, 'nothing missing before a run', async (p) => (await attr(p, 'srs-ready', 'data-ready')) === 'true', 30_000);
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	check('Proceed to Run applies the setup: the run is configured', true);
	await tid(a, 'act-start').click();
	await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 120_000);
	check('the review hold says what it waits for', await tid(a, 'srs-review').isVisible());
	check('and brings the identified gain forward', (await a.locator('[data-testid="srs-page-view"]').getAttribute('data-kind')) === 'identification');
	await until(a, 'the identified gain', async (p) => {
		const t = await text(p, 'srs-page-view');
		return /\|H\||Gain/i.test(t) && /Coherence/.test(t);
	}, 20_000);
	check('the gain and the coherence are drawn', true);
	check('the burst is held under its cap in volts, in words', /Burst [\d.]+ V pk, held at [\d.]+/.test(await text(a, 'srs-identification-facts')), await text(a, 'srs-identification-facts'));
	await neverLive(a, 'review');
	await shot(a, 'review');
	await tid(a, 'act-proceed').click();
	await until(a, 'three pulses', async (p) => (await pulsesFired(p)) >= 3, 120_000);
	await showPage(a, 'SRS');
	const srsPage = await text(a, 'srs-page-view');
	check('the SRS page follows the run: target, envelope and measured SRS', /Target/.test(srsPage) && /Measured/.test(srsPage) && /Alarm/.test(srsPage), srsPage.slice(0, 120));
	const words = await text(a, 'srs-level-card');
	check('the level card counts pulses against the plan', /Pulse \d+ of \d+ at [-+]?\d+\.\d dB|Completed/.test(words), words.slice(0, 160));
	check('and says how the last pulse went', /Pulse \d+ (passed|did not pass|fired)|Pulse \d+: \d+ bands? outside the alarm band/.test(await text(a, 'srs-last-pulse')), await text(a, 'srs-last-pulse'));
	check('and names its worst band', /^Worst band [\d.]+ Hz, /.test(await text(a, 'srs-worst-band')), await text(a, 'srs-worst-band'));
	await neverLive(a, 'running');
	await shot(a, 'run_srs');

	// A page in a window of its own follows the run.
	const [win] = await Promise.all([ctx.waitForEvent('page'), tid(a, 'srs-page-popout').click()]);
	await until(win, 'the display window', async (p) => tid(p, 'srs-run-view').isVisible(), 30_000);
	const before = await pulsesFired(win);
	await until(win, 'the window to follow a pulse', async (p) => (await pulsesFired(p)) > before || (await runState(p)) === 'finished', 120_000);
	check('a page opened in a window of its own follows the run', true, `${before} then ${await pulsesFired(win)} pulses`);
	await win.close();

	// Two pulses laid over the SRS page.
	await showPage(a, 'Pulses');
	await tid(a, 'srs-overlay-0').check();
	await tid(a, 'srs-overlay-1').check();
	await showPage(a, 'SRS');
	await until(a, 'the overlay', async (p) => /Pulse 1, measured/.test(await text(p, 'srs-page-view')) && /Pulse 2, measured/.test(await text(p, 'srs-page-view')), 20_000);
	check('two pulses picked in the table lay over the SRS page', true);
	await shot(a, 'run_overlay');
	await until(a, 'the end of the schedule', async (p) => (await runState(p)) === 'finished', 300_000);
	check('the automatic schedule completes', (await attr(a, 'run-state', 'data-outcome')) === 'completed', await attr(a, 'run-state', 'data-outcome'));
	await showPage(a, 'Pulses');
	// Automatic pulse starts are paced: never closer than the time between pulses (1 s by default).
	const starts = (await a.locator('[data-testid^="srs-pulse-"][data-at]').evaluateAll((els) => els.map((e) => Number(e.getAttribute('data-at'))))).filter(Number.isFinite);
	const gaps = starts.slice(1).map((t, i) => t - starts[i]);
	check('automatic pulses are at least the time between pulses apart', gaps.length > 0 && Math.min(...gaps) >= 0.95, `smallest gap ${Math.min(...gaps).toFixed(2)} s over ${gaps.length}`);
	await shot(a, 'run_pulses');

	// ---- Manual -----------------------------------------------------------------------------------
	await tid(a, 'back-to-setup').click();
	await section(a, 'schedule');
	await tid(a, 'shock-mode-manual').click();
	await section(a, 'identification');
	await tid(a, 'srs-id-review').setChecked(false, { force: true });
	await runFromSetup(a);
	await until(a, 'ready to fire', async (p) => (await runPhase(p)) === 'waiting', 120_000);
	await until(a, 'Fire offered', async (p) => (await attr(p, 'act-fire', 'aria-disabled')) === 'false', 30_000);
	await tid(a, 'act-fire').click();
	await until(a, 'the manual pulse', async (p) => (await pulsesFired(p)) === 1, 60_000);
	// The button waits (aria-disabled) and still answers a click with its reason, as an operator sees.
	await tid(a, 'act-fire').click({ force: true });
	let early = '';
	await until(a, 'the words for a Fire too early', async (p) => {
		early = (await p.locator('[data-testid="shock-verb-hint"], [data-testid="refusal"]').allTextContents()).join(' ');
		return /may fire in|Not Yet|not passed/i.test(early);
	}, 5000).catch(() => {});
	check('a Fire too early waits, in words', /may fire in|Not Yet|not passed/i.test(early), early.trim().slice(0, 100));
	await until(a, 'Fire offered again once the time between pulses has passed', async (p) => (await attr(p, 'act-fire', 'aria-disabled')) === 'false', 30_000);
	check('Fire is offered again once the time between pulses has passed', true);
	const levelBefore = await text(a, 'shock-level-now');
	await until(a, 'the level step offered', async (p) => (await attr(p, 'shock-level-up', 'aria-disabled')) === 'false', 30_000);
	await tid(a, 'shock-level-up').click();
	await until(a, 'the level to move', async (p) => (await text(p, 'shock-level-now')) !== levelBefore, 10_000);
	check('a 1 dB step moves the level', true, `${levelBefore} to ${await text(a, 'shock-level-now')}`);
	await a.locator('[data-testid="shock-level-goto"]').selectOption({ index: 1 });
	await until(a, 'the first level', async (p) => /-12\.0 dB/.test(await text(p, 'shock-level-now')), 10_000);
	check('Go to level moves to a level of the table', true, await text(a, 'shock-level-now'));
	check('no train and no burst', (await tid(a, 'act-train').count()) === 0 && (await tid(a, 'act-burst').count()) === 0);
	await shot(a, 'run_manual');

	// ---- An observer aborts; the controlling tab is closed ------------------------------------------
	// Both need one service shared by two tabs: the shell's mock lives in each tab, so `--mock` skips them.
	if (MOCK) console.log('skip  the observer abort and the closed controlling tab: they need the control service');
	else {
		const b = await ctx.newPage();
		await openSrs(b, false);
		await tid(b, 'flow-run').click().catch(() => {});
		await until(b, 'the observer sees the run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'running', 30_000);
		check('a second tab observes', (await attr(b, 'session-role', 'data-role')) === 'observer');
		await tid(b, 'act-abort').click();
		await until(a, 'the abort', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'operator_abort', 20_000);
		check('an observer tab aborts the run', true);

		// ---- The controlling tab is closed ------------------------------------------------------------
		await tid(a, 'act-reset').click();
		await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured', 20_000);
		await tid(a, 'act-start').click();
		await until(a, 'the run waiting', async (p) => (await runPhase(p)) === 'waiting', 120_000);
		const firedAtClose = await pulsesFired(b);
		await a.close();
		await until(b, 'the countdown', async (p) => tid(p, 'orphan').isVisible(), 20_000);
		check('with nobody in control the observer sees the countdown', true, await text(b, 'orphan'));
		await shot(b, 'orphan');
		await until(b, 'the safe stop', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'safe_stop', 90_000);
		check('firing paused, then the run safe-stopped', (await pulsesFired(b)) === firedAtClose, `${firedAtClose} pulses before, ${await pulsesFired(b)} after`);
		await neverLive(b, 'after the safe stop');
	}

	// ---- The dark theme ---------------------------------------------------------------------------
	const d = await ctx.newPage();
	await d.goto(URL_('dark'));
	await until(d, 'the screen', async (p) => (await tid(p, 'srs-screen').isVisible()) || (await tid(p, 'test-type-chooser').isVisible()));
	if (await tid(d, 'test-type-chooser').isVisible()) await tid(d, 'test-type-srs').click();
	await until(d, 'the SRS screen', async (p) => tid(p, 'srs-screen').isVisible());
	await section(d, 'target');
	await until(d, 'the target on its grid', async (p) => (await attr(p, 'srs-target-preview', 'data-state')) === 'ok', 30_000);
	await shot(d, 'dark_target');
	await tid(d, 'flow-run').click();
	await until(d, 'the run view', async (p) => tid(p, 'srs-run-view').isVisible(), 20_000);
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
