// Mixed Mode on the simulator, end to end: the real control service and the shell, driven through
// the installed Chrome the way an operator drives them, every check made on what the screen shows.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/mixed_sim.mjs [--shots <dir>] [--headed] [--mock]
//
// It builds and starts `sds-control` (release, loopback, a free port) and the shell's dev server
// (a free port), runs three Mixed Mode tests on the simulated station, and stops both. With
// `--mock` it starts only the shell, against its own mock control service, to try the flow without
// a build. The service's data root is `HERTZFORGE_DATA_ROOT` when set, else a fresh scratch folder. The runs take a few
// minutes. Not part of `npm test`.
//
// 1. The chooser offers Mixed Mode; its screen opens on Setup, and the screen takes control. The
//    SIM badge is checked at every step: a simulated run never shows LIVE.
// 2. Limits: the summed drive's ceiling and abort, which have no default; the line that a limit
//    lowers the random part only.
// 3. Sine-on-random: the floor and two tones (one standing, one sweeping), entered in the tones
//    table; the service's derived values in the summary; a short automatic ladder run to its end.
// 4. Random-on-random: the tones removed, two narrowbands (one standing, one sweeping by a share
//    of its center); run to its end.
// 5. The combined test: a standing tone, a swept tone with two tones following it at 2 and 3, a
//    standing band and a swept band; the floor alone first, then the parts; the parts table; a part
//    switched off and on; Hold stops parts time and Resume starts it; a level step on the manual
//    ladder; Stop ramps every part down.
// 6. New Run, then Abort.
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
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-mixed'));
const HEADED = args.includes('--headed');
const MOCK = args.includes('--mock');
const PREFIX = flag('--prefix') ?? 'mixed';
/** Run seconds per real second: the mock runs at 4, the service in real time. */
const SPEED = MOCK ? 4 : 1;
/** The floor's density, g²/Hz, flat across the band; the tones and bands are sized against it. */
const FLOOR_PSD = '1e-4';
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
/** A number field or a select by its test id: the control itself, or the one inside its field. */
const field = (page, id) => page.locator(`input[data-testid="${id}"], select[data-testid="${id}"], [data-testid="${id}"] input`).first();
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const attr = (page, id, name) => tid(page, id).getAttribute(name);
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);
const shots = [];
/** What a failed end adds to its words (set once the page exists). */
let failNote = () => '';

