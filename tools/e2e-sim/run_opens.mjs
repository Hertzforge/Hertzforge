// A finished run opened in Analysis, end to end, for any module that writes a run folder: the real
// control service on the simulator, the real analysis service and the shell, driven through the
// installed Chrome the way an operator drives them, with every check made on what the screen shows
// and on the run folder the run left.
//
//   npm --prefix tools/e2e-sim ci
//   node tools/e2e-sim/run_opens.mjs --module <module> [--exe <sds-control>] [--python <python with hertzforge installed>]
//                                    [--data <dir>] [--shots <dir>] [--unwritable] [--headed]
//
// <module> is one of random, sine, shock (Classical Shock), srs, twr, mixed, mimo_random, explorer,
// recorder, mimo_sine. MIMO Sine's run, its table set over the wire and its screen opened by name,
// is mimo_sine_sim.mjs, which this runs with the same arguments. Without `--exe` it builds `sds-control` (release) and runs that. It starts the control
// service (loopback, a free port, its data folder `--data`, else a new temporary folder), the
// analysis service (`python -m hertzforge serve`, a free port) and the shell's dev server, runs one
// short run on the simulator, and stops all three. Not part of `npm --prefix ui test`.
//
// 1. One screen takes control, sets a small channel table where the module needs one and a short
//    setup, and fills the run's record: operator, test article, the specification's document and
//    clause, and notes. Screenshot: the Run Record panel.
// 2. It runs: while it runs, the recording line names the run folder under the service's data
//    folder (screenshot); a review hold is shown before it goes on (screenshot). It runs to its end; Explorer and the Recorder, which run until stopped,
//    are stopped once they have something to keep.
// 3. The end line says where the run was recorded and offers its actions. The run folder holds the
//    operator's words in run.json and is complete.
// 4. Open in Analysis: the analysis service converts the folder and opens it as the project; the
//    shell is in the workspace that suits the run, and the project tree lists what the module's
//    section of docs/run-folder.md says the run saves (screenshot).
//
// With `--unwritable` the data folder is made read-only before the service starts: starting (or,
// for Random, arming) is refused, and the refusal names the path and the reason (screenshot).

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
const MODULE = flag('--module') ?? 'random';
if (MODULE === 'mimo_sine') {
	const { spawnSync } = await import('node:child_process');
	const rest = args.filter((x, i) => x !== '--module' && args[i - 1] !== '--module' && x !== '--unwritable');
	const r = spawnSync(process.execPath, [path.join(path.dirname(fileURLToPath(import.meta.url)), 'mimo_sine_sim.mjs'), ...rest], { stdio: 'inherit' });
	process.exit(r.status ?? 1);
}
const UNWRITABLE = args.includes('--unwritable');
const SHOTS = path.resolve(flag('--shots') ?? path.join(os.tmpdir(), 'hertzforge-e2e-opens', MODULE));
const PYTHON = flag('--python') ?? process.env.HERTZFORGE_PYTHON ?? (process.platform === 'win32' ? 'python' : 'python3');
const HEADED = args.includes('--headed');
fs.mkdirSync(SHOTS, { recursive: true });
const DATA_ROOT = flag('--data') ? path.resolve(flag('--data')) : fs.mkdtempSync(path.join(os.tmpdir(), 'hertzforge-opens-data-'));
fs.mkdirSync(DATA_ROOT, { recursive: true });

const RECORD = { operator: 'Bench operator', article: 'Loopback fixture', document: 'Bench plan', clause: 'Run 1', notes: `${MODULE} on the simulator.` };

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
/** A finding that does not stop the flow: listed in the report, for a person to judge. */
function note(name, ok, detail = '') {
	results.push({ name, ok, detail, soft: true });
	console.log(`${ok ? 'pass' : 'NOTE'}  ${name}${detail ? `  (${detail})` : ''}`);
}

const tid = (page, id) => page.locator(`[data-testid="${id}"]`).first();
const attr = (page, id, name) => tid(page, id).getAttribute(name);
const stateOf = (page) => attr(page, 'run-state', 'data-state');
const outcomeOf = (page) => attr(page, 'run-state', 'data-outcome');
const isRunning = async (page) => (await stateOf(page)) === 'running';
const roleOf = (page) => attr(page, 'session-role', 'data-role');
const text = async (page, id) => ((await tid(page, id).textContent()) ?? '').replace(/\s+/g, ' ').trim();
const until = (page, what, fn, ms) => waitFor(what, () => fn(page), ms);
/** An input or select by its test id, on itself or inside it. */
const field = (page, id) => page.locator(`input[data-testid="${id}"], select[data-testid="${id}"], textarea[data-testid="${id}"], [data-testid="${id}"] input`).first();

