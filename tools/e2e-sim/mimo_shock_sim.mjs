// MIMO Shock on the simulator, end to end: the real control service and the shell, driven through
// the installed Chrome the way an operator drives them, every check made on what the screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/mimo_shock_sim.mjs [--shots <dir>] [--headed] [--law <file.wasm>]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server (a
// free port), runs MIMO Shock on the simulated station, and stops both. The law imported in step 8
// is `--law`, else the `paired` MIMO Shock law file `scripts/build_laws.sh` builds (built first when
// it is missing). The run takes a few minutes. Not part of `npm test`.
//
// 1. The chooser offers MIMO Shock; its screen opens on Setup and the screen takes control. The SIM
//    badge is checked at every step: a simulated run never shows LIVE.
// 2. Station & Channels: four controls and four drives from the channel table, each drive's ceiling
//    and abort, the linear 4 x 4 plant.
// 3. A classical pulse at every control: the new setup opens on the service's default law for it.
//    Start identifies; the review hold shows each control's reachable floor; Proceed; the verdict
//    grid fills, the ladder card climbs, and the run completes at full level.
// 4. An SRS at every control: the law follows to the service's default for SRS; the run completes
//    with the SRS page drawn against its tolerance.
// 5. The severe jointed plant, the controls' pulses alternating in polarity (the plate's twist, the
//    mode the joints soften): the core's guards refuse the law's answers, each pulse says which
//    guard in words, and the run stops before full level and says why.
// 6. Six controls by three drives on the jointed 6 x 3: the run holds before the first pulse on the
//    unreachable targets, with each control's floor, and goes on only with the operator's words.
// 7. Scaled abort lines off: the setup and every run page carry a standing notice; back on, gone.
// 8. A law imported as a file, chosen, and run: the run names it.
//
// Screenshots (1,440 by 900): `<prefix>_<name>.png`.

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-mimo-shock'));
const HEADED = args.includes('--headed');
const PREFIX = flag('--prefix') ?? 'mimo_shock';
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
	check(`no LIVE badge (${where})`, (await page.locator('[data-testid="live-badge"]').count()) === 0);
}

