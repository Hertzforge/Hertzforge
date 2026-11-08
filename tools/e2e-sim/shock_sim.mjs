// Classical shock on the simulator, end to end: the real control service and the shell, driven
// through the installed Chrome the way an operator drives them, every check made on what the
// screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/shock_sim.mjs [--shots <dir>] [--headed] [--law <file.wasm>] [--mock]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs classical shock on the simulated station, and stops both. With `--mock` it
// starts only the shell, against its own mock control service, to try the flow without a build.
// The law imported in step 4 is `--law`, else the classical law file `scripts/build_laws.sh`
// builds (built first when it is missing). The run takes a few minutes. Not part of `npm test`.
//
// 1. The chooser offers Classical Shock; its screen opens on Setup, and the screen takes control.
//    The SIM badge is checked at every step: a simulated run never shows LIVE.
// 2. Pulse, a 1 g peak: the service draws the pulse with its tolerance band, velocity and displacement;
//    half-sine shoulders 1.5 times the duration leave the quiet window and say so; back to
//    rectangular shoulders four times the duration, inside it.
// 3. Identification in blocks of 8192 points; Limits: the drive ceiling and abort, which have no default.
// 4. Control: the classical laws from the service; a law file imported by its path shows its
//    parameters from its manifest; the built-in law is chosen again for the run.
// 5. Proceed to Run applies; Start identifies; the review hold brings |H| and the coherence
//    forward; Proceed.
// 6. Pulses fill the table and the Pulse page follows; the level card's words; a page opened in a
//    window of its own follows the run; two pulses picked in the table lay over the Pulse page;
//    the run completes.
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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-shock'));
const HEADED = args.includes('--headed');
const MOCK = args.includes('--mock');
const PREFIX = flag('--prefix') ?? 'shock';
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
	await page.locator('#sk-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

/** A run page by its name. */
async function showPage(page, name) {
	const sel = page.locator('[data-testid="shock-page-select"]');
	const value = await sel.locator('option', { hasText: name }).first().getAttribute('value');
	await sel.selectOption(value);
	await until(page, `the ${name} page`, async (p) => (await p.locator('[data-testid="shock-page-view"]').getAttribute('data-page')) === value);
}

const runState = (page) => attr(page, 'shock-run-view', 'data-state');
const runPhase = (page) => attr(page, 'shock-run-view', 'data-phase');
const pulsesFired = async (page) => Number((await attr(page, 'shock-run-view', 'data-pulses')) ?? '0');

// ---- the run ----------------------------------------------------------------------------------
let servicePort = null;
let LAW = flag('--law') ?? null;
if (!MOCK) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	if (!LAW) {
		LAW = path.join(TARGET, 'wasm32-wasip2', 'release', 'hertzforge_law_shock_fdc.wasm');
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
	LAW ??= path.join(os.tmpdir(), 'shock-template.wasm');
}
const shellPort = await freePort();
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], {
	shell: process.platform === 'win32'
});
const QUERY = MOCK ? '?mock=1&mock_speed=4' : `?control=ws://127.0.0.1:${servicePort}/`;
const URL_ = (theme) => `http://127.0.0.1:${shellPort}/${QUERY}&theme=${theme}#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`shell ${URL_('light')}${MOCK ? ' (mock control service)' : `, service ws://127.0.0.1:${servicePort}/`}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
let failed = null;

