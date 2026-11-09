// MIMO Random on the simulator, end to end: the real control service and the shell, driven
// through the installed Chrome the way an operator drives them, every check made on what the
// screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/mimo_sim.mjs [--shots <dir>] [--headed]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs one MIMO Random test on the simulator's four-by-four plant, and stops both.
// The simulator runs in real time; the run takes about two minutes. Not part of `npm test`.
//
// 1. The chooser offers MIMO Random; its screen opens on Setup, and the screen takes control.
// 2. Channels: four controls and four drives, each drive's ceiling set; the role table, the
//    control space, the summary beside it.
// 3. Reference: the breakpoint tables (one per control) and a pair with its coherence and phase;
//    the service builds it and draws every PSD, the smallest eigenvalue per line and the pair. A
//    triple of pairs that cannot hold is refused line by line, and accepted only with the repair.
// 4. Control: the law catalog from the service, the chosen law's details and parameter form; the
//    frame and identification set short for the simulator.
// 5. Schedule (the single-output editor) and Backend (the station and feasibility).
// 6. The summary lists nothing missing; Proceed to Run applies; Start runs the noise floor and
//    identification; the Predict page comes forward with the gates, each drive against its
//    ceiling and each control's error; Predict Again; Engage; the run reaches the ladder.
// 7. The Run step hosts the run view: each of its four pages during the run (error heatmap,
//    matrix grid, drive and plant, geometry), a page opened in a window of its own, then the
//    shell's dark theme over the run pages, the Predict page and every setup section; Stop.
//
// Screenshots (1,440 by 900): every section and the predict page, `<prefix>_<name>.png`; the run
// pages, `<prefix>_run_<page>.png`; the dark theme, `<prefix>_dark_<name>.png` (prefix `wp13`).

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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-mimo'));
const HEADED = args.includes('--headed');
const PREFIX = flag('--prefix') ?? 'wp13';
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

