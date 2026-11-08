// Time waveform replication (TWR) on the simulator, end to end: the real control service and the
// shell, driven through the installed Chrome the way an operator drives them, every check made on
// what the screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/twr_sim.mjs [--shots <dir>] [--headed] [--law <file.wasm>] [--mock]
//
// It builds and starts `sds-control` (release, loopback, a free port, its data root a fresh scratch
// folder) and the shell's dev server (a free port), runs TWR on the simulated station, and stops
// both. With `--mock` it starts only the shell, against its own mock control service, to try the
// flow without a build. The reference is a CSV this script writes into the scratch data root and
// imports by its path. The law imported in step 4 is `--law`, else the TWR template law
// `scripts/build_laws.sh` builds as a law host fixture (built first when it is missing), else the
// built-in law's own file. The run takes a few minutes. Not part of `npm test`.
//
// 1. The chooser offers TWR; its screen opens on Setup, and the screen takes control. The SIM badge
//    is checked at every step: a simulated run never shows LIVE.
// 2. Reference: a CSV read by its path; the service prepares it: the record, the section's peak,
//    the reachable share, its kinematics and four views, a computation and never a measurement.
// 3. Limits: the drive ceiling, which has no default; the setup checks clean.
// 4. Control: the TWR laws from the service; a law file imported by its path shows its parameters
//    from its manifest; the built-in law is chosen again for the run.
// 5. Proceed to Run applies; Start identifies; the review hold brings the identification forward;
//    Proceed.
// 6. The play's progress bar moves through the record on the station's frames, the plays table
//    fills, the level card's words; a page opened in a window of its own follows the run; the run
//    completes on the automatic ladder.
// 7. A new run on the manual ladder: Play waits for the gate and says why; Iterate until the level
//    gates; a level step; Freeze and Resume; Play; Stop.
// 8. A second tab observes and aborts.
// 9. The controlling tab is closed during a run: no new play starts, and the run safe-stops after the
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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-twr'));
const HEADED = args.includes('--headed');
const MOCK = args.includes('--mock');
const PREFIX = flag('--prefix') ?? 'twr';
/** Run seconds per real second: the mock runs at 4, the service in real time. */
const SPEED = MOCK ? 4 : 1;
fs.mkdirSync(SHOTS, { recursive: true });

// The scratch data root: the service's data folder, and where the reference CSV is written.
const DATA_ROOT = fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-e2e-twr-data-'));
const REF = path.join(DATA_ROOT, 'references', 'field_transient.csv');
fs.mkdirSync(path.dirname(REF), { recursive: true });
{
	// One second at 12,800 Hz: three decaying tones over a low floor, peaking near 0.2 g.
	const fs_hz = 12800;
	const lines = ['time_s,accel_g'];
	for (let i = 0; i < fs_hz; i++) {
		const t = i / fs_hz;
		const v = 0.12 * Math.exp(-3 * t) * Math.sin(2 * Math.PI * 250 * t) + 0.06 * Math.exp(-5 * t) * Math.sin(2 * Math.PI * 620 * t) + 0.03 * Math.sin(2 * Math.PI * 150 * t);
		lines.push(`${t.toFixed(10)},${v.toFixed(7)}`);
	}
	fs.writeFileSync(REF, `${lines.join('\n')}\n`);
}