async function openShock(page, claim) {
	await page.goto(URL_('light'));
	await until(page, 'the chooser or the screen', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && ((await tid(p, 'test-type-chooser').isVisible()) || (await tid(p, 'shock-screen').isVisible())));
	if (await tid(page, 'test-type-chooser').isVisible()) await tid(page, 'test-type-shock').click();
	await until(page, 'the Classical Shock screen', async (p) => tid(p, 'shock-screen').isVisible());
	if (claim) {
		await tid(page, 'claim').click();
		await until(page, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	}
}

/** Apply, start, proceed past the review hold when there is one, and wait for the first pulse to be due. */
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
	check('the chooser offers Classical Shock', offered.includes('shock'), offered.join());
	await openShock(a, true);
	check('Classical Shock opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- Pulse ----------------------------------------------------------------------------------
	await section(a, 'pulse');
	// A 1 g peak: the simulated station needs about 2 V a g, so the full-level drive stays under
	// the 3 V drive abort set below.
	await fill(tid(a, 'shock-amplitude'), 1);
	await until(a, 'the preview', async (p) => (await attr(p, 'shock-preview', 'data-state')) === 'ok', 30_000);
	const legend = await text(a, 'shock-preview-view-0');
	check('the service draws the pulse with its tolerance band', /Reference/.test(legend) && /tolerance/i.test(legend), legend.slice(0, 120));
	check('and its velocity and displacement', (await a.locator('[data-testid^="shock-preview-view-"]').count()) === 3);
	check('rectangular shoulders four times the duration stay inside the quiet window', (await attr(a, 'shock-quiet', 'data-inside')) === 'true', await text(a, 'shock-quiet'));
	await shot(a, 'pulse');
	await tid(a, 'shock-comp-half_sine').click();
	await fill(tid(a, 'shock-pre'), 1.5);
	await until(a, 'the quiet window warning', async (p) => (await attr(p, 'shock-quiet', 'data-inside')) === 'false', 20_000);
	check('half-sine shoulders 1.5 times the duration leave the quiet window, in words', await tid(a, 'shock-quiet-note').isVisible(), await text(a, 'shock-quiet'));
	await shot(a, 'pulse_quiet_window');
	await tid(a, 'shock-comp-rectangular').click();
	await fill(tid(a, 'shock-pre'), 4);
	await until(a, 'back inside the quiet window', async (p) => (await attr(p, 'shock-quiet', 'data-inside')) === 'true', 20_000);

	// ---- Identification: blocks of 8192 points (6.25 Hz at 51 200 Hz). The simulated station's
	// low-frequency gain is not flat, and at 2048 points (25 Hz) the pulse's velocity change sits
	// about 20% low at the first level and the run ends not converged there.
	await section(a, 'identification');
	await tid(a, 'shock-id-nfft').selectOption('8192');

	// ---- Limits ---------------------------------------------------------------------------------
	await section(a, 'limits');
	await fill(tid(a, 'shock-drive-ceiling'), 2);
	await fill(tid(a, 'shock-drive-abort'), 3);
	await shot(a, 'limits');

	// ---- Control: the catalog, an imported law, back to the built-in --------------------------------
	await section(a, 'control');
	await until(a, 'the law catalog', async (p) => (await p.locator('[data-testid="law-list"] [role="radio"]').count()) > 0);
	check('the classical laws come from the service, the built-in first', (await tid(a, 'law-fd_correct').count()) === 1);
	await tid(a, 'law-import-open').click();
	await fill(tid(a, 'law-path'), LAW);
	await tid(a, 'law-import-go').click();
	await until(a, 'the imported law', async (p) => (await tid(p, 'law-imported').count()) > 0 && (await attr(p, 'law-imported', 'aria-checked')) === 'true', 30_000);
	const params = await a.locator('[data-testid="law-params"] [data-testid^="law-param-"]').count();
	check('an imported law shows its parameters from its manifest', params > 0, `${params} parameters`);
	await shot(a, 'control_imported');
	await tid(a, 'law-fd_correct').click();
	await shot(a, 'control');

	// ---- Run: identification, the review hold, pulses -------------------------------------------
	await section(a, 'pulse');
	await until(a, 'nothing missing before a run', async (p) => (await attr(p, 'shock-ready', 'data-ready')) === 'true', 30_000);
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	check('Proceed to Run applies the setup: the run is configured', true);
	await tid(a, 'act-start').click();
	await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 120_000);
	check('the review hold says what it waits for', await tid(a, 'shock-review').isVisible());
	check('and brings the identified system forward', (await a.locator('[data-testid="shock-page-view"]').getAttribute('data-kind')) === 'identification');
	await until(a, 'the identified system', async (p) => {
		const t = await text(p, 'shock-page-view');
		return /\|H\||Gain/i.test(t) && /Coherence/.test(t);
	}, 20_000);
	check('|H| and the coherence are drawn', true);
	await neverLive(a, 'review');
	await shot(a, 'review');
	await tid(a, 'act-proceed').click();
	await until(a, 'three pulses', async (p) => (await pulsesFired(p)) >= 3, 120_000);
	await showPage(a, 'Pulse');
	const pulsePage = await text(a, 'shock-page-view');
	check('the Pulse page follows the run: reference, band and measured pulse', /Reference/.test(pulsePage) && /Measured/.test(pulsePage), pulsePage.slice(0, 120));
	const words = await text(a, 'shock-level-card');
	check('the level card counts pulses against the plan', /Pulse \d+ of \d+ at [-+]?\d+\.\d dB|Completed/.test(words), words.slice(0, 160));
	check('and says how the last pulse went', /Pulse \d+ (passed|was \d+(\.\d+)?% (low|high)|missed|failed)/.test(await text(a, 'shock-last-pulse')), await text(a, 'shock-last-pulse'));
	await neverLive(a, 'running');
	await shot(a, 'run_pulse');

	// A page in a window of its own follows the run.
	const [win] = await Promise.all([ctx.waitForEvent('page'), tid(a, 'shock-page-popout').click()]);
	await until(win, 'the display window', async (p) => tid(p, 'shock-run-view').isVisible(), 30_000);
	const before = await pulsesFired(win);
	await until(win, 'the window to follow a pulse', async (p) => (await pulsesFired(p)) > before || (await runState(p)) === 'finished', 120_000);
	check('a page opened in a window of its own follows the run', true, `${before} then ${await pulsesFired(win)} pulses`);
	await win.close();

	// Two pulses laid over the Pulse page.
	await showPage(a, 'Pulses');
	await tid(a, 'shock-overlay-0').check();
	await tid(a, 'shock-overlay-1').check();
	await showPage(a, 'Pulse');
	await until(a, 'the overlay', async (p) => /Pulse 1, measured/.test(await text(p, 'shock-page-view')) && /Pulse 2, measured/.test(await text(p, 'shock-page-view')), 20_000);
	check('two pulses picked in the table lay over the Pulse page', true);
	await shot(a, 'run_overlay');
	await until(a, 'the end of the schedule', async (p) => (await runState(p)) === 'finished', 300_000);
	check('the automatic schedule completes', (await attr(a, 'run-state', 'data-outcome')) === 'completed', await attr(a, 'run-state', 'data-outcome'));
	await showPage(a, 'Pulses');
	await shot(a, 'run_pulses');

	// ---- Manual -----------------------------------------------------------------------------------
	await tid(a, 'back-to-setup').click();
	await section(a, 'schedule');
	await tid(a, 'shock-mode-manual').click();
	await section(a, 'identification');
	await tid(a, 'shock-id-review').setChecked(false, { force: true });
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
	const levelBefore = await text(a, 'shock-level-now');
	await until(a, 'the level step offered', async (p) => (await attr(p, 'shock-level-up', 'aria-disabled')) === 'false', 30_000);
	await tid(a, 'shock-level-up').click();
	await until(a, 'the level to move', async (p) => (await text(p, 'shock-level-now')) !== levelBefore, 10_000);
	check('a 1 dB step moves the level', true, `${levelBefore} to ${await text(a, 'shock-level-now')}`);
	await a.locator('[data-testid="shock-level-goto"]').selectOption({ index: 1 });
	await until(a, 'the first level', async (p) => /-12\.0 dB/.test(await text(p, 'shock-level-now')), 10_000);
	check('Go to level moves to a level of the table', true, await text(a, 'shock-level-now'));
	await shot(a, 'run_manual');

	// ---- An observer aborts; the controlling tab is closed ------------------------------------------
	// Both need one service shared by two tabs: the shell's mock lives in each tab, so `--mock` skips them.
	if (MOCK) console.log('skip  the observer abort and the closed controlling tab: they need the control service');
	else {
		const b = await ctx.newPage();
		await openShock(b, false);
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
	await until(d, 'the screen', async (p) => (await tid(p, 'shock-screen').isVisible()) || (await tid(p, 'test-type-chooser').isVisible()));
	if (await tid(d, 'test-type-chooser').isVisible()) await tid(d, 'test-type-shock').click();
	await until(d, 'the Classical Shock screen', async (p) => tid(p, 'shock-screen').isVisible());
	await section(d, 'pulse');
	await until(d, 'the preview', async (p) => (await attr(p, 'shock-preview', 'data-state')) === 'ok', 30_000);
	await shot(d, 'dark_pulse');
	await tid(d, 'flow-run').click();
	await until(d, 'the run view', async (p) => tid(p, 'shock-run-view').isVisible(), 20_000);
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