/** A setup section, from the Setup step. */
async function section(page, id) {
	if (!(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
	await page.locator('#mm-panel').evaluate((el) => el.scrollTo({ top: 0 }));
}

/** The reference the service last built, settled (not building, current with the edits). */
async function referenceSettled(page) {
	await until(page, 'the reference to settle', async (p) => {
		const s = await attr(p, 'mimo-reference-preview', 'data-state');
		return s === 'ok' || s === 'refused';
	}, 30_000);
	await page.waitForTimeout(600);
	return attr(page, 'mimo-reference-preview', 'data-state');
}

// ---- the run ----------------------------------------------------------------------------------
console.log('building the control service (release)');
execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
const servicePort = await freePort();
const shellPort = await freePort();
const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
const service = start('sds-control', exe, ['--port', String(servicePort)]);
await waitFor('the control service', () => service.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], {
	shell: process.platform === 'win32'
});
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${servicePort}/&theme=light#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`service ws://127.0.0.1:${servicePort}/, shell ${URL_}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } });
let failed = null;

try {
	const a = await ctx.newPage();
	await a.goto(URL_);
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers Random, Sine, Classical Shock, SRS, TWR and MIMO Random', offered.join() === 'random,sine,shock,srs,twr,mimo_random', offered.join());
	await tid(a, 'test-type-mimo_random').click();
	await until(a, 'the MIMO Random screen', async (p) => tid(p, 'mimo-screen').isVisible());
	check('MIMO Random opens on Setup, Channels', (await attr(a, 'flow-setup', 'aria-current')) === 'step' && (await attr(a, 'tab-channels', 'aria-selected')) === 'true');
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');

	// ---- Channels: four controls, four drives ---------------------------------------------------
	const roleCount = async (role) => a.locator(`[data-testid^="mimo-row-"][data-role="${role}"]`).count();
	while ((await roleCount('control')) < 4) await tid(a, 'mimo-add-input').click();
	while ((await roleCount('drive')) < 4) await tid(a, 'mimo-add-drive').click();
	const rows = await a.locator('[data-testid^="mimo-row-"]').evaluateAll((els) => els.map((e) => ({ id: e.dataset.testid.replace('mimo-row-', ''), role: e.dataset.role })));
	check('four controls and four drives, nothing else', rows.filter((r) => r.role === 'control').length === 4 && rows.filter((r) => r.role === 'drive').length === 4 && rows.length === 8, rows.map((r) => `${r.id}:${r.role}`).join(' '));
	// Name the rows plainly, so the plots and tables read as a bench would.
	let k = 0;
	let d = 0;
	for (const r of rows) {
		const name = r.role === 'control' ? `Control ${++k}` : `Shaker ${String.fromCharCode(64 + ++d)}`;
		await fill(a.locator(`[data-testid="mimo-row-${r.id}"] input[aria-label$=" name"]`), name);
		if (r.role === 'drive') {
			await fill(a.locator(`[data-testid="mimo-drive-${r.id}-ceiling"]`), 5);
			await fill(a.locator(`[data-testid="mimo-drive-${r.id}-peak"]`), 10);
		}
	}
	// A role conflict is named before anything is sent: a duplicate name.
	const firstControl = rows.find((r) => r.role === 'control').id;
	const secondControl = rows.filter((r) => r.role === 'control')[1].id;
	await fill(a.locator(`[data-testid="mimo-row-${secondControl}"] input[aria-label$=" name"]`), 'Control 1');
	await until(a, 'the duplicate name named', async (p) => (await p.locator(`[data-testid="mimo-row-${secondControl}"] + tr.aq-findrow, [data-testid="mimo-row-${secondControl}"] ~ tr.aq-findrow`).first().innerText()).includes('also named'));
	check('a duplicate control name is named on its row', true);
	await fill(a.locator(`[data-testid="mimo-row-${secondControl}"] input[aria-label$=" name"]`), 'Control 2');
	check('the channel count reads back', (await text(a, 'mimo-channel-count')).startsWith('4 controls, 4 drives'), await text(a, 'mimo-channel-count'));
	await shot(a, 'channels');

	// The virtual space and its grid, then back to physical.
	await tid(a, 'space-virtual').click();
	await fill(tid(a, 'space-dofs'), 'Heave, Pitch, Roll');
	await until(a, 'the transformation grid', async (p) => tid(p, 'tr-0-0').isVisible());
	await a.locator('[data-testid="tr-0-0"]').focus();
	await a.evaluate(() => {
		const el = document.querySelector('[data-testid="tr-0-0"]');
		const dt = new DataTransfer();
		dt.setData('text/plain', '0.25\t0.25\t0.25\t0.25\n0.5\t0.5\t-0.5\t-0.5\n0.5\t-0.5\t0.5\t-0.5\n');
		el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
	});
	const pasted = await a.locator('[data-testid="tr-1-2"]').inputValue();
	check('a spreadsheet block pastes into the response transformation', pasted === '-0.5', `cell (1,2) ${pasted}`);
	await tid(a, 'mimo-space').scrollIntoViewIfNeeded();
	await shot(a, 'channels_virtual');
	await tid(a, 'space-physical').click();

	// ---- Reference --------------------------------------------------------------------------------
	await section(a, 'reference');
	await tid(a, 'ref-tab-breakpoints').click();
	let state = await referenceSettled(a);
	check('the service builds the reference from the four breakpoint tables', state === 'ok', state);
	check('the preview draws every PSD, the smallest eigenvalue per line, and a pair', (await a.locator('[data-testid^="preview-view-"]').count()) === 4);
	// Three pairs at coherence 0.9 with phases 0, 0 and 180 degrees cannot all hold.
	for (let i = 0; i < 3; i++) await tid(a, 'pair-add').click();
	const pairsSet = [
		[0, 0],
		[0, 0],
		[0, 180]
	];
	for (let i = 0; i < 3; i++) {
		await fill(tid(a, `pair-${i}-coherence`), 0.9);
		await fill(tid(a, `pair-${i}-phase`), pairsSet[i][1]);
	}
	// Pairs added in order: (0,1), (0,2), (0,3); make the third (1,2) so the triple closes.
	await a.locator('[data-testid="pair-2"] select').first().selectOption('1');
	await a.locator('[data-testid="pair-2"] select').nth(1).selectOption('2');
	state = await referenceSettled(a);
	const report = await text(a, 'psd-report');
	check('a triple that cannot hold is refused, line by line, with the repair described', state === 'refused' && /cannot all hold/.test(report) && (await a.locator('[data-testid="repair-changes"] tbody tr').count()) > 0, report.slice(0, 160));
	await tid(a, 'pair-2-preview').click();
	await referenceSettled(a);
	await tid(a, 'psd-report').scrollIntoViewIfNeeded();
	await shot(a, 'reference_repair');
	await tid(a, 'accept-repair').click();
	state = await referenceSettled(a);
	check('the repaired reference is accepted once the repair is', state === 'ok' && (await attr(a, 'psd-report', 'data-accepted')) === 'true', state);
	// Back to one plain pair for the run: coherence 0.5, phase 30 degrees.
	await a.locator('[data-testid="pair-table"] button[aria-label="Remove pair 3"]').click();
	await a.locator('[data-testid="pair-table"] button[aria-label="Remove pair 2"]').click();
	await fill(tid(a, 'pair-0-coherence'), 0.5);
	await fill(tid(a, 'pair-0-phase'), 30);
	await tid(a, 'pair-0-preview').click();
	state = await referenceSettled(a);
	check('one pair at coherence 0.5 and 30 degrees builds clean', state === 'ok', state);
	await a.locator('#mm-panel').evaluate((el) => el.scrollTo({ top: 0 }));
	await shot(a, 'reference');
	await tid(a, 'ref-tab-import').click();
	await shot(a, 'reference_import');
	await tid(a, 'ref-tab-rigid').click();
	await a.waitForTimeout(800);
	await shot(a, 'reference_rigid');
	await tid(a, 'ref-tab-field').click();
	await shot(a, 'reference_field');
	await tid(a, 'ref-tab-breakpoints').click();
	state = await referenceSettled(a);
	check('back on Breakpoints the reference is the one built', state === 'ok', state);

	// ---- Control ----------------------------------------------------------------------------------
	await section(a, 'control');
	await until(a, 'the law catalog', async (p) => (await p.locator('[data-testid="law-list"] [role="radio"]').count()) > 0);
	const laws = await a.locator('[data-testid="law-list"] [role="radio"]').count();
	check('the law chooser lists the built-in laws from the service', laws >= 1, `${laws} laws`);
	check('the chosen law shows its details and its parameter form from its manifest', (await tid(a, 'law-details').isVisible()) && (await a.locator('[data-testid="law-params"] [data-testid^="law-param-"]').count()) > 0);
	await fill(tid(a, 'mimo-fs'), 4096);
	await a.locator('#mc-npts').selectOption('1024');
	await fill(tid(a, 'mimo-frames_per_loop'), 8);
	await fill(tid(a, 'mimo-frames'), 32);
	await fill(tid(a, 'mimo-noise_frames'), 16);
	await shot(a, 'control');

	// ---- Schedule and Backend -----------------------------------------------------------------
	await section(a, 'schedule');
	const levels = a.locator('[data-testid="level-table"] tbody tr');
	while ((await levels.count()) > 2) await a.locator('[data-testid="level-table"] button[aria-label="Remove level 1"]').click();
	await fill(a.locator('#lv-0-db'), -6);
	await fill(a.locator('#lv-0-dw'), 3);
	await fill(tid(a, 'full-level-s'), 90);
	await shot(a, 'schedule');
	await section(a, 'backend');
	await until(a, 'feasibility per drive', async (p) => (await p.locator('[data-testid^="feas-"]').count()) === 4, 20_000);
	await shot(a, 'backend');

	// ---- Proceed, identify, predict, engage ---------------------------------------------------------
	await section(a, 'reference');
	state = await referenceSettled(a);
	await until(a, 'nothing missing before a run', async (p) => (await attr(p, 'mimo-ready', 'data-ready')) === 'true', 30_000);
	check('the summary lists nothing missing', true, await text(a, 'mimo-ready'));
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	check('Proceed to Run applies the setup: the run is configured', true);
	await tid(a, 'act-start').click();
	await until(a, 'identification', async (p) => ['sysid_noise', 'sysid', 'awaiting_review', 'gate_failed'].includes(await attr(p, 'mimo-run-view', 'data-phase')), 30_000);
	check('Start runs the noise floor and identification', true, await attr(a, 'mimo-run-view', 'data-phase'));
	await until(a, 'the prediction', async (p) => (await attr(p, 'mimo-predict', 'data-state')) === 'review', 240_000);
	const phase = await attr(a, 'mimo-predict', 'data-phase');
	check('identification done: the Predict page comes forward by itself', (await attr(a, 'flow-predict', 'aria-current')) === 'step', phase);
	const driveRows = await a.locator('[data-testid^="predict-drive-"]').count();
	const controlRows = await a.locator('[data-testid^="predict-control-"]').count();
	check('the prediction shows each drive against its ceiling and each control\'s error', driveRows === 4 && controlRows === 4, `${driveRows} drives, ${controlRows} controls`);
	await shot(a, 'predict');
	const before = await text(a, 'predict-law');
	await tid(a, 'predict-again').click();
	await until(a, 'a second prediction', async (p) => (await text(p, 'predict-law')) !== before, 30_000);
	check('Predict Again recomputes with the law shown', true, (await text(a, 'predict-law')).slice(0, 80));
	if (phase === 'gate_failed') {
		await tid(a, 'predict-override').click();
		await a.locator('[data-testid="override-text"]').fill('Simulator plant; reviewed the gates and the prediction.');
		await tid(a, 'override-arm').click();
	} else {
		await tid(a, 'predict-engage-go').click();
	}
	await until(a, 'the ladder', async (p) => ['engaged', 'full_level'].includes(await attr(p, 'mimo-run-view', 'data-phase')), 60_000);
	check(`engaged ${phase === 'gate_failed' ? 'through the override' : 'from the prediction'}: the Run step shows the ladder`, (await attr(a, 'flow-run', 'aria-current')) === 'step');
	await a.waitForTimeout(3000);
	await shot(a, 'run');

	// ---- The run view's pages during the run ------------------------------------------------------
	const PAGES = [
		['mp-1', 'heatmap', '[data-testid="heatmap-canvas"]'],
		['mp-2', 'grid', 'canvas'],
		['mp-3', 'plant', 'canvas, svg'],
		['mp-4', 'geometry', '[data-testid="mimo-geometry-page"]']
	];
	const runPages = async (prefix) => {
		for (const [id, kind, drawn] of PAGES) {
			await tid(a, 'mimo-page-select').selectOption(id);
			await until(a, `the ${kind} page`, async (p) => (await p.locator('[data-testid="mimo-run-view"]').locator(drawn).count()) > 0, 20_000);
			await a.waitForTimeout(2500);
			const err = await tid(a, 'mimo-display-error').isVisible().catch(() => false);
			check(`the run view's ${kind} page draws during the run${prefix ? ' (dark)' : ''}`, !err, err ? await text(a, 'mimo-display-error') : '');
			await shot(a, `${prefix}run_${kind}`);
		}
	};
	await runPages('');
	// A page in a window of its own, for another monitor: the same run, observed.
	const [win] = await Promise.all([ctx.waitForEvent('page'), tid(a, 'mimo-page-popout').click()]);
	await win.setViewportSize({ width: 1280, height: 820 });
	await until(win, 'the display window', async (p) => (await tid(p, 'mimo-run-view').isVisible()) && (await attr(p, 'mimo-run-view', 'data-phase')) !== '', 30_000);
	check('a page opens in a window of its own showing the MIMO run', (await text(win, 'display-window-page')).length > 0, `${await text(win, 'display-window-page')}, ${await attr(win, 'mimo-run-view', 'data-phase')}`);
	await win.waitForTimeout(2000);
	await win.screenshot({ path: path.join(SHOTS, `${PREFIX}_run_window.png`) });
	shots.push({ file: path.join(SHOTS, `${PREFIX}_run_window.png`), kb: Math.round(fs.statSync(path.join(SHOTS, `${PREFIX}_run_window.png`)).size / 1024) });
	await win.close();

	// ---- The dark theme over the MIMO screens and views -------------------------------------------
	const setTheme = async (want) => {
		for (let i = 0; i < 4 && (await a.evaluate(() => document.documentElement.getAttribute('data-theme'))) !== want; i++) await tid(a, 'theme-toggle').click();
		return a.evaluate(() => document.documentElement.getAttribute('data-theme'));
	};
	check('the shell\'s theme switch turns the MIMO screen dark', (await setTheme('dark')) === 'dark');
	await runPages('dark_');
	await tid(a, 'flow-predict').click();
	await until(a, 'the Predict page', async (p) => tid(p, 'mimo-predict').isVisible());
	await shot(a, 'dark_predict');
	for (const id of ['channels', 'reference', 'control', 'schedule', 'backend']) {
		await section(a, id);
		if (id === 'reference') await referenceSettled(a);
		await shot(a, `dark_${id}`);
	}
	await tid(a, 'flow-run').click();
	await until(a, 'the run view', async (p) => tid(p, 'mimo-run-view').isVisible());
	check('the theme switch turns it back', (await setTheme('light')) === 'light');
	await tid(a, 'act-stop').click();
	await until(a, 'the run to end', async (p) => (await attr(p, 'run-state', 'data-state')) === 'finished', 60_000);
	check('Stop ends the run with its ramp', (await attr(a, 'run-state', 'data-outcome')) === 'operator_stop', await attr(a, 'run-state', 'data-outcome'));
} catch (e) {
	failed = e;
	console.error(e);
	try {
		const p = ctx.pages()[0];
		if (p) await p.screenshot({ path: path.join(SHOTS, 'failure.png') });
	} catch {
		/* no page */
	}
} finally {
	await browser.close();
	stopAll();
}

const big = shots.filter((s) => s.kb > 1024);
console.log(`\n${results.filter((r) => r.ok).length} of ${results.length} checks passed; ${shots.length} screenshots, largest ${Math.max(0, ...shots.map((s) => s.kb))} kB${big.length ? `; over 1 MB: ${big.map((s) => s.file).join(', ')}` : ''}`);
if (failed || big.length) process.exit(1);