async function shot(page, name) {
	const file = path.join(SHOTS, `${name}.png`);
	await page.evaluate(() => document.activeElement instanceof HTMLElement && document.activeElement.blur());
	await page.waitForTimeout(300);
	await page.screenshot({ path: file });
	console.log(`      screenshot ${file}`);
}

async function fill(locator, value) {
	await locator.fill(String(value));
	await locator.blur();
}
const fillId = (page, id, value) => fill(field(page, id), value);

/** A setup section, from the Setup step. */
async function section(page, id) {
	if ((await attr(page, 'flow-setup', 'aria-current')) !== 'step' || !(await tid(page, `tab-${id}`).isVisible())) await tid(page, 'flow-setup').click();
	await tid(page, `tab-${id}`).click();
}

async function ready(page, id) {
	try {
		await until(page, 'the setup checked clean', async (p) => (await attr(p, id, 'data-ready')) === 'true', 45_000);
	} catch {
		check('the setup checks clean', false, await text(page, id).catch(() => ''));
	}
}

/** Remove breakpoint rows until two are left, and set them flat. */
async function flatBreakpoints(page, psd) {
	while ((await page.locator('[data-testid^="bp-row-"]').count()) > 2) {
		const n = await page.locator('[data-testid^="bp-row-"]').count();
		await page.getByRole('button', { name: `Remove row ${n}`, exact: true }).click();
	}
	await fill(page.locator('#bp-0-f'), 20);
	await fill(page.locator('#bp-0-v'), psd);
	if (await tid(page, 'bp-1-psd').count()) await tid(page, 'bp-1-psd').click();
	await fill(page.locator('#bp-1-f'), 2000);
	await fill(page.locator('#bp-1-v'), psd);
}

/** A TWR reference: one second at 12,800 Hz, three decaying tones over a low floor, near 0.2 g peak. */
function writeReference() {
	const file = path.join(path.dirname(DATA_ROOT), `twr_reference_${process.pid}.csv`);
	const fs_hz = 12800;
	const lines = ['time_s,accel_g'];
	for (let i = 0; i < fs_hz; i++) {
		const t = i / fs_hz;
		const v = 0.12 * Math.exp(-3 * t) * Math.sin(2 * Math.PI * 250 * t) + 0.06 * Math.exp(-5 * t) * Math.sin(2 * Math.PI * 620 * t) + 0.03 * Math.sin(2 * Math.PI * 150 * t);
		lines.push(`${t.toFixed(10)},${v.toFixed(7)}`);
	}
	fs.writeFileSync(file, `${lines.join('\n')}\n`);
	return file;
}