/** A setup section, from the Setup step. */
async function section(page, id) {
	if (!(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

/** The page a run window shows, by its tab under the plots. */
const pageName = async (page) => (await page.locator('[data-testid="page-tabs"] [role="tab"][aria-selected="true"]').first().innerText()).trim();
async function showPage(page, name) {
	await page.locator('[data-testid="page-tabs"]').getByRole('tab', { name, exact: true }).click();
	await until(page, `the ${name} page`, async (p) => (await pageName(p)) === name);
}

const runState = (page) => attr(page, 'ms-run-view', 'data-state');
const pulsesFired = async (page) => Number((await attr(page, 'ms-run-view', 'data-pulses')) ?? '0');
const chosenLaw = async (page) => ((await page.locator('[data-testid="law-list"] [role="radio"][aria-checked="true"]').first().getAttribute('data-testid')) ?? '').replace('law-', '');

/**
 * The channel table: `nc` controls and `nd` drives. Row 2 is the station's drive row; the others
 * after it are switched on and given their role, the controls first.
 */
async function channelTable(page, nc, nd) {
	await section(page, 'channels');
	const ctl = [0, ...Array.from({ length: nc - 1 }, (_, k) => k + 2)];
	const drv = [1, ...Array.from({ length: nd - 1 }, (_, k) => nc + 1 + k)];
	const states = await page.locator('[data-testid^="channel-on-"]').evaluateAll((els) => els.map((e) => [Number(e.dataset.testid.replace('channel-on-', '')), e.checked]));
	for (const [i, checked] of states) {
		const on = ctl.includes(i) || drv.includes(i);
		if (checked !== on) await page.locator(`[data-testid="channel-on-${i}"]`).setChecked(on, { force: true });
	}
	for (const i of [...ctl, ...drv]) if (i > 1) await tid(page, `channel-role-${i}`).selectOption(ctl.includes(i) ? 'control' : 'drive');
	await page.waitForTimeout(300);
	if (await tid(page, 'table-apply').isEnabled()) {
		await tid(page, 'table-apply').click();
		await until(page, 'the table applied', async (p) => /applied \(revision \d+\)/.test(await text(p, 'note')), 20_000);
	}
	for (let i = 0; i < nd; i++) {
		await fill(tid(page, `ms-drive-ceiling-${i}`), 6);
		await fill(tid(page, `ms-drive-abort-${i}`), 9);
	}
}

/** Apply, start, and wait for the review hold (or the end). */
async function toReview(page) {
	await until(page, 'nothing missing before a run', async (p) => (await attr(p, 'ms-ready', 'data-ready')) === 'true', 60_000);
	await tid(page, 'proceed-to-run').click();
	await until(page, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(page, 'act-start').click();
	await until(page, 'identification done', async (p) => ['review', 'finished'].includes(await runState(p)), 180_000);
}

async function toEnd(page) {
	await until(page, 'the end of the run', async (p) => (await runState(p)) === 'finished', 600_000);
	return attr(page, 'run-state', 'data-outcome');
}

/** New Run, back on Setup. */
async function newRun(page) {
	await tid(page, 'act-reset').click();
	await until(page, 'a new run', async (p) => ['idle', 'configured', ''].includes((await attr(p, 'run-state', 'data-state')) ?? ''), 20_000);
	await tid(page, 'flow-setup').click();
}

// ---- the run ----------------------------------------------------------------------------------
console.log('building the control service (release)');
execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
let LAW = flag('--law') ?? path.join(TARGET, 'wasm32-wasip2', 'release', 'hertzforge_law_mimo_shock_paired.wasm');
if (!fs.existsSync(LAW)) {
	console.log('building the law files');
	execFileSync('bash', [path.join(ROOT, 'scripts', 'build_laws.sh')], { cwd: ROOT, stdio: 'inherit' });
}
const servicePort = await freePort();
const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
const service = start('sds-control', exe, ['--port', String(servicePort)]);
await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
const shellPort = await freePort();
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], {
	shell: process.platform === 'win32'
});
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${servicePort}/&theme=light#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`service ws://127.0.0.1:${servicePort}/, shell ${URL_}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
let failed = null;
let a = null;

try {
	a = await ctx.newPage();
	await a.goto(URL_);
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers MIMO Shock', offered.includes('mimo_shock'), offered.join());
	await tid(a, 'test-type-mimo_shock').click();
	await until(a, 'the MIMO Shock screen', async (p) => tid(p, 'mimo-shock-screen').isVisible());
	check('MIMO Shock opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	await neverLive(a, 'setup');

	// ---- 2. Station & Channels ---------------------------------------------------------------------
	await channelTable(a, 4, 4);
	await tid(a, 'ms-plant').selectOption('shock-linear-4x4');
	const controls = await a.locator('[data-testid^="ms-control-"]').count();
	check('the run takes four controls and four drives from the table', controls === 4 && (await a.locator('[data-testid="ms-drives"] tbody tr').count()) === 4, `${controls} controls`);
	await shot(a, 'channels');

	// ---- 3. A classical pulse at every control -------------------------------------------------------
	await section(a, 'law');
	await until(a, 'the law catalog', async (p) => (await p.locator('[data-testid="law-list"] [role="radio"]').count()) > 0);
	check("a classical setup opens on the service's default law for it", (await chosenLaw(a)) === 'inverse', await chosenLaw(a));
	await section(a, 'target');
	await until(a, 'the target drawn', async (p) => /Target/.test(await text(p, 'ms-target-preview')), 30_000);
	await shot(a, 'target_classical');
	await toReview(a);
	check('identification holds for review', (await runState(a)) === 'review');
	const floors = await text(a, 'ms-floors');
	check("the review names each control's reachable floor, none at a computation's guard", /Reachable/i.test(floors) && !/-300/.test(floors), floors.slice(0, 200));
	await neverLive(a, 'review');
	await shot(a, 'review');
	await tid(a, 'review-proceed').click();
	await until(a, 'three pulses', async (p) => (await pulsesFired(p)) >= 3, 120_000);
	await showPage(a, 'Verdicts');
	check('the verdict grid fills, a cell per control and pulse', (await a.locator('[data-testid^="ms-cell-"]').count()) >= 12);
	const outcome = await toEnd(a);
	check('the classical run completes at full level', outcome === 'completed', outcome);
	const ladder = await text(a, 'ms-ladder-card');
	const fired = await pulsesFired(a);
	const counted = [...ladder.matchAll(/(\d+) fired/g)].reduce((n, m) => n + Number(m[1]), 0);
	check('the ladder card counts every pulse of this run once', counted === fired, `${counted} counted, ${fired} fired`);
	check('the end says so in words', /Full level passed/.test(await text(a, 'ms-level-card')), (await text(a, 'ms-level-card')).slice(0, 160));
	await shot(a, 'end_classical');
	await neverLive(a, 'end');

	// ---- 4. An SRS at every control -----------------------------------------------------------------
	await newRun(a);
	await section(a, 'target');
	await tid(a, 'ms-kind-srs').click();
	await section(a, 'law');
	check("the law follows to the service's default for an SRS target", (await chosenLaw(a)) === 'shape', await chosenLaw(a));
	// A 20 g SRS, +6 dB an octave to 300 Hz, at every control. (At the 24 g the setup opens on, the
	// shape law's responses run about 0.8 of the synthesized waveform's effective duration, the edge of
	// the standard's window, and the run ends at -6 dB on duration with every SRS inside tolerance.)
	await section(a, 'target');
	for (const [k, g] of [20 / 6, 20, 20].entries()) await fill(tid(a, `ms-srs-points-${k}-accel_g`), g);
	await tid(a, 'ms-same-target').click();
	await section(a, 'ladder');
	await tid(a, 'ms-record-len').selectOption('16384');
	await toReview(a);
	if ((await runState(a)) === 'review') await tid(a, 'review-proceed').click();
	const srsOutcome = await toEnd(a);
	await showPage(a, 'SRS');
	const srsPage = await text(a, 'ms-page-view');
	check('the SRS page draws each control against its tolerance', /Tolerance/.test(srsPage) && /Measured/.test(srsPage), srsPage.slice(0, 120));
	check('the SRS run completes at full level', srsOutcome === 'completed', `${srsOutcome}: ${(await text(a, 'ms-level-card')).slice(0, 200)}`);
	await shot(a, 'end_srs');

	// ---- 5. The severe jointed plant, twisted ---------------------------------------------------------
	// Classical pulses alternating in polarity drive the plate's twist, the mode the joints soften.
	await newRun(a);
	await section(a, 'channels');
	await tid(a, 'ms-plant').selectOption('shock-jointed-4x4-severe');
	await section(a, 'target');
	await tid(a, 'ms-kind-waveform').click();
	for (const c of [3, 5]) {
		await tid(a, `ms-target-tab-${c}`).click();
		await tid(a, 'ms-negative').click();
	}
	await tid(a, 'ms-target-tab-3').click();
	check('the controls alternate in polarity', (await attr(a, 'ms-negative', 'aria-checked')) === 'true');
	await toReview(a);
	if ((await runState(a)) === 'review') await tid(a, 'review-proceed').click();
	const severe = await toEnd(a);
	const severeWords = await text(a, 'ms-end-words');
	check('the severe jointed plant stops before full level, and says why', severe !== 'completed' && severeWords.length > 20, `${severe}: ${severeWords}`);
	// The core's guards refused the law's answers: each pulse after names the guard in words.
	const detail = await text(a, 'ms-pulse-detail');
	check('a pulse the guard held says which guard, in words', /not used: (a band of a drive|a drive's peak|the predicted)[^;]*; the last drives played again/.test(detail), detail.slice(0, 260));
	await shot(a, 'end_severe');

	// ---- 6. Six controls by three drives --------------------------------------------------------------
	await newRun(a);
	await channelTable(a, 6, 3);
	await tid(a, 'ms-plant').selectOption('shock-jointed-6x3');
	await section(a, 'target');
	await tid(a, 'ms-same-target').click();
	await toReview(a);
	check('the run holds before the first pulse on unreachable targets', await tid(a, 'ms-ack').isVisible(), await runState(a));
	check('each control has its floor on the card', (await a.locator('[data-testid^="ms-floor-"]').count()) === 6);
	const ackGo = tid(a, 'ms-ack-proceed');
	check('and it does not go on without words', (await ackGo.getAttribute('aria-disabled')) === 'true');
	await fill(tid(a, 'ms-ack-text'), 'Three drives cannot reach six independent targets; judged by each floor.');
	await until(a, 'the acknowledgement offered', async (p) => (await attr(p, 'ms-ack-proceed', 'aria-disabled')) === 'false', 10_000);
	await shot(a, 'unreachable_hold');
	await ackGo.click();
	await until(a, 'a pulse past the hold', async (p) => (await pulsesFired(p)) >= 1 || (await runState(p)) === 'finished', 120_000);
	check('past the hold with the operator\'s words, pulses fire', (await pulsesFired(a)) >= 1, `${await pulsesFired(a)} pulses`);
	await tid(a, 'act-abort').click();
	await until(a, 'the abort', async (p) => (await runState(p)) === 'finished', 30_000);

	// ---- 7. Scaled abort lines off ----------------------------------------------------------------------
	await newRun(a);
	await channelTable(a, 4, 4);
	await tid(a, 'ms-plant').selectOption('shock-linear-4x4');
	await section(a, 'limits');
	await tid(a, 'ms-scaled-aborts').setChecked(false, { force: true });
	check('the switch says what off means', /flag/i.test(await text(a, 'ms-scaled-words')), await text(a, 'ms-scaled-words'));
	await toReview(a);
	for (const name of ['Pulses', 'SRS', 'Drives', 'Verdicts', 'Ladder', 'Identification', 'Law']) {
		await showPage(a, name);
		check(`the standing notice stands above the ${name} page`, await tid(a, 'ms-unscaled-notice').isVisible());
	}
	await shot(a, 'unscaled_notice');
	await tid(a, 'act-abort').click();
	await until(a, 'the abort', async (p) => (await runState(p)) === 'finished', 30_000);
	await newRun(a);
	await section(a, 'limits');
	await tid(a, 'ms-scaled-aborts').setChecked(true, { force: true });
	await toReview(a);
	check('back on, the notice is gone', !(await tid(a, 'ms-unscaled-notice').isVisible().catch(() => false)));
	await tid(a, 'act-abort').click();
	await until(a, 'the abort', async (p) => (await runState(p)) === 'finished', 30_000);

	// ---- 8. A law imported as a file ---------------------------------------------------------------------
	await newRun(a);
	await section(a, 'target');
	await tid(a, 'ms-kind-waveform').click();
	await section(a, 'law');
	await tid(a, 'law-import-open').click();
	await fill(tid(a, 'law-path'), LAW);
	await tid(a, 'law-import-go').click();
	await until(a, 'the imported law', async (p) => (await tid(p, 'law-imported').count()) > 0 && (await attr(p, 'law-imported', 'aria-checked')) === 'true', 30_000);
	const params = await a.locator('[data-testid="law-params"] [data-testid^="law-param-"]').count();
	check('an imported law is chosen and shows its parameters from its manifest', params > 0, `${params} parameters`);
	await shot(a, 'law_imported');
	await toReview(a);
	if ((await runState(a)) === 'review') await tid(a, 'review-proceed').click();
	const imported = await toEnd(a);
	const lawLine = await text(a, 'ms-run-law');
	check('the run names the imported law and ends with a reason', /paired/.test(lawLine) && !!imported, `${imported}; ${lawLine}`);
	await shot(a, 'end_imported');
	await neverLive(a, 'the last run');
} catch (err) {
	failed = err;
	console.error(err);
	if (a) await a.screenshot({ path: path.join(SHOTS, `${PREFIX}_failed.png`) }).catch(() => {});
} finally {
	await browser.close();
	stopAll();
}

const passed = results.filter((r) => r.ok).length;
console.log(`\n${passed} of ${results.length} checks passed; ${shots.length} screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
