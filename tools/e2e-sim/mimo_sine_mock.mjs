// MIMO Sine's screens against the shell's own mock control service, driven through the installed
// Chrome the way an operator drives them, every check made on what the screen shows. It needs no
// build: it starts only the shell's dev server (a free port) and stops it.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/mimo_sine_mock.mjs [--shots <dir>] [--headed]
//
// The mock's numbers are made up so that every panel can be seen; nothing here is a measurement,
// and the run against the real control service is a flow of its own. Not part of `npm test`.
//
// 1. The chooser shows MIMO Sine as coming; opened by name (`preview=mimo_sine`) it is offered, and
//    its screen opens on Setup and takes control. The SIM badge is checked at every step.
// 2. Station & Channels: the run's controls and drives from the channel table; each drive's ceiling
//    and abort, which have no default.
// 3. References and Test: each control's reference; the test is a sweep.
// 4. Control: the laws that cannot run a sweep are listed apart, each with why in words, and the
//    chosen law states its limit.
// 5. Proceed to Run applies; Start identifies; the review hold brings the identification forward
//    with the sweep-rate warning; Proceed with words for the log.
// 6. The frequency readout moves; the sweep offers Jog and Go to and no Reverse (the sweep turns
//    only at the band's edges); Hold keeps it and Resume lets it go on; the run completes.
// 7. A dwell: the sweep verbs are gone; Abort ends it in words.
//
// Screenshots (1,440 by 900): `mimo_sine_<name>.png`.

import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-mimo-sine'));
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });

const children = [];
process.on('exit', () => {
	for (const c of children) if (c.exitCode === null) c.kill('SIGTERM');
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

async function shot(page, name) {
	const file = path.join(SHOTS, `mimo_sine_${name}.png`);
	await page.waitForTimeout(300);
	await page.screenshot({ path: file });
	console.log(`      screenshot ${file}`);
}

async function neverLive(page, where) {
	check(`no LIVE badge (${where})`, (await page.locator('[data-testid="live-badge"]').count()) === 0);
}

async function section(page, id) {
	if (!(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

const runState = (page) => attr(page, 'msine-run-view', 'data-state');
const runPhase = (page) => attr(page, 'msine-run-view', 'data-phase');
const hzNow = async (page) => Number((await attr(page, 'msine-run-view', 'data-hz')) || NaN);

const port = await freePort();
children.push(spawn(process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(port), '--strictPort'], { cwd: ROOT, stdio: 'ignore', shell: process.platform === 'win32' }));
const URL_ = (extra = '') => `http://127.0.0.1:${port}/?mock=1&mock_speed=4&theme=light${extra}#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${port}/`)).ok, 60_000);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
let failed = null;

async function startRun(page) {
	await tid(page, 'proceed-to-run').click();
	await until(page, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step', 30_000);
	await tid(page, 'act-start').click();
	await until(page, 'the review hold', async (p) => (await runState(p)) === 'review', 30_000);
}

try {
	const a = await ctx.newPage();
	// ---- 1. coming, then by name -------------------------------------------------------------------
	await a.goto(URL_());
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	check('the chooser shows MIMO Sine as coming', (await attr(a, 'test-type-mimo_sine', 'aria-disabled')) === 'true');
	await a.goto(URL_('&preview=mimo_sine'));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	check('opened by name, the chooser offers MIMO Sine', (await attr(a, 'test-type-mimo_sine', 'aria-disabled')) === 'false');
	await tid(a, 'test-type-mimo_sine').click();
	await until(a, 'the MIMO Sine screen', async (p) => tid(p, 'mimo-sine-screen').isVisible());
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	check('MIMO Sine opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- 2. channels -------------------------------------------------------------------------------
	await section(a, 'channels');
	await until(a, 'the drives', async (p) => tid(p, 'msine-drives').isVisible());
	await tid(a, 'msine-drive-ceiling-0').fill('2');
	await tid(a, 'msine-drive-abort-0').fill('3');
	await tid(a, 'msine-drive-abort-0').blur();
	await shot(a, 'channels');

	// ---- 3. references and test --------------------------------------------------------------------
	await section(a, 'references');
	check('each control has its reference', await tid(a, 'msine-reference-1').isVisible());
	await section(a, 'test');
	check('the test is a sweep', await tid(a, 'msine-kind-sweep').isChecked());
	await shot(a, 'test');

	// ---- 4. the laws ---------------------------------------------------------------------------------
	await section(a, 'law');
	await until(a, 'the law list', async (p) => tid(p, 'law-list').isVisible());
	const why = await text(a, 'msine-refused-secant');
	check('a law that cannot run a sweep is listed apart, with why', /Not for a sweep: secant accepts a dwell only\./.test(why), why);
	check('the chosen law states its limit', /a quarter of its half-power bandwidth/.test(await text(a, 'msine-law-limit')));
	await shot(a, 'law');
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'msine-ready', 'data-ready')) === 'true', 30_000);

	// ---- 5. identification and the review ------------------------------------------------------------
	await startRun(a);
	check('the review shows the sweep-rate warning', /a slower sweep is the remedy/.test(await text(a, 'msine-rate-warnings')));
	await tid(a, 'msine-ack').fill('Reviewed the identification.');
	await shot(a, 'review');
	await tid(a, 'review-proceed').click();
	await until(a, 'the tone engaged', async (p) => ['engaged', 'full_level'].includes(await runPhase(p)) && Number.isFinite(await hzNow(p)), 30_000);
	await neverLive(a, 'run');

	// ---- 6. the frequency moves; hold and resume; completion ---------------------------------------
	const f0 = await hzNow(a);
	await until(a, 'the frequency moving', async (p) => (await hzNow(p)) !== f0, 15_000);
	check('the frequency readout moves', true);
	check('a sweep offers Jog and Go to, and no Reverse', (await tid(a, 'jog-up').count()) === 1 && (await tid(a, 'act-goto').count()) === 1 && (await a.getByRole('button', { name: 'Reverse' }).count()) === 0);
	await tid(a, 'act-hold').click();
	await until(a, 'held', async (p) => (await attr(p, 'msine-run-view', 'data-held')) === 'true', 10_000);
	await shot(a, 'held');
	await tid(a, 'act-resume').click();
	await until(a, 'resumed', async (p) => (await attr(p, 'msine-run-view', 'data-held')) === 'false', 10_000);
	await until(a, 'the run finished', async (p) => (await runState(p)) === 'finished', 120_000);
	check('the sweep completes', /Completed/.test(await text(a, 'msine-end')));
	await shot(a, 'finished');

	// ---- 7. a dwell, aborted ---------------------------------------------------------------------------
	await section(a, 'test');
	await tid(a, 'msine-kind-dwell').check();
	await section(a, 'schedule');
	// a dwell holds each level for seconds: sweeps count in a sweep only, and the setup says so
	check('the schedule asks for seconds in a dwell', (await a.locator('#msine-panel, [data-testid="msine-schedule"]').first().innerText()).includes('Sweeps count in a sweep test only'));
	await a.locator('#msine-per_level-by').selectOption('seconds');
	await a.locator('#msine-full_level-by').selectOption('seconds');
	await until(a, 'the setup checked clean', async (p) => (await attr(p, 'msine-ready', 'data-ready')) === 'true', 30_000);
	await startRun(a);
	await tid(a, 'review-proceed').click();
	await until(a, 'the dwell engaged', async (p) => ['engaged', 'full_level'].includes(await runPhase(p)), 30_000);
	check('a dwell has no sweep verbs', (await tid(a, 'act-goto').count()) === 0 && (await tid(a, 'jog-up').count()) === 0);
	await tid(a, 'act-abort').click();
	await until(a, 'the run aborted', async (p) => (await runState(p)) === 'finished', 15_000);
	check('Abort ends the run in words', /Aborted by the Operator/.test(await text(a, 'msine-end')));
} catch (err) {
	failed = err;
	console.error(err);
} finally {
	await browser.close();
	const passed = results.filter((r) => r.ok).length;
	console.log(`\n${passed} of ${results.length} checks passed${failed ? '; the flow stopped on a failure' : ''}.`);
	process.exit(failed ? 1 : 0);
}