// ---- the modules ------------------------------------------------------------------------------
// Each: the chooser's id, the folder name's pattern, the section with the Run Record panel, the
// workspace a finished run opens in, the setup, what ends the run, and the objects the module's
// section of docs/run-folder.md says the run saves, as the project tree names them.
const MODULES = {
	random: {
		type: 'random',
		folder: /^\d{8}-\d{6}_random_/,
		recordSection: 'test',
		workspace: 'random',
		arm: true,
		async setup(a) {
			await section(a, 'station');
			await tid(a, 'station-sim').click();
			if ((await a.locator('[data-testid="station-use"]').count()) > 0) await tid(a, 'station-use').click();
			await until(a, 'the simulator in use', async (p) => /simulat/i.test(await text(p, 'station-current')));
			await fill(a.locator('#ch-0-sens'), 1000);
			await fill(a.locator('#ch-0-range'), 5);
			await fill(a.locator('#ch-1-range'), 1);
			await until(a, 'the table check', async (p) => tid(p, 'table-apply').isEnabled());
			await tid(a, 'table-apply').click();
			await until(a, 'the table applied', async (p) => /applied \(revision \d+\)/.test(await text(p, 'note')));
			await section(a, 'test');
			await fillId(a, 'setup-title', 'Opened in Analysis');
			await flatBreakpoints(a, '2e-5');
			await fill(a.locator('#acq-fs'), 12800);
			await a.locator('#acq-npts').selectOption('2048');
			await fill(a.locator('#acq-navg'), 4);
			await tid(a, 'schedule-auto').click();
			await fill(a.locator('#lv-0-dw'), 2);
			await fill(a.locator('#lv-1-dw'), 2);
			await fill(a.locator('#lv-full'), 4);
			await section(a, 'control');
			const review = tid(a, 'ctl-review');
			if (await review.isChecked()) await review.uncheck({ force: true });
			await fill(a.locator('#ctl-drive_ceiling_vrms'), 0.3);
			await fill(a.locator('#ctl-pre-max'), 0.2);
			await section(a, 'test');
			await until(a, 'the dry run', async (p) => /Checked by the control service/.test(await text(p, 'setup-check')), 15_000);
		},
		objects: ['Channel Table', 'inputs', 'drives', 'control_psd', 'response_psd', 'drive_psd', 'Specification', 'identification_frf', 'identification_coherence', 'Run Record']
	},
	sine: {
		type: 'sine',
		folder: /^\d{8}-\d{6}_sine_/,
		recordSection: 'record',
		workspace: 'sine',
		async setup(a) {
			await section(a, 'limits');
			await fillId(a, 'sine-drive-ceiling', 2);
			await fillId(a, 'sine-drive-abort', 3);
			await section(a, 'profile');
			await tid(a, 'sine-profile-paste').fill('freq_hz, accel_g\n20, 0.5\n2000, 0.5\n');
			await tid(a, 'sine-profile-read-text').click();
			await until(a, 'the pasted rows', async (p) => /Read 2 rows/.test(await text(p, 'sine-profile-imported')), 20_000);
			await section(a, 'test');
			await tid(a, 'sine-kind-sweep').click();
			await fillId(a, 'sine-band-lo', 100);
			await fillId(a, 'sine-band-hi', 400);
			await fillId(a, 'sine-rate', 16);
			await tid(a, 'sine-end-sweeps').click();
			await fillId(a, 'sine-sweeps', 3);
			await section(a, 'schedule');
			await tid(a, 'sine-mode-auto').click();
			await fillId(a, 'sine-per_level-n', 1);
			await fillId(a, 'sine-full_level-n', 1);
			await ready(a, 'sine-ready');
		},
		objects: ['Channel Table', 'inputs', 'drives', 'Specification', 'Sine Levels', 'identification_frf', 'identification_coherence', 'Run Record']
	},
	shock: {
		type: 'shock',
		folder: /^\d{8}-\d{6}_classical_shock_/,
		recordSection: 'trains',
		workspace: 'shock',
		async setup(a) {
			await section(a, 'pulse');
			await fillId(a, 'shock-amplitude', 1);
			await section(a, 'identification');
			await tid(a, 'shock-id-nfft').selectOption('8192');
			await section(a, 'limits');
			await fillId(a, 'shock-drive-ceiling', 2);
			await fillId(a, 'shock-drive-abort', 3);
			await section(a, 'pulse');
			await ready(a, 'shock-ready');
		},
		objects: ['Channel Table', 'inputs identification', 'inputs pulse', 'drives identification', 'drives pulse', 'pulse_srs', 'srs_reference', 'identification_frf', 'identification_coherence', 'Run Record']
	},
	srs: {
		type: 'srs',
		folder: /^\d{8}-\d{6}_srs_/,
		recordSection: 'record',
		workspace: 'shock',
		async setup(a) {
			await section(a, 'limits');
			await fillId(a, 'srs-drive-ceiling', 2);
			await fillId(a, 'srs-drive-abort', 3);
			await section(a, 'target');
			for (const [i, g] of [0.4, 1, 1].entries()) await fill(a.locator(`[data-testid="srs-target-points-${i}-accel_g"]`), g);
			await until(a, 'the target on its grid', async (p) => (await attr(p, 'srs-target-preview', 'data-state')) === 'ok', 30_000);
			await section(a, 'synthesis');
			await until(a, 'the synthesized pulse', async (p) => (await attr(p, 'srs-synthesis-preview', 'data-state')) === 'ok', 30_000);
			await section(a, 'target');
			await ready(a, 'srs-ready');
		},
		objects: ['Channel Table', 'inputs identification', 'inputs pulse', 'drives identification', 'drives pulse', 'pulse_srs', 'srs_reference', 'identification_frf', 'identification_coherence', 'Run Record']
	},
	twr: {
		type: 'twr',
		folder: /^\d{8}-\d{6}_twr_/,
		recordSection: 'record',
		workspace: 'transient',
		async setup(a) {
			const ref = writeReference();
			await section(a, 'reference');
			await tid(a, 'twr-ref-from-path').click();
			await fillId(a, 'twr-ref-path', ref);
			await tid(a, 'twr-ref-read').click();
			await until(a, 'the reference read', async (p) => (await attr(p, 'twr-ref-facts', 'data-sha'))?.length === 64, 30_000);
			await until(a, 'the reference prepared', async (p) => (await attr(p, 'twr-preview', 'data-state')) === 'ok', 30_000);
			await section(a, 'limits');
			await fillId(a, 'twr-drive-ceiling', 1);
			await section(a, 'identification');
			await ready(a, 'twr-ready');
		},
		objects: ['Channel Table', 'inputs identification', 'inputs play', 'drives identification', 'drives play', 'Specification', 'identification_frf', 'identification_coherence', 'Run Record']
	},
	mixed: {
		type: 'mixed',
		folder: /^\d{8}-\d{6}_mixed_/,
		recordSection: 'record',
		workspace: 'sine',
		async setup(a) {
			await section(a, 'floor');
			await flatBreakpoints(a, '1e-4');
			await section(a, 'limits');
			await fillId(a, 'mixed-vpk-ceiling', 5);
			await fillId(a, 'mixed-vpk-abort', 6);
			await section(a, 'schedule');
			await tid(a, 'schedule-auto').click();
			for (const sel of ['#lv-0-dw', '#lv-1-dw']) if (await a.locator(sel).count()) await fill(a.locator(sel), 3);
			await fillId(a, 'full-level-s', 6);
			await section(a, 'parts');
			for (const table of ['tones', 'bands']) {
				const n = await a.locator(`[data-testid^="mixed-${table}-row-"]`).count();
				if (!n) continue;
				await tid(a, `mixed-${table}-pick-0`).click();
				if (n > 1) await tid(a, `mixed-${table}-pick-${n - 1}`).click({ modifiers: ['Shift'] });
				await tid(a, `mixed-${table}-remove`).click();
				await until(a, `the ${table} table empty`, async (p) => (await p.locator(`[data-testid^="mixed-${table}-row-"]`).count()) === 0);
			}
			await tid(a, 'mixed-tones-add').click();
			await until(a, 'a tone row', async (p) => tid(p, 'mixed-tones-row-0').isVisible());
			await fillId(a, 'mixed-tones-0-name', 'Blade pass');
			await fillId(a, 'mixed-tones-0-freq', 120);
			await fillId(a, 'mixed-tones-0-amp', 0.3);
			await ready(a, 'mixed-ready');
		},
		objects: ['Channel Table', 'inputs', 'drives', 'control_psd', 'residual_psd', 'drive_psd', 'Specification', 'Tones', 'identification_frf', 'identification_coherence', 'Run Record']
	},
	mimo_random: {
		type: 'mimo_random',
		folder: /^\d{8}-\d{6}_mimo_random_/,
		recordSection: 'record',
		workspace: 'random',
		running: async (a) => ['engaged', 'full_level'].includes(await attr(a, 'mimo-run-view', 'data-phase')),
		async setup(a) {
			// Four controls and four drives, the simulator's smallest plant: the service's control and
			// drive rows, and three more of each.
			const ids = await a.locator('[data-testid^="mimo-row-"]').evaluateAll((els) => els.map((e) => e.dataset.testid.replace('mimo-row-', '')));
			for (const [k, id] of ids.slice(2, 8).entries()) {
				await a.locator(`[data-testid="mimo-on-${id}"]`).check({ force: true });
				await a.locator(`[data-testid="mimo-role-${id}"]`).selectOption(k < 3 ? 'control' : 'drive');
			}
			const rows = await a.locator('[data-testid^="mimo-row-"][data-on="true"]').evaluateAll((els) => els.map((e) => ({ id: e.dataset.testid.replace('mimo-row-', ''), role: e.dataset.role })));
			let k = 0;
			let d = 0;
			for (const r of rows) {
				await fill(a.locator(`[data-testid="mimo-row-${r.id}"] input[aria-label$=" name"]`), r.role === 'control' ? `Control ${++k}` : `Shaker ${String.fromCharCode(64 + ++d)}`);
				if (r.role === 'drive') {
					await fill(a.locator(`[data-testid="mimo-drive-${r.id}-ceiling"]`), 5);
					await fill(a.locator(`[data-testid="mimo-drive-${r.id}-peak"]`), 10);
				}
			}
			await section(a, 'reference');
			await tid(a, 'ref-tab-breakpoints').click();
			await until(a, 'the reference built', async (p) => (await attr(p, 'mimo-reference-preview', 'data-state')) === 'ok', 30_000);
			await section(a, 'control');
			await until(a, 'the law catalog', async (p) => (await p.locator('[data-testid="law-list"] [role="radio"]').count()) > 0);
			await fillId(a, 'mimo-fs', 4096);
			await a.locator('#mc-npts').selectOption('1024');
			await fillId(a, 'mimo-frames_per_loop', 8);
			await fillId(a, 'mimo-frames', 32);
			await fillId(a, 'mimo-noise_frames', 16);
			await section(a, 'schedule');
			const levels = a.locator('[data-testid="level-table"] tbody tr');
			while ((await levels.count()) > 2) await a.locator('[data-testid="level-table"] button[aria-label="Remove level 1"]').click();
			await fill(a.locator('#lv-0-db'), -6);
			await fill(a.locator('#lv-0-dw'), 3);
			await fillId(a, 'full-level-s', 8);
			await section(a, 'reference');
			await ready(a, 'mimo-ready');
		},
		objects: ['Channel Table', 'inputs', 'drives', 'Run Record']
	},
	explorer: {
		type: 'explorer',
		folder: /^explorer/,
		recordSection: 'capture',
		workspace: 'signals',
		stops: true,
		async setup(a) {
			await section(a, 'test');
			await fillId(a, 'explorer-ceiling', 0.5);
			await fillId(a, 'explorer-band-lo', 20);
			await fillId(a, 'explorer-band-hi', 2000);
			await fillId(a, 'explorer-start', 250);
			await tid(a, 'explorer-nfft-4096').click();
			await ready(a, 'explorer-ready');
		},
		async whileRunning(a) {
			await until(a, 'the tone at 0 V', async (p) => (await attr(p, 'explorer-run-view', 'data-phase')) === 'quiet', 30_000).catch(() => {});
			await fillId(a, 'explorer-volts', 0.2);
			await tid(a, 'explorer-set-volts').click();
			await a.waitForTimeout(4000);
		},
		objects: ['Channel Table', 'inputs', 'drives', 'Run Record']
	},
	recorder: {
		type: 'recorder',
		folder: /^recorder/,
		recordSection: 'capture',
		workspace: 'signals',
		stops: true,
		async setup(a) {
			await section(a, 'trigger');
			await tid(a, 'recorder-mode-start').click();
			await tid(a, 'recorder-length-off').uncheck();
			await fillId(a, 'recorder-length', 2);
			await ready(a, 'recorder-ready');
		},
		async whileRunning(a) {
			await until(a, 'the recording kept', async (p) => Number(await attr(p, 'recorder-run-view', 'data-recordings')) >= 1 && (await attr(p, 'recorder-run-view', 'data-phase')) === 'monitoring', 60_000);
		},
		objects: ['Channel Table', 'rec_0001', 'Run Record']
	}
};

