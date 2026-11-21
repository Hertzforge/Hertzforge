// The Modal Test on the simulator, end to end, driven through the installed Chrome the way an
// operator drives it, every check made on what the screen shows (and, against the real services,
// on the run folder the test left).
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/modal_test_sim.mjs [--mock] [--exe <sds-control>] [--python <python with hertzforge installed>]
//                                         [--data <dir>] [--shots <dir>] [--headed]
//
// With `--mock` it starts only the shell's dev server against the shell's own mock control service:
// the mock's numbers are made up so that every panel can be seen, and nothing it shows is a
// measurement. Without it, it runs the real control service (`--exe`, else it builds `sds-control`
// in release), the real analysis service (`python -m hertzforge serve`) and the shell's dev server,
// each on a free loopback port, and stops all three. Not part of `npm --prefix ui test`.
//
// 1. The chooser's Specialized section offers the Modal Test; it opens on Setup and takes control
//    on the simulator.
// 2. Station & Channels and DOFs: a hammer row (N, row 1) and two accelerometer rows (g, rows 3 and 4); in DOFs the
//    hammer is the reference at 1Z+, the accelerometers responses at 2Z+ and 3Z+. A setup with no
//    reference is refused in DOFs, in the service's words.
// 3. Roving: row 4 roves over four points typed in; the plan comes from the service's check.
// 4. Impact: automatic accept, no auto-advance, a force target with a tight tolerance.
// 5. Measure (impact): Proceed to Run, Start; Space arms; a simulated double hit is rejected in
//    words with its flag; hits come in; the counter and the tiles read the service's counts; a High
//    flag shows its percent and the hit is still accepted; R and Enter override the verdict; U
//    undoes; Right moves to the next point; End Test ends the test (screenshots, light and dark).
// 6. Measure (shaker): one shaker on the drive row, burst random; Start Driving; the frames count
//    up mid-average (screenshots, light and dark); Stop on the run bar ends it.
// 7. Against the real services only: the run folder holds the test's setup and outcome, and Open in
//    Analysis lands in the Modal workspace.
//
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
	start('analysis', PYTHON, ['-m', 'hertzforge', 'serve', '--port', String(analysisPort)], { env });
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
	await a.goto(URL_(theme));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()), 60_000);
	await tid(a, 'test-type-modal_test').click();
	await until(a, 'the Modal Test screen', async (p) => tid(p, 'modal-test-screen').isVisible());
	await tid(a, 'claim').click();
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
		const on = i <= 3;
		if (checked !== on) await a.locator(`[data-testid="channel-on-${i}"]`).setChecked(on, { force: true });
	}
	for (const i of [2, 3]) await tid(a, `channel-role-${i}`).selectOption('monitor');
	await fill(a.getByLabel('Channel 1 description', { exact: true }), shaker ? 'Force gauge' : 'Hammer');
	await fill(a.getByLabel('Channel 1 units', { exact: true }), 'N');
	for (const [i, d] of [[3, 'Accel A'], [4, 'Accel B']]) {
		await fill(a.getByLabel(`Channel ${i} description`, { exact: true }), d);
		await fill(a.getByLabel(`Channel ${i} units`, { exact: true }), 'g');
	}
	await a.waitForTimeout(300);
	if (await tid(a, 'table-apply').isEnabled()) await tid(a, 'table-apply').click();
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

	await section(a, 'roving');
	await tid(a, 'mt-rov-on').setChecked(true, { force: true });
	await a.locator('[data-testid="mt-rov-row"][data-row="4"]').setChecked(true);
	await fill(tid(a, 'mt-rov-text'), '3 Z+\n4 Z+\n5 Z+\n6 Z+');
	check('the typed points are read', (await text(a, 'mt-rov-count')) === '4 points read.', await text(a, 'mt-rov-count'));

	await section(a, 'excitation');
	await tid(a, 'mt-advance').setChecked(false, { force: true });
	await tid(a, 'mt-target-on').setChecked(true, { force: true });
	await fill(field(a, 'mt-target-peak'), 150);
	await fill(field(a, 'mt-target-tol'), 5);

	// Apply sends the table with its DOFs first, then the setup: the plan comes from the service.
	await until(a, 'the setup to check', async (p) => (await attr(p, 'mt-ready', 'data-ready')) === 'true' || (await tid(p, 'mt-summary-table').count()) === 1, 30_000);
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	if (full) {
		await section(a, 'roving');
		await until(a, 'the plan', async (p) => tid(p, 'mt-plan').isVisible(), 20_000);
		check('the plan has the four points from the service', (await a.locator('[data-testid="mt-plan"] tbody tr').count()) === 4);
		await shot(a, 'setup_roving_plan');
		await section(a, 'excitation');
		await until(a, 'the service windows', async (p) => tid(p, 'mt-fw-derived').isVisible(), 20_000);
		await shot(a, 'setup_impact');
		await tid(a, 'flow-run').click();
	}
	await tid(a, 'act-start').click();
	await until(a, 'the test at its first point', async (p) => (await measureState(p)) === 'idle', 30_000);
	check('the counter reads the service: 0 of 5 at point 1 of 4', (await text(a, 'mt-count')) === '0 of 5' && (await text(a, 'run-point')) === '1 of 4', `${await text(a, 'mt-count')}; ${await text(a, 'run-point')}`);

	await tid(a, 'mt-sim-double').click();
	await press(a, 'Space');
	await until(a, 'armed', async (p) => (await measureState(p)) === 'armed', 10_000);
	await until(a, 'the first hit', async (p) => (await tiles(p)).length >= 1, 20_000);
	const first = (await tiles(a))[0];
	check('the double hit is rejected, its flag in words', first.status === 'rejected' && /Double Hit/.test(first.text), first.text);
	check('the rejected count is the service count', (await text(a, 'mt-rejected')) === '1 rejected');
	check('the verdict says why in words', /Hit 1 Rejected: Double Hit/.test(await text(a, 'mt-verdict')), await text(a, 'mt-verdict'));

	// Hits come on; a High flag shows its percent and never rejects.
	await until(a, 'a High flag on an accepted hit', async (p) => (await tiles(p)).some((t) => t.status === 'accepted' && /High \+\d+%/.test(t.text)), 60_000);
	await until(a, 'three hits', async (p) => (await tiles(p)).length >= 3, 30_000);
	await press(a, 'Space');
	await until(a, 'disarmed', async (p) => (await measureState(p)) !== 'armed', 10_000);
	const ts = await tiles(a);
	const acc = ts.filter((t) => t.status === 'accepted').length;
	check('the counter is the service count of accepted hits', (await text(a, 'mt-count')) === `${acc} of 5`, `${await text(a, 'mt-count')} with ${acc} accepted tiles`);
	check('the target flag is shown with its percent', /High|Low|On Target/.test(await text(a, 'mt-target-flag').catch(() => '')), await text(a, 'mt-target-flag').catch(() => 'none'));
	await shot(a, theme === 'dark' ? 'dark_measure_impact' : 'measure_impact');
	if (!full) return a;

	// The keys override the service's verdict on the latest hit, and U undoes.
	const before = await text(a, 'mt-count');
	await press(a, 'r');
	await until(a, 'the latest hit rejected', async (p) => (await attr(p, 'mt-verdict', 'data-status')) === 'rejected', 10_000);
	check('R rejects the latest hit', (await text(a, 'mt-count')) !== before, `${before} to ${await text(a, 'mt-count')}`);
	await press(a, 'Enter');
	await until(a, 'the latest hit accepted', async (p) => (await attr(p, 'mt-verdict', 'data-status')) === 'accepted', 10_000);
	check('Enter accepts it again', (await text(a, 'mt-count')) === before, await text(a, 'mt-count'));
	await press(a, 'u');
	await until(a, 'the undo', async (p) => (await text(p, 'mt-count')) !== before, 10_000);
	check('U undoes the last accepted hit', true, `${before} to ${await text(a, 'mt-count')}`);
	await tid(a, 'mt-rov-text').count(); // the setup's fields are not on this screen, so typing cannot reach them
	await press(a, 'ArrowRight');
	await until(a, 'the next point', async (p) => (await text(p, 'run-point')) === '2 of 4', 10_000);
	check('Right moves to the next point', true);
	await press(a, 'ArrowLeft');
	await until(a, 'the previous point', async (p) => (await text(p, 'run-point')) === '1 of 4', 10_000);
	check('Left moves back', true);

	await tid(a, 'mt-end').count();
	await a.getByRole('button', { name: 'End Test' }).first().click();
	await until(a, 'the end', async (p) => (await attr(p, 'run-state', 'data-state')) === 'finished', 30_000);
	check('End Test ends the test in words', (await tid(a, 'mt-end').count()) === 1, await text(a, 'mt-end').catch(() => ''));
	await neverLive(a, 'after the test');
	await shot(a, 'measure_ended');
	if (!MOCK) {
		const runs = fs.readdirSync(DATA_ROOT).filter((f) => /^modal_test/.test(f));
		check('the test left its folder under the data folder', runs.length === 1, runs.join(', '));
		const run = path.join(DATA_ROOT, runs[0]);
		check("the run's setup holds the test's configuration", JSON.parse(fs.readFileSync(path.join(run, 'setup.json'), 'utf8')).config?.mode === 'impact');
		await tid(a, 'act-open-analysis').click();
		await until(a, 'the Modal workspace', async (p) => (await attr(p, 'rail-modal', 'aria-current')) === 'page', 180_000);
		check('Open in Analysis lands in the Modal workspace', true);
		await shot(a, 'in_analysis');
	}
	return a;
}

