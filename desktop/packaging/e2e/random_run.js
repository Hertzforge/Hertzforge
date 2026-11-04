// The local proof of the built desktop app, run inside its window by the `e2e`
// build (desktop/src/e2e.rs): the shell driven through its test ids, as an operator
// drives it, and every step reported to the app's log.
//
//   1. Both services connected, as the window opened.
//   2. Random opens the plate random run and runs a tool on it; Modal opens the
//      quadcopter model and lists its modes (the bundled Python, its test data and
//      its cache folder).
//   3. Acquire & Control: take control, the simulator, a channel table, a short
//      bench profile and schedule, apply, arm, start, proceed past the review hold,
//      and the run completes.
//   4. A display page in a window of its own.
//   5. A second run starts, and while it drives the app is told to quit, the path
//      closing the window takes. The run must end with its controlled stop before
//      the app exits; that is checked from outside, from the control service's
//      own log and an observer connection (docs/desktop-app.md).
//
// Every run here is SIMULATED.
(async () => {
	const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
	const say = (line) => invoke('e2e_log', { line: String(line) }).catch(() => {});
	const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
	const q = (sel, root = document) => root.querySelector(sel);
	const tid = (id, root = document) => q(`[data-testid="${id}"]`, root);
	const visible = (el) => !!el && el.getClientRects().length > 0;
	const text = (id) => (tid(id)?.textContent ?? '').replace(/\s+/g, ' ').trim();
	const stateOf = () => tid('run-state')?.getAttribute('data-state') ?? null;
	const outcomeOf = () => tid('run-state')?.getAttribute('data-outcome') ?? null;
	async function until(what, test, ms = 60000) {
		const t0 = Date.now();
		for (;;) {
			try {
				if (await test()) return;
			} catch {
				/* not yet */
			}
			if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
			await sleep(150);
		}
	}
	function check(name, ok, detail = '') {
		say(`${ok ? 'pass' : 'FAIL'}  ${name}${detail ? `  (${detail})` : ''}`);
		if (!ok) throw new Error(`check failed: ${name}`);
	}
	function setValue(el, value) {
		const proto = el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
		Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, String(value));
		el.dispatchEvent(new Event('input', { bubbles: true }));
		el.dispatchEvent(new Event('change', { bubbles: true }));
	}
	async function fill(selector, value) {
		await until(selector, () => visible(q(selector)), 15000);
		const el = q(selector);
		el.focus();
		setValue(el, value);
		el.blur();
		el.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
		await sleep(60);
	}
	async function click(id, ms = 30000) {
		await until(`${id} shown`, () => visible(tid(id)) && !tid(id).disabled, ms);
		tid(id).click();
		await sleep(80);
	}
	async function verb(id, ms = 30000) {
		await until(`${id} enabled`, () => visible(tid(id)) && tid(id).getAttribute('aria-disabled') === 'false', ms);
		tid(id).click();
		await sleep(80);
	}
	async function tab(id) {
		if (tid('back-to-setup')) await click('back-to-setup');
		await click(`tab-${id}`);
	}

	try {
		say(`shell loaded at ${location.href}`);
		// 1. Both services.
		await until('both services connected', () =>
			tid('conn-analysis')?.getAttribute('data-status') === 'open' && tid('conn-control')?.getAttribute('data-status') === 'open');
		check('both services connected', true, `${new URLSearchParams(location.search).get('analysis')} and ${new URLSearchParams(location.search).get('control')}`);
		const remote = performance.getEntriesByType('resource').map((e) => e.name).filter((u) => !/^(tauri|http:\/\/(tauri|ipc)\.localhost|ipc|ws:\/\/127\.0\.0\.1|data|blob):/.test(u));
		check('nothing loaded from the network', remote.length === 0, remote.join(', '));

		// 2. Random and Modal, on the bundled Python and its test data.
		await click('rail-random');
		await click('dataset-plate-random', 60000);
		await until('the tools of the plate run', () => q('[data-testid^="tool-"]:not([data-testid="tool-list"])'), 120000);
		const tool = q('[data-testid^="tool-"]:not([data-testid="tool-list"])');
		tool.click();
		await verb('run-op').catch(async () => click('run-op'));
		await until('a result', () => document.querySelectorAll('[data-testid="result-card"]').length > 0, 120000);
		check('Random: the plate random run opens and a tool runs on it', true, `${tool.textContent.trim()} -> ${q('[data-testid="result-card"]').getAttribute('data-op')}`);
		await click('rail-modal');
		await click('dataset-drone-fem', 60000);
		await click('tool-modal.modes', 120000);
		await verb('run-op').catch(async () => click('run-op'));
		await until('the modes of the quadcopter', () => document.querySelectorAll('[data-testid="result-card"][data-op="modal.modes"]').length > 0, 120000);
		check('Modal: the quadcopter model solves and shows its modes', true, 'modal.modes result');

		// 3. Acquire & Control: one Random run on the simulator, to completion.
		await click('rail-acquire');
		await until('the test-type chooser', () => visible(tid('test-type-chooser')));
		await click('test-type-random');
		await until('the run bar', () => tid('session-role'));
		await click('claim');
		await until('control', () => tid('session-role')?.getAttribute('data-role') === 'controller');
		await tab('station');
		await click('station-sim');
		if (tid('station-use')) await click('station-use');
		await until('the simulator in use', () => /simulat/i.test(text('station-current')));
		check('station: the simulator', true, text('station-current'));
		await fill('#ch-0-sens', 1000);
		await fill('#ch-0-range', 5);
		await fill('#ch-1-range', 1);
		await until('the table check', () => !tid('table-apply').disabled);
		await click('table-apply');
		await until('the table applied', () => /applied \(revision \d+\)/.test(text('note')));
		await tab('test');
		await fill('[data-testid="setup-title"]', 'Desktop app proof');
		while (document.querySelectorAll('[data-testid^="bp-row-"]').length > 2) {
			const n = document.querySelectorAll('[data-testid^="bp-row-"]').length;
			q(`button[aria-label="Remove row ${n}"]`).click();
			await sleep(80);
		}
		await fill('#bp-0-f', 20);
		await fill('#bp-0-v', '2e-5');
		await click('bp-1-psd');
		await fill('#bp-1-f', 2000);
		await fill('#bp-1-v', '2e-5');
		await fill('#acq-fs', 12800);
		await fill('#acq-npts', '2048');
		await fill('#acq-navg', 8);
		await click('schedule-auto');
		await fill('#lv-0-dw', 4);
		await fill('#lv-1-dw', 4);
		await fill('#lv-full', 10);
		await tab('control');
		await fill('#ctl-drive_ceiling_vrms', 0.3);
		await fill('#ctl-pre-avg', 4);
		await fill('#ctl-pre-max', 0.2);
		await tab('test');
		await until('the service checked the setup', () => /Checked by the control service/.test(text('setup-check')), 20000);
		await verb('act-apply');
		await until('applied', () => tid('setup-status')?.getAttribute('data-status') === 'applied');
		await verb('act-arm');
		await until('armed', () => stateOf() === 'armed');
		check('armed on the simulator, labeled SIM', visible(tid('sim-badge')) && !tid('live-badge'));
		await click('proceed-to-run');
		await until('the Run step', () => visible(tid('run-view')));
		await verb('act-start');
		await until('the review hold', () => stateOf() === 'review', 120000);
		await click('review-proceed');
		await until('running', () => stateOf() === 'running');
		check('the run drives', true, text('run-level'));
		// 4. A display page in a window of its own, while the run goes.
		let opened = null;
		const open = window.open;
		window.open = (...a) => (opened = open.apply(window, a));
		if (tid('page-popout')) {
			tid('page-popout').click();
			const shown = () => !!(opened && opened.document && visible(tid('display-window', opened.document)));
			await until('the display window', shown, 30000).catch(() => {});
			// Reported, not required: the run goes on either way.
			say(`${shown() ? 'pass' : 'FAIL'}  a display page opens in a window of its own  (${opened ? opened.location.href : 'window.open gave nothing'})`);
			if (opened) opened.close();
		}
		window.open = open;
		await until('the run to complete', () => stateOf() === 'finished', 180000);
		check('Run 1 completes on the simulator', outcomeOf() === 'completed', `${outcomeOf()}: ${text('run-end')}`);

		// 5. A second run, and the app told to quit while it drives.
		await verb('act-reset');
		await until('a new run', () => stateOf() === 'configured');
		await verb('act-start');
		await until('the review hold', () => stateOf() === 'review', 120000);
		await click('review-proceed');
		await until('running at a level', () => stateOf() === 'running' && /dB/.test(text('run-level')), 60000);
		await sleep(4000);
		say(`quitting with run 2 ${stateOf()} at ${text('run-level')}`);
		const seen = [];
		const watch = setInterval(() => {
			const s = `${stateOf()} ${outcomeOf() || ''} ${text('run-level')}`.trim();
			if (seen[seen.length - 1] !== s) {
				seen.push(s);
				say(`window shows: ${s}`);
			}
		}, 100);
		await invoke('e2e_quit');
		await sleep(60000);
		clearInterval(watch);
		say('FAIL  the app was still open a minute after quitting');
	} catch (e) {
		say(`FAIL  ${e && e.message ? e.message : e}`);
	}
})();