const M = MODULES[MODULE];
if (!M) {
	console.error(`--module is one of ${Object.keys(MODULES).join(', ')}`);
	process.exit(2);
}

// ---- the services -----------------------------------------------------------------------------
let exe = flag('--exe');
if (!exe) {
	console.log('building the control service (release)');
	execFileSync('cargo', ['build', '--release', '-q', '-p', 'sds-control', '--locked'], { cwd: ROOT, stdio: 'inherit' });
	const TARGET = process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(ROOT, 'target');
	exe = path.join(TARGET, 'release', process.platform === 'win32' ? 'sds-control.exe' : 'sds-control');
}
if (UNWRITABLE) fs.chmodSync(DATA_ROOT, 0o555);
const [controlPort, analysisPort, shellPort] = [await freePort(), await freePort(), await freePort()];
const control = start('sds-control', exe, ['--port', String(controlPort)], { env: { ...process.env, HERTZFORGE_DATA_ROOT: DATA_ROOT } });
await waitFor('the control service', () => control.log.join('').includes('listening on ws://127.0.0.1:'), 30_000);
start('analysis', PYTHON, ['-m', 'hertzforge', 'serve', '--port', String(analysisPort)]);
await waitFor('the analysis service', () => new Promise((resolve) => {
	const s = net.connect(analysisPort, '127.0.0.1', () => {
		s.end();
		resolve(true);
	});
	s.on('error', () => resolve(false));
}), 60_000);
start('shell', process.platform === 'win32' ? 'npm.cmd' : 'npm', ['--prefix', 'ui', 'run', 'dev', '--', '--port', String(shellPort), '--strictPort'], { shell: process.platform === 'win32' });
const URL_ = `http://127.0.0.1:${shellPort}/?control=ws://127.0.0.1:${controlPort}/&analysis=ws://127.0.0.1:${analysisPort}/&theme=light#acquire`;
await waitFor('the shell dev server', async () => (await fetch(`http://127.0.0.1:${shellPort}/`)).ok, 60_000);
console.log(`${MODULE}: control ws://127.0.0.1:${controlPort}/ (data ${DATA_ROOT}${UNWRITABLE ? ', read-only' : ''}), analysis ws://127.0.0.1:${analysisPort}/, shell ${URL_}`);

