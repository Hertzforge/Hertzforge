// Sine on the simulator, end to end: the real control service and the shell, driven through the
// installed Chrome the way an operator drives them, every check made on what the screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/sine_sim.mjs [--shots <dir>] [--headed] [--law <file.wasm>] [--mock]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs Sine on the simulated station, and stops both. With `--mock` it starts only
// the shell, against its own mock control service, to try the flow without a build. The law
// imported in step 4 is `--law`, else the Sine template law `scripts/build_laws.sh` builds as a law
// host fixture (built first when it is missing), else the Sine servo law file. The run takes a few
// minutes. Not part of `npm test`.
//
// 1. The chooser offers Sine; its screen opens on Setup, and the screen takes control. The SIM
//    badge is checked at every step: a simulated run never shows LIVE.
// 2. Limits: the drive ceiling and abort, which have no default.
// 3. Profile: rows pasted as text are read by the service; it draws the profile, its velocity and
//    its displacement, a computation and never a measurement, with the kinematics; a sweep of 50
//    to 800 Hz at 8 oct/min, three levels of one sweep each, five sweeps in all; the setup checks clean.
// 4. Control: the Sine laws from the service; a law file imported by its path shows its parameters
//    from its manifest, its live ones marked; the built-in law is chosen again for the run.
// 5. Proceed to Run applies; Start identifies; the review hold brings the identified gain and the
//    coherence forward; Proceed.
// 6. The frequency readout rises on the service's own clock; the level card's words; a page opened
//    in a window of its own follows the run.
// 7. Hold keeps the frequency, Resume lets it go on, Reverse turns the sweep, Go to lands on its
//    frequency; a level step on the manual ladder; a live change of the prediction weight.
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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-sine'));
const HEADED = args.includes('--headed');
const MOCK = args.includes('--mock');
const PREFIX = flag('--prefix') ?? 'sine';
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
	await page.locator('#sine-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

/** A run page by its name. */
async function showPage(page, name) {
	const sel = page.locator('[data-testid="sine-page-select"]');
	const value = await sel.locator('option', { hasText: name }).first().getAttribute('value');
	await sel.selectOption(value);
	await until(page, `the ${name} page`, async (p) => (await p.locator('[data-testid="sine-page-view"]').getAttribute('data-page')) === value);
}

const runState = (page) => attr(page, 'sine-run-view', 'data-state');
const runPhase = (page) => attr(page, 'sine-run-view', 'data-phase');
const num = async (page, name) => Number((await attr(page, 'sine-run-view', name)) || NaN);
/** The tone's frequency and the service's station time, read together from the run view. */
const reading = async (page) => ({ hz: await num(page, 'data-hz'), t: await num(page, 'data-t') });

// ---- the run ----------------------------------------------------------------------------------
let servicePort = null;
let LAW = flag('--law') ?? null;
if (!MOCK) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	if (!LAW) {
		const wasm = (name) => path.join(TARGET, 'wasm32-wasip2', 'release', `${name}.wasm`);
		const pick = () => [wasm('law_fixture_sine_template'), wasm('hertzforge_law_sine_servo')].find((f) => fs.existsSync(f));
		if (!pick()) {
			console.log('building the law files');
			execFileSync('bash', [path.join(ROOT, 'scripts', 'build_laws.sh')], { cwd: ROOT, stdio: 'inherit' });
		}
		LAW = pick() ?? wasm('law_fixture_sine_template');
	}
	servicePort = await freePort();
	const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
	const service = start('sds-control', exe, ['--port', String(servicePort)]);
	await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
} else {
	LAW ??= path.join(os.tmpdir(), 'sine-template.wasm');
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

async function openSine(page, claim) {
	await page.goto(URL_('light'));
	await until(page, 'the chooser or the screen', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && ((await tid(p, 'test-type-chooser').isVisible()) || (await tid(p, 'sine-screen').isVisible())));
	if (await tid(page, 'test-type-chooser').isVisible()) await tid(page, 'test-type-sine').click();
	await until(page, 'the Sine screen', async (p) => tid(p, 'sine-screen').isVisible());
	if (claim) {
		await tid(page, 'claim').click();
		await until(page, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	}
}

/** Apply, start, proceed past the review hold when there is one, and wait for the tone. */
async function runFromSetup(page) {
	await tid(page, 'proceed-to-run').click();
	await until(page, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(page, 'act-start').click();
	await until(page, 'identification done', async (p) => ['review', 'finished'].includes(await runState(p)) || ['engaged', 'full_level'].includes(await runPhase(p)), 180_000);
	if ((await runState(page)) === 'review') await tid(page, 'act-proceed').click();
	await until(page, 'the tone engaged', async (p) => ['engaged', 'full_level'].includes(await runPhase(p)) && Number.isFinite(await num(p, 'data-hz')), 60_000);
}

try {
	const a = await ctx.newPage();
	await a.goto(URL_('light'));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers Sine', offered.includes('sine'), offered.join());
	await openSine(a, true);
	check('Sine opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- Limits first: with no drive ceiling and abort the setup is not checked by the service ------
	// The simulated station needs about 2 V a g, so a 0.5 g profile drives about 1 V.
	await section(a, 'limits');
	await fill(tid(a, 'sine-drive-ceiling'), 2);
	await fill(tid(a, 'sine-drive-abort'), 3);
	await shot(a, 'limits');

	// ---- Profile: pasted rows, read by the service, and drawn by it ----------------------------------
	await section(a, 'profile');
	await tid(a, 'sine-profile-paste').fill('# a flat half g\nfreq_hz, accel_g\n20, 0.5\n2000, 0.5\n');
	await tid(a, 'sine-profile-read-text').click();
	await until(a, 'the pasted rows', async (p) => /Read 2 rows/.test(await text(p, 'sine-profile-imported')), 20_000);
	check('pasted rows are read by the service into the table', (await a.locator('[data-testid^="sine-profile-rows-row-"]').count()) === 2, await text(a, 'sine-profile-imported'));
	await until(a, 'the profile drawn', async (p) => (await attr(p, 'sine-profile-preview', 'data-state')) === 'ok' && (await p.locator('[data-testid^="sine-profile-view-"]').count()) === 3, 30_000);
	check('the service draws the profile, its velocity and its displacement, as a computation', /never a measurement/.test(await text(a, 'sine-profile-preview')));
	const legend = await text(a, 'sine-profile-view-0');
	check('the profile carries its alarm and abort bands', /Profile/.test(legend) && /Alarm/.test(legend) && /Abort/.test(legend), legend.slice(0, 120));
	check('and its kinematics', /g pk/.test(await text(a, 'sine-kin-accel')) && /in pk-pk/.test(await text(a, 'sine-kin-disp')), await text(a, 'sine-kinematics'));
	await shot(a, 'profile');

	// ---- Test and schedule: a short sweep for the flow ---------------------------------------------
	await section(a, 'test');
	await tid(a, 'sine-kind-sweep').click();
	await fill(tid(a, 'sine-band-lo'), 50);
	await fill(tid(a, 'sine-band-hi'), 800);
	await fill(tid(a, 'sine-rate'), 8);
	await tid(a, 'sine-end-sweeps').click();
	await fill(tid(a, 'sine-sweeps'), 5);
	await section(a, 'schedule');
	await tid(a, 'sine-mode-auto').click();
	await fill(tid(a, 'sine-per_level-n'), 1);
	await fill(tid(a, 'sine-full_level-n'), 1);
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'sine-ready', 'data-ready')) === 'true', 30_000);
	check('the setup checks clean', true, await text(a, 'sine-schedule-time'));
	await shot(a, 'schedule');

	// ---- Control: the catalog, an imported law, back to the built-in --------------------------------
	await section(a, 'control');
	await until(a, 'the law catalog', async (p) => (await p.locator('[data-testid="law-list"] [role="radio"]').count()) > 0);
	check('the Sine laws come from the service, the built-in first', (await tid(a, 'law-servo_blend').count()) === 1);
	check('the built-in marks its live parameter', /Live/.test(await text(a, 'law-live')) && (await attr(a, 'law-live', 'data-live')) === 'blend_w', await text(a, 'law-live'));
	await tid(a, 'law-import-open').click();
	await fill(tid(a, 'law-path'), LAW);
	await tid(a, 'law-import-go').click();
	await until(a, 'the imported law', async (p) => (await tid(p, 'law-imported').count()) > 0 && (await attr(p, 'law-imported', 'aria-checked')) === 'true', 30_000);
	const params = await a.locator('[data-testid="law-params"] [data-testid^="law-param-"]').count();
	check('an imported law shows its parameters from its manifest, its live ones marked', params > 0 && (await tid(a, 'law-live').count()) === 1, `${params} parameters; ${await text(a, 'law-live')}`);
	await shot(a, 'control_imported');
	await tid(a, 'law-servo_blend').click();
	await shot(a, 'control');

	// ---- Run: identification, the review hold, the tone --------------------------------------------
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	check('Proceed to Run applies the setup: the run is configured', true);
	await tid(a, 'act-start').click();
	await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 180_000);
	check('the review hold says what it waits for', await tid(a, 'sine-review').isVisible());
	check('and brings the identified gain forward', (await a.locator('[data-testid="sine-page-view"]').getAttribute('data-kind')) === 'identification');
	await until(a, 'the identified gain', async (p) => {
		const t = await text(p, 'sine-page-view');
		return /Gain/.test(t) && /Coherence/.test(t);
	}, 20_000);
	check('the gain and the coherence are drawn', true, await text(a, 'sine-identification-facts'));
	await neverLive(a, 'review');
	await shot(a, 'review');
	await tid(a, 'act-proceed').click();
	await until(a, 'the tone engaged', async (p) => ['engaged', 'full_level'].includes(await runPhase(p)) && Number.isFinite(await num(p, 'data-t')), 60_000);
	const r0 = await reading(a);
	await until(a, 'the frequency to rise', async (p) => (await reading(p)).hz > r0.hz + 2, 30_000);
	const r1 = await reading(a);
	check('the frequency readout rises on the service clock', r1.hz > r0.hz && r1.t > r0.t, `${r0.hz} Hz at ${r0.t} s, then ${r1.hz} Hz at ${r1.t} s`);
	const words = await text(a, 'sine-level-status');
	check('the level card says what the run is doing', /^Sweeping (up|down) at [\d.]+ oct\/min at (full level, )?[-+]?\d+\.\d dB/.test(words) || /^Ramping to/.test(words), words);
	check('the sweep under way and the target against the measured amplitude', /^Sweep \d+( of \d+)?, (up|down)$/.test(await text(a, 'sine-sweep-words')) && / against .*dB$/.test(await text(a, 'sine-error-words')), `${await text(a, 'sine-sweep-words')}; ${await text(a, 'sine-error-words')}`);
	await showPage(a, 'Control');
	await until(a, 'the control page drawn', async (p) => /Profile/.test(await text(p, 'sine-page-view')) && /Control/.test(await text(p, 'sine-page-view')), 20_000);
	await neverLive(a, 'running');
	await shot(a, 'run_control');

	// A page in a window of its own follows the run.
	const [win] = await Promise.all([ctx.waitForEvent('page'), tid(a, 'sine-page-popout').click()]);
	await until(win, 'the display window', async (p) => tid(p, 'sine-run-view').isVisible(), 30_000);
	await until(win, 'the window to read the run', async (p) => Number.isFinite(await num(p, 'data-t')), 30_000);
	const w0 = await num(win, 'data-t');
	await until(win, 'the window to follow the run', async (p) => (await num(p, 'data-t')) > w0 + 1, 30_000);
	check('a page opened in a window of its own follows the run', true, `station time ${w0} then ${await num(win, 'data-t')} s`);
	await win.close();

	// ---- Hold, Resume, Reverse, Go to -------------------------------------------------------------
	// A hold is judged on the frames the service sends while held: their frequency on their own station time.
	await tid(a, 'act-hold').click();
	await until(a, 'a frame sent while held', async (p) => (await attr(p, 'sine-run-view', 'data-frame-held')) === 'true', 10_000);
	const h0 = await reading(a);
	await until(a, 'the station clock to run on while held', async (p) => (await num(p, 'data-t')) > h0.t + 2, 20_000);
	const h1 = await reading(a);
	check('Hold keeps the frequency while the station runs on', Math.abs(h1.hz - h0.hz) <= 0.01, `${h0.hz} Hz at ${h0.t} s, ${h1.hz} Hz at ${h1.t} s`);
	check('and says so', /^Holding at [\d.]+ Hz/.test(await text(a, 'sine-level-status')), await text(a, 'sine-level-status'));
	await tid(a, 'goto-hz').fill('300');
	await tid(a, 'act-goto').click();
	await until(a, 'the frequency at 300 Hz', async (p) => Math.abs((await num(p, 'data-hz')) - 300) <= 0.5, 10_000);
	check('Go to lands within 0.5 Hz', true, `${await num(a, 'data-hz')} Hz`);
	await tid(a, 'act-resume').click();
	await until(a, 'the resume', async (p) => (await attr(p, 'sine-run-view', 'data-held')) === 'false', 10_000);
	const dir0 = await attr(a, 'sine-run-view', 'data-direction');
	await tid(a, 'act-reverse').click();
	await until(a, 'the sweep to turn', async (p) => (await attr(p, 'sine-run-view', 'data-direction')) !== dir0, 10_000);
	check('Reverse turns the sweep', true, `${dir0} to ${await attr(a, 'sine-run-view', 'data-direction')}`);

	// ---- Manual level and a live parameter ------------------------------------------------------
	await tid(a, 'mode-manual').click();
	await until(a, 'the manual ladder', async (p) => (await attr(p, 'level-up', 'aria-disabled')) === 'false', 10_000);
	const levelBefore = await text(a, 'run-level');
	await tid(a, 'level-step').selectOption('1');
	await tid(a, (await text(a, 'run-level')).startsWith('0.0') ? 'level-down' : 'level-up').click();
	await until(a, 'the level to move', async (p) => (await text(p, 'run-level')) !== levelBefore, 10_000);
	check('a level step on the manual ladder moves the level', true, `${levelBefore} to ${await text(a, 'run-level')}`);
	await fill(tid(a, 'sine-live-blend_w-input'), 0.2);
	await tid(a, 'sine-live-blend_w-set').click();
	await until(a, 'the change in force', async (p) => (await p.locator('[data-testid="sine-live-blend_w-input"]').inputValue()) === '0.2' && (await attr(p, 'sine-live-blend_w-set', 'disabled')) !== null, 10_000);
	check('a live change of the prediction weight is in force', true);
	check('the fixed parameters stay fixed', (await a.locator('[data-testid^="sine-live-"][data-live="false"][data-enabled="true"]').count()) === 0);
	await shot(a, 'run_manual');

	// ---- An observer aborts; the controlling tab is closed ------------------------------------------
	// Both need one service shared by two tabs: the shell's mock lives in each tab, so `--mock` skips them.
	if (MOCK) {
		console.log('skip  the observer abort and the closed controlling tab: they need the control service');
		await tid(a, 'act-stop').click();
		await until(a, 'the stop', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'operator_stop', 20_000);
		check('Stop ramps the drive down and ends the run', /ramped down in [\d.]+ s/.test(await text(a, 'sine-end')), await text(a, 'sine-end'));
	} else {
		const b = await ctx.newPage();
		await openSine(b, false);
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
		await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 180_000);
		await tid(a, 'act-proceed').click();
		await until(a, 'the tone engaged', async (p) => ['engaged', 'full_level'].includes(await runPhase(p)), 60_000);
		await a.close();
		await until(b, 'the countdown', async (p) => tid(p, 'orphan').isVisible(), 20_000);
		check('with nobody in control the observer sees the countdown', true, await text(b, 'orphan'));
		const t0 = await num(b, 'data-t');
		await until(b, 'the run to go on', async (p) => (await num(p, 'data-t')) > t0 + 2, 20_000);
		check('the run goes on with nobody in control', true);
		await shot(b, 'orphan');
		await until(b, 'the safe stop', async (p) => (await attr(p, 'run-state', 'data-outcome')) === 'safe_stop', 90_000);
		check('the run safe-stopped with its ramp', /ramped down in [\d.]+ s/.test(await text(b, 'sine-end')), await text(b, 'sine-end'));
		await neverLive(b, 'after the safe stop');
	}

	// ---- The dark theme ---------------------------------------------------------------------------
	const d = await ctx.newPage();
	await d.goto(URL_('dark'));
	await until(d, 'the screen', async (p) => (await tid(p, 'sine-screen').isVisible()) || (await tid(p, 'test-type-chooser').isVisible()));
	if (await tid(d, 'test-type-chooser').isVisible()) await tid(d, 'test-type-sine').click();
	await until(d, 'the Sine screen', async (p) => tid(p, 'sine-screen').isVisible());
	await section(d, 'profile');
	await until(d, 'the profile drawn', async (p) => (await attr(p, 'sine-profile-preview', 'data-state')) === 'ok', 30_000);
	await shot(d, 'dark_profile');
	await tid(d, 'flow-run').click();
	await until(d, 'the run view', async (p) => tid(p, 'sine-run-view').isVisible(), 20_000);
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
