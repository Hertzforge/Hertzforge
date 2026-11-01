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
// 1. One screen takes control, selects the simulator, sets the channel table and the bench
//    loopback setup, sees the service draw the profile, applies, arms, starts, holds for review,
//    proceeds, steps the level on the manual ladder and stops with the ramp.
// 2. A second browser (its own storage) observes: it shows the setup the service holds and its
//    numbers; a new run starts and the observer aborts it with Ctrl and period.
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

async function tab(page, id) {
	await tid(page, `tab-${id}`).click();
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
const exe = path.join(ROOT, 'target', 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
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
	await a.goto(URL_);
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
	check('channel table: one control row and one drive row', (await a.locator('[data-testid^="channel-row-"]').count()) === 2);
	await fill(a, '#ch-0-sens', 1000);
	await fill(a, '#ch-0-range', 5);
	await fill(a, '#ch-1-range', 1);
	await tid(a, 'channel-more-0').click();
	const abortOff = a.locator('.nf', { has: a.locator('#ch-0-ab-g') }).locator('input[type="checkbox"]');
	if (await abortOff.isChecked()) await abortOff.uncheck();
	await fill(a, '#ch-0-ab-g', 1.0);
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

	await verb(a, 'act-arm');
	await until(a, 'armed', async (p) => (await stateOf(p)) === 'armed');
	const gridNote = await text(a, 'grid-note');
	check('Arm: armed, on the grid the station runs at', /Actual/.test(gridNote), gridNote);
	await simBadge(a, 'armed');
	await shot(a, '04-armed');

	await verb(a, 'act-start');
	await until(a, 'the review hold', async (p) => (await stateOf(p)) === 'review', 90_000);
	check('Start: identification holds for review', await tid(a, 'review-hold').isVisible());
	await shot(a, '05-review-hold');
	await tid(a, 'review-proceed').click();

	// The manual ladder: step the level once the run drives at the schedule.
	await until(a, 'running', async (p) => (await stateOf(p)) === 'running');
	await tid(a, 'mode-manual').click();
	await until(a, 'the manual ladder', async (p) => (await tid(p, 'level-up').getAttribute('aria-disabled')) === 'false', 90_000);
	check('Manual ladder: the level steps are offered while driving', true);
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
	await until(a, 'stopping', async (p) => ['stopping', 'finished'].includes(await stateOf(p)), 10_000);
	const sawStopping = (await stateOf(a)) === 'stopping';
	await until(a, 'finished', async (p) => (await stateOf(p)) === 'finished', 30_000);
	check('Stop: ramps down, then ends by the operator', (await outcomeOf(a)) === 'operator_stop', `stopping seen ${sawStopping}, outcome ${await outcomeOf(a)}`);
	check('the end is shown in words', /Stopped by the Operator/.test(await text(a, 'run-end')));
	await shot(a, '07-stopped-ramped');

	// ============ Run 2: a second browser observes, and aborts ==================================
	const b = await ctxB.newPage();
	await b.goto(URL_);
	await until(b, 'the observer to connect', async (p) => (await roleOf(p)) === 'observer');
	await tab(b, 'test');
	await until(b, 'the observer to read the setup back', async (p) => (await tid(p, 'setup-status').getAttribute('data-status')) === 'applied');
	check('second screen: shows the setup the service holds', (await b.locator('[data-testid="setup-title"]').inputValue()) === TITLE, await b.locator('[data-testid="setup-title"]').inputValue());
	check('second screen: the same derived numbers', Number(await text(b, 'derived-grms')) === grms, `${await text(b, 'derived-grms')} g rms`);
	check('second screen: the reference drawn by the service', (await tid(b, 'reference-preview').getAttribute('data-source')) === 'service');
	await shot(b, '08-observer-reads-setup');

	await verb(a, 'act-reset');
	await until(a, 'a new run', async (p) => (await stateOf(p)) === 'configured');
	await verb(a, 'act-start');
	await until(a, 'the review hold', async (p) => (await stateOf(p)) === 'review', 90_000);
	await until(b, 'the observer to see the review hold', async (p) => (await stateOf(p)) === 'review');
	check('observer: Proceed is not offered to a screen without control', (await tid(b, 'review-proceed').isDisabled()));
	await tid(a, 'review-proceed').click();
	await until(b, 'the observer to see the run drive', async (p) => (await stateOf(p)) === 'running' && (await tid(p, 'live-control').isVisible()), 60_000);
	await simBadge(b, 'observer, driving');
	check('observer: Abort is offered', (await tid(b, 'act-abort').getAttribute('aria-disabled')) === 'false');
	await b.keyboard.press('Control+Period');
	await until(b, 'the abort', async (p) => (await stateOf(p)) === 'finished', 5_000);
	check('observer: Ctrl and period aborts at once', (await outcomeOf(b)) === 'operator_abort', await outcomeOf(b));
	await until(a, 'the controller to see the end', async (p) => (await stateOf(p)) === 'finished');
	const endMsg = await text(a, 'run-end-message');
	check('controller: the end names the observer', (await outcomeOf(a)) === 'operator_abort' && /observer/i.test(endMsg), endMsg);
	await shot(b, '09-observer-aborted');
	await shot(a, '10-controller-sees-observer-abort');

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