const browser = await chromium.launch({ channel: 'chrome', headless: !HEADED });
let failed = null;
const report = { module: MODULE };
let page = null;
try {
	const a = (page = await (await browser.newContext({ viewport: { width: 1440, height: 1000 } })).newPage());
	await a.goto(URL_);
	await until(a, 'the chooser', async (p) => (await attr(p, 'conn-control', 'data-status')) === 'open' && (await tid(p, 'test-type-chooser').isVisible()));
	await until(a, 'the analysis service', async (p) => (await attr(p, 'conn-analysis', 'data-status')) === 'open');
	await tid(a, `test-type-${M.type}`).click();
	await until(a, 'the shell to connect', async (p) => (await roleOf(p)) === 'observer');
	await tid(a, 'claim').click();
	await until(a, 'control', async (p) => (await roleOf(p)) === 'controller');

	// 1. The setup and the run's record.
	await M.setup(a);
	await section(a, M.recordSection);
	await until(a, 'the Run Record panel', async (p) => tid(p, 'record-panel').isVisible(), 10_000);
	await fillId(a, 'rec-operator', RECORD.operator);
	await fillId(a, 'rec-article', RECORD.article);
	await fillId(a, 'rec-document', RECORD.document);
	await fillId(a, 'rec-clause', RECORD.clause);
	await fillId(a, 'rec-notes', RECORD.notes);
	const hasSwitch = (await a.locator('[data-testid="rec-streams"]').count()) > 0;
	report.streamsSwitch = hasSwitch;
	if (hasSwitch) check('Run Record: time histories are on by default', await tid(a, 'rec-streams').isChecked());
	note('Run Record: the size an hour of time histories takes is said', !hasSwitch || /per hour/.test(await text(a, 'rec-size')), hasSwitch ? await text(a, 'rec-size') : 'no switch: the module always keeps its recordings');
	await tid(a, 'record-panel').scrollIntoViewIfNeeded();
	await shot(a, '01-run-record');

	// 2. Apply, start (Random arms first), past any review hold; the recording line while it runs.
	if (M.arm) {
		await tid(a, 'act-apply').click();
		await until(a, 'applied', async (p) => (await attr(p, 'setup-status', 'data-status')) === 'applied');
	}
	await tid(a, 'proceed-to-run').click();
	await until(a, 'the setup applied', async (p) => (await attr(p, 'flow-run', 'aria-current')) === 'step' || (await stateOf(p)) === 'configured', 30_000);
	const verb = async (id) => {
		const b = tid(a, id);
		await until(a, `${id} offered`, async () => (await b.isVisible()) && (await b.getAttribute('aria-disabled')) !== 'true', 30_000);
		await b.click();
	};
	if (UNWRITABLE) {
		await verb(M.arm ? 'act-arm' : 'act-start');
		await until(a, 'the refusal', async (p) => tid(p, 'refusal').isVisible(), 30_000);
		const words = await text(a, 'refusal');
		report.refusal = words;
		check('the refusal is the record that cannot be written', (await attr(a, 'refusal', 'data-reason')) === 'record_unwritable', await attr(a, 'refusal', 'data-reason'));
		check('the refusal names the data folder', words.includes(DATA_ROOT) || words.includes(fs.realpathSync(DATA_ROOT)), words);
		check('the refusal says why', /permission|denied|read-only|not permitted/i.test(words), words);
		// A module whose refusal comes once the station is being opened (Explorer, the Recorder)
		// ends the run `arm_refused`: finished, nothing armed or running.
		const st = await stateOf(a);
		check('nothing is armed or running', ['configured', 'idle', null].includes(st) || (st === 'finished' && (await outcomeOf(a)) === 'arm_refused'), `${st} ${(await outcomeOf(a)) ?? ''}`);
		await tid(a, 'refusal').scrollIntoViewIfNeeded();
		await shot(a, '05-refused');
		throw null;
	}
	if (M.arm) {
		await verb('act-arm');
		await until(a, 'armed', async (p) => (await stateOf(p)) === 'armed');
	}
	await verb('act-start');
	await until(a, 'the recording line', async (p) => (await tid(p, 'record-line').isVisible()) || !!(await outcomeOf(p)), 120_000);
	const folder = await text(a, 'record-path');
	const name = await text(a, 'record-folder');
	report.folder = folder;
	check('running: the recording line names the run folder under the data folder', path.dirname(folder) === fs.realpathSync(DATA_ROOT) || path.dirname(folder) === DATA_ROOT, folder);
	check('running: the folder is named for the module', M.folder.test(name) && folder.endsWith(name), name);
	check('running: the line says it is recording', /^Recording to/.test(await text(a, 'record-line')), await text(a, 'record-line'));
	report.recordingLine = await text(a, 'record-line');

	// Past a review hold, a prediction, or both, to the end.
	let shotRunning = false;
	let shotHold = false;
	const t0 = Date.now();
	for (;;) {
		if (await outcomeOf(a)) break;
		if (Date.now() - t0 > 600_000) throw new Error('timed out waiting for the run to end');
		if ((await a.locator('[data-testid="review-proceed"]').count()) && (await tid(a, 'review-proceed').isVisible()) && (await tid(a, 'review-proceed').getAttribute('aria-disabled')) !== 'true') {
			if (!shotHold) {
				// The review hold, once: what the identification drew before the run goes on.
				await a.waitForTimeout(1500);
				await shot(a, '02-hold');
				shotHold = true;
			}
			await tid(a, 'review-proceed').click().catch(() => {});
		} else if ((await a.locator('[data-testid="predict-engage-go"]').count()) && (await tid(a, 'predict-engage-go').isVisible())) {
			if ((await attr(a, 'mimo-predict', 'data-phase')) === 'gate_failed') {
				await tid(a, 'predict-override').click();
				await a.locator('[data-testid="override-text"]').fill('Simulator plant; reviewed the gates and the prediction.');
				await tid(a, 'override-arm').click();
			} else await tid(a, 'predict-engage-go').click().catch(() => {});
		} else if (!shotRunning && (await (M.running ?? isRunning)(a)) && (await tid(a, 'record-line').isVisible())) {
			await a.waitForTimeout(1500);
			if (await (M.running ?? isRunning)(a)) {
				await tid(a, 'flow-run').click().catch(() => {});
				await shot(a, '02-recording');
				shotRunning = true;
				if (M.whileRunning) {
					await M.whileRunning(a);
					await tid(a, 'act-stop').click();
				}
			}
		}
		await a.waitForTimeout(500);
	}
	const outcome = await outcomeOf(a);
	report.outcome = outcome;
	check('the run ended as it should', outcome === (M.stops ? 'operator_stop' : 'completed'), `${outcome}: ${await text(a, 'run-end').catch(() => '')}`);
	note('a screenshot was taken while it ran', shotRunning);

	// 3. The end line and its actions; the run folder.
	await until(a, 'the end line', async (p) => /^Recorded in/.test(await text(p, 'record-line')), 30_000);
	check('finished: the line says where the run was recorded', true, await text(a, 'record-line'));
	await until(a, 'the end actions', async (p) => tid(p, 'run-end-actions').isVisible(), 30_000);
	const actions = await a.locator('[data-testid="run-end-actions"] button').evaluateAll((els) => els.map((e) => `${e.dataset.testid}:${e.textContent.trim()}`));
	report.actions = actions;
	check('the end line offers Open in Analysis and Copy Folder Path', actions.some((x) => x.startsWith('act-open-analysis:Open in Analysis')) && actions.some((x) => x.startsWith('act-copy-path:')), actions.join(', '));
	await tid(a, 'run-end-actions').scrollIntoViewIfNeeded();
	await shot(a, '03-end-actions');
	const manifest = JSON.parse(fs.readFileSync(path.join(folder, 'run.json'), 'utf8'));
	check('run.json keeps the operator, the article, the notes and the specification', manifest.operator === RECORD.operator && manifest.article === RECORD.article && manifest.notes === RECORD.notes && manifest.specification?.document === RECORD.document && manifest.specification?.clause === RECORD.clause, JSON.stringify({ operator: manifest.operator, article: manifest.article, notes: manifest.notes, specification: manifest.specification }));
	check('run.json says the run is complete', manifest.complete === true, String(manifest.complete));
	const sub = (d) => (fs.existsSync(path.join(folder, d)) ? fs.readdirSync(path.join(folder, d)).map((f) => `${d}/${f}`) : []);
	report.files = fs.readdirSync(folder).concat(sub('results'), sub('streams'));

	// 4. Open in Analysis.
	await tid(a, 'act-open-analysis').click();
	try {
		await until(a, `the ${M.workspace} workspace`, async (p) => (await attr(p, `rail-${M.workspace}`, 'aria-current')) === 'page', 180_000);
	} catch {
		const where = await a.locator('[data-testid^="rail-"][aria-current="page"]').evaluateAll((els) => els.map((x) => x.dataset.testid)).catch(() => []);
		const status = await text(a, 'open-analysis-status').catch(() => '');
		await shot(a, '04-not-opened');
		check(`Open in Analysis lands in ${M.workspace}`, false, `on ${where.join(',') || 'nothing'}; ${status}`);
	}
	report.workspace = M.workspace;
	await until(a, 'the project tree', async (p) => tid(p, 'project-tree').isVisible(), 30_000);
	await a.waitForTimeout(800);
	const objects = await a.locator('[data-testid="project-tree"] [data-testid^="object-"]').evaluateAll((els) => els.map((e) => ({ name: e.dataset.testid.slice('object-'.length), selected: e.getAttribute('aria-selected') === 'true' })));
	const names = objects.map((o) => o.name);
	report.objects = names;
	await shot(a, '04-in-analysis');
	const missing = M.objects.filter((n) => !names.includes(n));
	check('the project tree lists what the run saves', missing.length === 0, `listed: ${names.join(', ')}${missing.length ? `; missing: ${missing.join(', ')}` : ''}`);
	note("the run's objects are selected", objects.filter((o) => o.selected).length === objects.length, `${objects.filter((o) => o.selected).length} of ${objects.length}`);
	check("the project came from the run folder's converted file", fs.readdirSync(folder).some((f) => f.endsWith('.hforge')), fs.readdirSync(folder).join(', '));

	await tid(a, 'rail-acquire').click();
	await until(a, 'the run again', async (p) => tid(p, 'open-analysis-status').isVisible(), 30_000);
	check('back on the run: it says the run was opened', (await attr(a, 'open-analysis-status', 'data-status')) === 'done' && (await text(a, 'act-open-analysis')) === 'Show in Analysis', await text(a, 'open-analysis-status'));
} catch (err) {
	if (err !== null) {
		failed = err;
		if (page) await shot(page, '99-stopped').catch(() => {});
	}
} finally {
	await browser.close();
	stopAll();
	if (UNWRITABLE) fs.chmodSync(DATA_ROOT, 0o755);
}

const passed = results.filter((r) => r.ok).length;
report.checks = `${passed} of ${results.length}`;
report.notes = results.filter((r) => r.soft && !r.ok).map((r) => `${r.name}: ${r.detail}`);
if (failed) report.stopped = failed.message;
fs.writeFileSync(path.join(SHOTS, UNWRITABLE ? 'report_unwritable.json' : 'report.json'), JSON.stringify(report, null, 2));
console.log(`\n${MODULE}: ${passed} of ${results.length} checks passed${failed ? `; stopped: ${failed.message}` : ''}`);
console.log(`screenshots and report in ${SHOTS}`);
process.exit(failed ? 1 : 0);