async function shaker(theme) {
	const a = await open(theme);
	await table(a, true);
	await section(a, 'acquisition');
	await tid(a, 'mt-mode-shaker').click();
	await fill(field(a, 'mt-averages'), 40);
	await section(a, 'dofs');
	await dof(a, 1, 1, 'reference');
	await dof(a, 3, 2, 'response');
	await dof(a, 4, 3, 'response');
	await section(a, 'excitation');
	await a.locator('[data-testid="mt-drive-row"][data-row="2"]').setChecked(true);
	await tid(a, 'mt-source').selectOption('burst_random');
	await until(a, 'the setup to check', async (p) => (await attr(p, 'mt-ready', 'data-ready')) === 'true' || (await tid(p, 'mt-summary-table').count()) === 1, 30_000);
	if (theme === 'light') await shot(a, 'setup_shaker');
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(a, 'act-start').click();
	await until(a, 'the test at its first point', async (p) => (await measureState(p)) === 'idle', 30_000);
	await tid(a, 'mt-drive_start').click();
	await until(a, 'frames mid-average', async (p) => {
		const m = /^(\d+) of (\d+)/.exec(await text(p, 'mt-count'));
		return !!m && Number(m[1]) >= 6;
	}, 60_000);
	check('the frames count up from the service, mid-average', /^\d+ of 40$/.test(await text(a, 'mt-count')), await text(a, 'mt-count'));
	check('the drive phase is in words', /Drive:/.test(await text(a, 'mt-drive-phase')), await text(a, 'mt-drive-phase'));
	await shot(a, theme === 'dark' ? 'dark_measure_shaker' : 'measure_shaker');
	await tid(a, 'act-stop').click();
	await until(a, 'the stop', async (p) => (await attr(p, 'run-state', 'data-state')) === 'finished', 30_000);
	check('Stop on the run bar ends the shaker test', true, await text(a, 'mt-end').catch(() => ''));
	await neverLive(a, 'after the shaker test');
	return a;
}

try {
	await impact('light', true);
	await impact('dark', false);
	await shaker('light');
	await shaker('dark');
} catch (err) {
	failed = err;
	console.error(err);
	if (page) await shot(page, 'stopped').catch(() => {});
} finally {
	await browser.close();
	stopAll();
}

const passed = results.filter((r) => r.ok).length;
console.log(`\nmodal test: ${passed} of ${results.length} checks passed${failed ? `; stopped: ${failed.message}` : ''}`);
console.log(`screenshots in ${SHOTS}`);
process.exit(failed ? 1 : 0);