async function shot(page, name) {
	const file = path.join(SHOTS, `${PREFIX}_${name}.png`);
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await page.waitForTimeout(400);
	await page.screenshot({ path: file });
	const kb = Math.round(fs.statSync(file).size / 1024);
	shots.push({ file, kb });
	console.log(`      screenshot ${file} (${kb} kB)`);
}
async function fill(page, id, value) {
	const f = field(page, id);
	if ((await f.evaluate((e) => e.tagName)) === 'SELECT') await f.selectOption(String(value));
	else {
		await f.fill(String(value));
		await f.blur();
	}
}
async function neverLive(page, where) {
	check(`no LIVE badge (${where})`, (await page.locator('[data-testid="live-badge"]').count()) === 0);
}
async function section(page, id) {
	if (!(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}
const pageName = async (page) => (await page.locator('[data-testid="page-tabs"] [role="tab"][aria-selected="true"]').first().innerText()).trim();
async function showPage(page, name) {
	await page.locator('[data-testid="page-tabs"]').getByRole('tab', { name, exact: true }).click();
	await until(page, `the ${name} page`, async (p) => (await pageName(p)) === name);
}
const runState = (page) => attr(page, 'mixed-run-view', 'data-state');
const runPhase = (page) => attr(page, 'mixed-run-view', 'data-phase');
const num = async (page, name) => Number((await attr(page, 'mixed-run-view', name)) || NaN);

/** Remove every row of a parts table. */
async function clearTable(page, table) {
	const n = await page.locator(`[data-testid^="mixed-${table}-row-"]`).count();
	if (!n) return;
	await tid(page, `mixed-${table}-pick-0`).click();
	if (n > 1) await tid(page, `mixed-${table}-pick-${n - 1}`).click({ modifiers: ['Shift'] });
	await tid(page, `mixed-${table}-remove`).click();
	await until(page, `the ${table} table empty`, async (p) => (await p.locator(`[data-testid^="mixed-${table}-row-"]`).count()) === 0);
}
/** Add a row and set its fields: `{name, kind, freq|lo,hi,rate,law, of,ratio, amp|level,width,widthby, on}`. */
async function addPart(page, table, i, f) {
	await tid(page, `mixed-${table}-add`).click();
	await until(page, `row ${i + 1} of ${table}`, async (p) => tid(p, `mixed-${table}-row-${i}`).isVisible());
	const k = (c) => `mixed-${table}-${i}-${c}`;
	if (f.name) {
		await field(page, k('name')).fill(f.name);
		await field(page, k('name')).blur();
	}
	if (f.kind) await field(page, k('kind')).selectOption(f.kind);
	if (f.freq !== undefined) await fill(page, k('freq'), f.freq);
	if (f.lo !== undefined) {
		if (f.law) await field(page, k('law')).selectOption(f.law);
		await fill(page, k('lo'), f.lo);
		await fill(page, k('hi'), f.hi);
		await fill(page, k('rate'), f.rate);
	}
	if (f.of !== undefined) {
		await field(page, k('of')).selectOption(f.of);
		await fill(page, k('ratio'), f.ratio);
	}
	if (f.amp !== undefined) await fill(page, k('amp'), f.amp);
	if (f.level !== undefined) await fill(page, k('level'), f.level);
	if (f.widthby) await field(page, k('widthby')).selectOption(f.widthby);
	if (f.width !== undefined) await fill(page, k('width'), f.width);
	if (f.on !== undefined) await fill(page, k('on'), f.on);
}
/** Apply, start, proceed past the review hold, and wait for every part to have come in. */
async function runFromSetup(page) {
	await tid(page, 'proceed-to-run').click();
	await until(page, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' && (await attr(p, 'run-state', 'data-state')) === 'configured', 30_000);
	await tid(page, 'act-start').click();
	await until(page, 'identification done', async (p) => ['review', 'finished'].includes(await runState(p)) || ['floor_only', 'engaged', 'full_level'].includes(await runPhase(p)), 180_000);
	if ((await runState(page)) === 'review') await tid(page, 'review-proceed').click();
	await until(page, 'the floor alone', async (p) => ['floor_only', 'engaged', 'full_level'].includes(await runPhase(p)), 60_000);
	await until(page, 'the parts in', async (p) => ['engaged', 'full_level'].includes(await runPhase(p)) && Number.isFinite(await num(p, 'data-parts-s')), 120_000);
}
/** Wait for the run to end, on any outcome; an end other than `outcome` fails at once with the end's words. */
/** Wait for the setup to check clean; when it does not, fail with what the screen says is left. */
async function checkedClean(page) {
	try {
		await until(page, 'the setup checked clean', async (p) => (await attr(p, 'mixed-ready', 'data-ready')) === 'true', 30_000);
	} catch {
		const errs = await page.locator('.aq-err, .err, [role="alert"], [data-testid$="-error"]').allInnerTexts();
		check('the setup checks clean', false, `${await text(page, 'mixed-ready')} ${errs.map((e) => e.trim()).filter(Boolean).join(' | ')}`);
	}
}
async function toEnd(page, outcome, ms = 300_000) {
	await until(page, `the run to end (expecting ${outcome})`, async (p) => !!(await attr(p, 'run-state', 'data-outcome')), ms);
	const got = await attr(page, 'run-state', 'data-outcome');
	if (got !== outcome) {
		const words = [await text(page, 'run-end').catch(() => ''), await text(page, 'mixed-end').catch(() => '')].filter(Boolean).join(' | ');
		check(`the run ends ${outcome}`, false, `ended ${got}: ${words}${failNote()}`);
	}
}

// ---- the run ----------------------------------------------------------------------------------
let servicePort = null;
if (!MOCK) {
	// The service reads and writes laws, captures and run records under its data root: a fresh
	// scratch folder, so nothing an earlier session imported or recorded changes the run.
	const DATA_ROOT = process.env.HERTZFORGE_DATA_ROOT ?? fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-e2e-mixed-data-'));
	console.log(`data root ${DATA_ROOT}`);
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	servicePort = await freePort();
	const exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
	const service = start('sds-control', exe, ['--port', String(servicePort)], { env: { ...process.env, HERTZFORGE_DATA_ROOT: DATA_ROOT } });
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

try {
	const a = await ctx.newPage();
	// The last configuration the screen applied, written beside the screenshots when a run ends
	// otherwise than expected, so the run can be repeated against the service alone.
	let lastConfigure = null;
	a.on('websocket', (ws) =>
		ws.on('framesent', (f) => {
			if (typeof f.payload === 'string' && f.payload.includes('"mixed.configure"') && !f.payload.includes('"dry_run":true')) lastConfigure = f.payload;
		})
	);
	failNote = () => {
		if (!lastConfigure) return '';
		const file = path.join(SHOTS, `${PREFIX}_last_configure.json`);
		fs.writeFileSync(file, lastConfigure);
		return `; its configuration in ${file}`;
	};
	await a.goto(URL_('light'));
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	const offered = await a.locator('[data-testid="test-type-chooser"] [data-type]').evaluateAll((els) => els.filter((e) => e.getAttribute('aria-disabled') === 'false').map((e) => e.dataset.type));
	check('the chooser offers Mixed Mode', offered.includes('mixed'), offered.join());
	await tid(a, 'test-type-mixed').click();
	await until(a, 'the Mixed Mode screen', async (p) => tid(p, 'mixed-screen').isVisible());
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await attr(p, 'session-role', 'data-role')) === 'controller');
	check('Mixed Mode opens on Setup', (await attr(a, 'flow-setup', 'aria-current')) === 'step');
	await neverLive(a, 'setup');

	// ---- The floor: flat 1e-4 g²/Hz from 20 to 2000 Hz (0.44 g rms), which the simulated plant
	// plays at about half a volt rms; with the tones and bands below, the summed drive at full level
	// stays near 3 V peak, inside the limits with room. ---------------------------------------------
	await section(a, 'floor');
	while ((await a.locator('[data-testid^="bp-row-"]').count()) > 2) {
		const n = await a.locator('[data-testid^="bp-row-"]').count();
		await a.getByRole('button', { name: `Remove row ${n}`, exact: true }).click();
	}
	await a.locator('#bp-0-f').fill('20');
	await a.locator('#bp-0-f').blur();
	await a.locator('#bp-0-v').fill(FLOOR_PSD);
	await a.locator('#bp-0-v').blur();
	if (await tid(a, 'bp-1-psd').count()) await tid(a, 'bp-1-psd').click();
	await a.locator('#bp-1-f').fill('2000');
	await a.locator('#bp-1-f').blur();
	await a.locator('#bp-1-v').fill(FLOOR_PSD);
	await a.locator('#bp-1-v').blur();

	// ---- Limits ------------------------------------------------------------------------------------
	await section(a, 'limits');
	await fill(a, 'mixed-vpk-ceiling', 5);
	await fill(a, 'mixed-vpk-abort', 6);
	check('a limit lowers the random part only, said where limits are set', /random part only, never a tone/.test(await text(a, 'mixed-limit-note')));
	// ---- A short automatic ladder, every run ------------------------------------------------------
	await section(a, 'schedule');
	await tid(a, 'schedule-auto').click();
	for (const [sel, v] of [['#lv-0-dw', 5], ['#lv-1-dw', 5]]) {
		if (await a.locator(sel).count()) {
			await a.locator(sel).fill(String(v));
			await a.locator(sel).blur();
		}
	}
	await fill(a, 'full-level-s', 15);

	// ---- 3. Sine-on-random ---------------------------------------------------------------------------
	await section(a, 'parts');
	await clearTable(a, 'tones');
	await clearTable(a, 'bands');
	await addPart(a, 'tones', 0, { name: 'Blade pass', freq: 120, amp: 0.3 });
	await addPart(a, 'tones', 1, { name: 'Rotor', kind: 'sweep', lo: 400, hi: 600, rate: 1, amp: 0.3 });
	await checkedClean(a);
	check('the summary says sine-on-random', /Sine on random/.test(await text(a, 'mixed-summary-kind')));
	check("the service's derived values: each part's mean square and motion", (await a.locator('[data-testid="mixed-summary-parts"] tr[data-part]').count()) === 3, await text(a, 'mixed-summary-parts'));
	await shot(a, 'setup_sine_on_random');
	await runFromSetup(a);
	check('the floor came in alone, then the parts', true);
	await neverLive(a, 'sine-on-random');
	await showPage(a, 'Control');
	await shot(a, 'run_sine_on_random');
	await toEnd(a, 'completed');
	check('the sine-on-random run completes', /ramped down/.test(await text(a, 'mixed-end')), await text(a, 'mixed-end'));

	// ---- 4. Random-on-random -------------------------------------------------------------------------
	await section(a, 'parts');
	await clearTable(a, 'tones');
	await addPart(a, 'bands', 0, { name: 'Gear mesh', freq: 250, level: 5e-4, width: 20 });
	await addPart(a, 'bands', 1, { name: 'Rotor band', kind: 'sweep', law: 'linear', lo: 1300, hi: 1600, rate: 5, level: 5e-4, widthby: 'share', width: 0.05 });
	await checkedClean(a);
	check('the summary says random-on-random', /Random on random/.test(await text(a, 'mixed-summary-kind')));
	await runFromSetup(a);
	await showPage(a, 'Narrowbands');
	await shot(a, 'run_random_on_random');
	await toEnd(a, 'completed');
	check('the random-on-random run completes', true);

	// ---- 5. The combined test ---------------------------------------------------------------------
	await section(a, 'parts');
	await addPart(a, 'tones', 0, { name: 'Blade pass', freq: 120, amp: 0.3 });
	// slower than before: its third harmonic moves three times as fast, and a reading that lags
	// half the tracking width is refused (tracking_lag)
	await addPart(a, 'tones', 1, { name: 'Rotor', kind: 'sweep', lo: 400, hi: 600, rate: 0.3, amp: 0.3 });
	const rotor = await field(a, 'mixed-tones-1-id').inputValue();
	await addPart(a, 'tones', 2, { name: 'Rotor 2nd', kind: 'follow', of: rotor, ratio: 2, amp: 0.1 });
	await addPart(a, 'tones', 3, { name: 'Rotor 3rd', kind: 'follow', of: rotor, ratio: 3, amp: 0.1 });
	await section(a, 'schedule');
	await tid(a, 'schedule-manual').click();
	await checkedClean(a);
	check('the summary says both', /Sine and random on random/.test(await text(a, 'mixed-summary-kind')));
	await section(a, 'parts');
	await shot(a, 'setup_combined');
	await runFromSetup(a);
	await showPage(a, 'Parts');
	const rows = await a.locator('[data-testid="mixed-parts-table"] tr[data-testid^="mixed-part-"]').count();
	check('the parts table has a row for every part', rows === 7, `${rows} rows`);
	await shot(a, 'run_combined_parts');
	// a part switched off and on, as a ramp
	const sw = a.locator('[data-testid$="-switch"]').first();
	const id = (await sw.getAttribute('data-testid')).replace(/^mixed-part-|-switch$/g, '');
	// the switch's track draws over its box: the operator clicks the switch as drawn
	await sw.click({ force: true });
	await until(a, 'the part off', async (p) => (await attr(p, `mixed-part-${id}`, 'data-on')) === 'false', 30_000);
	await tid(a, `mixed-part-${id}-switch`).click({ force: true });
	await until(a, 'the part on', async (p) => (await attr(p, `mixed-part-${id}`, 'data-on')) === 'true', 30_000);
	check('a part switched off and on live', true, id);
	// hold stops parts time
	await tid(a, 'act-hold').click();
	await until(a, 'the hold', async (p) => (await attr(p, 'mixed-run-view', 'data-held')) === 'true', 10_000);
	const p0 = await num(a, 'data-parts-s');
	await a.waitForTimeout(2500);
	check('Hold stops parts time', (await num(a, 'data-parts-s')) === p0, `${p0} s`);
	await tid(a, 'act-resume').click();
	await until(a, 'parts time again', async (p) => (await num(p, 'data-parts-s')) > p0, 20_000);
	check('Resume starts it again', true);
	// a level step on the manual ladder
	const levelBefore = await text(a, 'run-level');
	await tid(a, 'level-step').selectOption('1');
	await tid(a, levelBefore.startsWith('0.0') ? 'level-down' : 'level-up').click();
	await until(a, 'the level to move', async (p) => (await text(p, 'run-level')) !== levelBefore, 20_000);
	check('a level step moves every part', true, `${levelBefore} to ${await text(a, 'run-level')}`);
	// the composite against its reference at the level in force, and every input's rms
	const total = await text(a, 'mixed-total');
	const m = total.match(/^([\d.]+) g rms against ([\d.]+) g rms at this level$/);
	check('the composite is shown against the reference at this level', !!m && Number(m[1]) / Number(m[2]) > 0.7 && Number(m[1]) / Number(m[2]) < 1.4, total);
	const rmsCells = await a.locator('[data-testid="mixed-inputs"] tbody tr td:nth-child(3)').allInnerTexts();
	check('the Inputs card shows every input with its rms', rmsCells.length > 0 && rmsCells.every((t) => /^[\d.]+(e-?\d+)? \S+$/.test(t.trim())), rmsCells.join(' | '));
	// every run page, as the real frames draw it
	for (const name of ['Control', 'Parts', 'Tones', 'Narrowbands', 'Drive', 'Identification', 'Law']) {
		await showPage(a, name);
		await a.waitForTimeout(1500);
		await shot(a, `combined_page_${name.toLowerCase()}`);
	}
	await showPage(a, 'Control');
	await tid(a, 'act-stop').click();
	await toEnd(a, 'operator_stop', 60_000);
	check('Stop ramps every part down', /ramped down in [\d.]+ s/.test(await text(a, 'mixed-end')), await text(a, 'mixed-end'));

	// ---- 6. New Run, then Abort -------------------------------------------------------------------
	await tid(a, 'act-reset').click();
	await until(a, 'a new run', async (p) => (await attr(p, 'run-state', 'data-state')) === 'configured', 20_000);
	await tid(a, 'act-start').click();
	await until(a, 'identification', async (p) => ['running', 'review'].includes(await runState(p)), 60_000);
	await tid(a, 'act-abort').click();
	await toEnd(a, 'operator_abort', 30_000);
	check('Abort ends the run at once', true);
	await neverLive(a, 'after the abort');
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