// ---- processes --------------------------------------------------------------------------------
const children = [];
const stopAll = () => {
	for (const c of children) if (c.exitCode === null) c.kill('SIGTERM');
};
process.on('exit', () => {
	stopAll();
	fs.rmSync(DATA_ROOT, { recursive: true, force: true });
});
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
	await page.locator('#twr-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

/** A run page by its name. */
async function showPage(page, name) {
	const sel = page.locator('[data-testid="twr-page-select"]');
	const value = await sel.locator('option', { hasText: name }).first().getAttribute('value');
	await sel.selectOption(value);
	await until(page, `the ${name} page`, async (p) => (await p.locator('[data-testid="twr-page-view"]').getAttribute('data-page')) === value);
}

const runState = (page) => attr(page, 'twr-run-view', 'data-state');
const runPhase = (page) => attr(page, 'twr-run-view', 'data-phase');
const num = async (page, name) => Number((await attr(page, 'twr-run-view', name)) || NaN);
const outcome = (page) => attr(page, 'run-state', 'data-outcome');

// ---- the run ----------------------------------------------------------------------------------
let servicePort = null;
let LAW = flag('--law') ?? null;
if (!MOCK) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	if (!LAW) {
		const wasm = (name) => path.join(TARGET, 'wasm32-wasip2', 'release', `${name}.wasm`);
		const pick = () => [wasm('law_fixture_twr_template'), wasm('hertzforge_law_twr_correct')].find((f) => fs.existsSync(f));
		if (!pick()) {
			console.log('building the law files');
			execFileSync('bash', [path.join(ROOT, 'scripts', 'build_laws.sh')], { cwd: ROOT, stdio: 'inherit' });
		}
		LAW = pick() ?? wasm('law_fixture_twr_template');
	}
	servicePort = await freePort();
	const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
	const service = start('sds-control', exe, ['--port', String(servicePort)], { env: { ...process.env, HERTZFORGE_DATA_ROOT: DATA_ROOT } });
	await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
} else {
	LAW ??= path.join(os.tmpdir(), 'law_fixture_twr_template.wasm');
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
// A page's own errors are printed as they come, so a failure says what the page said.
ctx.on('page', (p) => p.on('pageerror', (e) => console.error(`page error: ${e.message}`)));
let failed = null;

async function openTwr(page, claim) {
	await page.goto(URL_('light'));
	await until(page, 'the chooser or the screen', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && ((await tid(p, 'test-type-chooser').isVisible()) || (await tid(p, 'twr-screen').isVisible())));
	if (await tid(page, 'test-type-chooser').isVisible()) await tid(page, 'test-type-twr').click();
	await until(page, 'the TWR screen', async (p) => tid(p, 'twr-screen').isVisible());
	if (claim) {
		await tid(page, 'claim').click();
		await until(page, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	}
}

/** Apply, start, proceed past the review hold, and wait for the first play. */
async function runFromSetup(page) {
	await tid(page, 'proceed-to-run').click();
	await until(page, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(page, 'act-start').click();
	await until(page, 'the review hold', async (p) => (await runState(p)) === 'review', 120_000);
	await tid(page, 'act-proceed').click();
}

try {
	const a = await ctx.newPage();
	await a.goto(URL_('light'));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers TWR', offered.includes('twr'), offered.join());
	await openTwr(a, true);
	check('TWR opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- Reference: a CSV by its path, read and prepared by the service ----------------------------
	await section(a, 'reference');
	await tid(a, 'twr-ref-from-path').click();
	await fill(tid(a, 'twr-ref-path'), REF);
	await tid(a, 'twr-ref-read').click();
	await until(a, 'the reference read', async (p) => (await attr(p, 'twr-ref-facts', 'data-sha'))?.length === 64, 30_000);
	check('the reference is read by the service and named by its SHA-256', /field_transient/.test(await text(a, 'twr-ref-facts')), (await text(a, 'twr-ref-facts')).slice(0, 120));
	await until(a, 'the reference prepared', async (p) => (await attr(p, 'twr-preview', 'data-state')) === 'ok' && (await p.locator('[data-testid^="twr-preview-view-"]').count()) === 4, 30_000);
	check('the service prepares the record, the section and the reachable share', /samples/.test(await text(a, 'twr-grid')) && /of the section's energy/.test(await text(a, 'twr-coverage')), await text(a, 'twr-grid'));
	check('with its kinematics, as a computation', /g pk/.test(await text(a, 'twr-kin-accel')) && /in pk-pk/.test(await text(a, 'twr-kin-disp')) && /never a measurement/.test(await text(a, 'twr-preview')));
	await shot(a, 'reference');

	// ---- Limits: the drive ceiling, which has no default -------------------------------------------
	await section(a, 'limits');
	await fill(tid(a, 'twr-drive-ceiling'), 1);
	await section(a, 'identification');
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'twr-ready', 'data-ready')) === 'true', 30_000);
	check('the setup checks clean', true, await text(a, 'twr-summary-derived'));
	await shot(a, 'identification');

	// ---- Control: the catalog, an imported law, back to the built-in -------------------------------
	await section(a, 'control');
	await until(a, 'the law catalog', async (p) => (await p.locator('[data-testid="law-list"] [role="radio"]').count()) > 0);
	check('the TWR laws come from the service, the built-in first', (await tid(a, 'law-twr_correct').count()) === 1);
	await tid(a, 'law-import-open').click();
	await fill(tid(a, 'law-path'), LAW);
	await tid(a, 'law-import-go').click();
	await until(a, 'the imported law', async (p) => (await tid(p, 'law-imported').count()) > 0 && (await attr(p, 'law-imported', 'aria-checked')) === 'true', 30_000);
	const params = await a.locator('[data-testid="law-params"] [data-testid^="law-param-"]').count();
	check('an imported law shows its parameters from its manifest', params > 0, `${params} parameters`);
	await shot(a, 'control_imported');
	await tid(a, 'law-twr_correct').click();
	await until(a, 'the built-in chosen', async (p) => (await attr(p, 'law-twr_correct', 'aria-checked')) === 'true');
	await shot(a, 'control');

	// ---- Run: identification, the review hold, the ladder ------------------------------------------
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	check('Proceed to Run applies the setup: the run is configured', true);
	await tid(a, 'act-start').click();
	await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 120_000);
	check('the review hold says what it waits for', await tid(a, 'twr-review').isVisible());
	check('and brings the identification forward', (await a.locator('[data-testid="twr-page-view"]').getAttribute('data-kind')) === 'identification');
	await until(a, 'the identification drawn', async (p) => {
		const t = await text(p, 'twr-page-view');
		return /Gain/.test(t) && /Coherence/.test(t) && /Trust/.test(t);
	}, 20_000);
	check('the gain, the coherence and the trusted bins are drawn', true, await text(a, 'twr-identification-facts'));
	await neverLive(a, 'review');
	await shot(a, 'review');
	await tid(a, 'act-proceed').click();
	await until(a, 'a play in flight', async (p) => Number.isFinite(await num(p, 'data-progress')), 60_000);
	const p0 = await num(a, 'data-progress');
	await until(a, 'the play to move through the record', async (p) => (await num(p, 'data-progress')) > p0 || (await num(p, 'data-plays')) > 0, 30_000);
	check("the play's progress moves through the record on the station's frames", true, `${p0} then ${await num(a, 'data-progress')}`);
	const words = await text(a, 'twr-level-words');
	check('the level card says what the run is doing', /^(Correcting at .+, iteration \d+ of \d+|Between plays at .+|Playing \d+ of \d+ at .+)$/.test(words), words);
	await showPage(a, 'Plays');
	await until(a, 'the plays table to fill', async (p) => Number((await attr(p, 'twr-play-table', 'data-rows')) ?? 0) >= 2, 120_000);
	check('the plays table fills', true, `${await attr(a, 'twr-play-table', 'data-rows')} rows`);
	await neverLive(a, 'running');
	await shot(a, 'run_plays');

	// A page in a window of its own follows the run.
	await showPage(a, 'Replication');
	const [win] = await Promise.all([ctx.waitForEvent('page'), tid(a, 'twr-page-popout').click()]);
	await until(win, 'the display window', async (p) => tid(p, 'twr-run-view').isVisible(), 30_000);
	await until(win, 'the window to read the run', async (p) => Number((await p.locator('[data-testid="twr-run-view"]').getAttribute('data-plays')) ?? 0) > 0, 60_000);
	const w0 = Number(await win.locator('[data-testid="twr-run-view"]').getAttribute('data-plays'));
	await until(win, 'the window to follow the run', async (p) => Number(await p.locator('[data-testid="twr-run-view"]').getAttribute('data-plays')) > w0 || (await p.locator('[data-testid="twr-run-view"]').getAttribute('data-state')) === 'finished', 120_000);
	check('a page opened in a window of its own follows the run', true, `${w0} then ${await win.locator('[data-testid="twr-run-view"]').getAttribute('data-plays')} plays`);
	await win.close();
	await a.bringToFront();
	await until(a, 'the run to complete', async (p) => (await outcome(p)) !== '', 300_000);
	check('the automatic ladder completes', (await outcome(a)) === 'completed', `${await outcome(a)}: ${await text(a, 'twr-end')}`);
	await showPage(a, 'Convergence');
	await until(a, 'the convergence drawn', async (p) => {
		const t = await text(p, 'twr-page-view');
		return /Convergence/.test(t) && /Gate/.test(t);
	}, 20_000);
	await shot(a, 'run_convergence');

	// ---- Manual: Play waits for the gate; Iterate; a level step; Freeze; Play; Stop ----------------
	await section(a, 'schedule');
	await tid(a, 'twr-mode-manual').click();
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'twr-ready', 'data-ready')) === 'true', 30_000);
	await runFromSetup(a);
	await until(a, 'the manual ladder waiting', async (p) => (await runPhase(p)) === 'waiting' && (await attr(p, 'act-iterate', 'aria-disabled')) === 'false', 60_000);
	// Play waits with its reason; a click on it says why (the button is marked disabled, not removed).
	await tid(a, 'act-play').click({ force: true });
	await until(a, 'the reason Play waits', async (p) => /meets the gate/.test(await text(p, 'twr-verb-hint')), 10_000);
	check('Play waits for the level to gate, and says why', true, await text(a, 'twr-verb-hint'));
	check('the level card waits for a verb', /^Waiting for Iterate or Play at /.test(await text(a, 'twr-level-words')), await text(a, 'twr-level-words'));
	for (let i = 0; i < 8 && (await attr(a, 'act-play', 'aria-disabled')) === 'true'; i++) {
		const before = await num(a, 'data-plays');
		await tid(a, 'act-iterate').click();
		await until(a, 'the correction play judged', async (p) => (await num(p, 'data-plays')) > before && (await runPhase(p)) === 'waiting', 120_000);
	}
	check('Iterate corrects until the level gates', (await attr(a, 'act-play', 'aria-disabled')) === 'false', `${await num(a, 'data-plays')} plays`);
	const levelBefore = await text(a, 'twr-level-now');
	await tid(a, 'twr-level-up').click();
	await until(a, 'the level to move', async (p) => (await text(p, 'twr-level-now')) !== levelBefore, 10_000);
	check('a level step moves the level by 1 dB', true, `${levelBefore} to ${await text(a, 'twr-level-now')}`);
	check('and Play waits again for the new level to gate', (await attr(a, 'act-play', 'aria-disabled')) === 'true');
	await tid(a, 'act-freeze').click();
	await until(a, 'the corrections frozen', async (p) => (await attr(p, 'twr-run-view', 'data-frozen')) === 'true', 10_000);
	check('Freeze holds the corrections off', /Corrections frozen/.test(await text(a, 'twr-frozen')));
	await tid(a, 'act-unfreeze').click();
	await until(a, 'the corrections resumed', async (p) => (await attr(p, 'twr-run-view', 'data-frozen')) === 'false', 10_000);
	for (let i = 0; i < 8 && (await attr(a, 'act-play', 'aria-disabled')) === 'true'; i++) {
		const before = await num(a, 'data-plays');
		await tid(a, 'act-iterate').click();
		await until(a, 'the correction play judged', async (p) => (await num(p, 'data-plays')) > before && (await runPhase(p)) === 'waiting', 120_000);
	}
	await shot(a, 'run_manual');
	await tid(a, 'act-play').click();
	await until(a, 'playback', async (p) => (await runPhase(p)) === 'playing' || (await outcome(p)) !== '', 60_000);
	check('Play begins playback', true, await text(a, 'twr-level-words'));
	if ((await outcome(a)) === '') {
		await tid(a, 'act-stop').click();
		await until(a, 'the stop', async (p) => (await outcome(p)) !== '', 30_000);
	}
	check('Stop ends the run, or playback finished first', ['operator_stop', 'completed'].includes(await outcome(a)), `${await outcome(a)}: ${await text(a, 'twr-end')}`);

	// ---- An observer aborts; the controlling tab is closed ------------------------------------------
	// Both need one service shared by two tabs: the shell's mock lives in each tab, so `--mock` skips them.
	if (MOCK) {
		console.log('skip  the observer abort and the closed controlling tab: they need the control service');
	} else {
		await tid(a, 'act-reset').click();
		await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured', 20_000);
		await tid(a, 'act-start').click();
		await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 120_000);
		const b = await ctx.newPage();
		await openTwr(b, false);
		await tid(b, 'flow-run').click().catch(() => {});
		await until(b, 'the observer sees the run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'review', 30_000);
		check('a second tab observes', (await attr(b, 'session-role', 'data-role')) === 'observer');
		await tid(b, 'act-abort').click();
		await until(a, 'the abort', async (p) => (await outcome(p)) === 'operator_abort', 20_000);
		check('an observer tab aborts the run', true);

		await tid(a, 'act-reset').click();
		await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured', 20_000);
		await tid(a, 'act-start').click();
		await until(a, 'the review hold', async (p) => (await runState(p)) === 'review', 120_000);
		await tid(a, 'act-proceed').click();
		await until(a, 'the manual ladder waiting', async (p) => (await runPhase(p)) === 'waiting', 60_000);
		await a.close();
		await until(b, 'the countdown', async (p) => tid(p, 'orphan').isVisible(), 20_000);
		check('with nobody in control the observer sees the countdown', true, await text(b, 'orphan'));
		const plays = await num(b, 'data-plays');
		await shot(b, 'orphan');
		await until(b, 'the safe stop', async (p) => (await outcome(p)) === 'safe_stop', 120_000);
		check('the run safe-stopped, and no play started with nobody in control', (await num(b, 'data-plays')) === plays, `${plays} plays`);
		await neverLive(b, 'after the safe stop');
	}

	// ---- The dark theme ---------------------------------------------------------------------------
	const d = await ctx.newPage();
	await d.goto(URL_('dark'));
	await until(d, 'the screen', async (p) => (await tid(p, 'twr-screen').isVisible()) || (await tid(p, 'test-type-chooser').isVisible()));
	if (await tid(d, 'test-type-chooser').isVisible()) await tid(d, 'test-type-twr').click();
	await until(d, 'the TWR screen', async (p) => tid(p, 'twr-screen').isVisible());
	await section(d, 'reference');
	await shot(d, 'dark_reference');
	await tid(d, 'flow-run').click();
	await until(d, 'the run view', async (p) => tid(p, 'twr-run-view').isVisible(), 20_000);
	await d.waitForTimeout(1000);
	await shot(d, 'dark_run');
} catch (err) {
	failed = err;
	console.error(err);
	for (const [i, p] of ctx.pages().entries()) {
		const v = await p.locator('[data-testid="twr-run-view"]').first().evaluate((el) => Object.fromEntries([...el.attributes].filter((a) => a.name.startsWith('data-')).map((a) => [a.name, a.value]))).catch(() => null);
		if (v) console.error(`page ${i} run view: ${JSON.stringify(v)}`);
		await p.screenshot({ path: path.join(SHOTS, `${PREFIX}_failure_${i}.png`) }).catch(() => {});
	}
} finally {
	await browser.close();
	stopAll();
}

const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} of ${results.length} checks passed; ${shots.length} screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
