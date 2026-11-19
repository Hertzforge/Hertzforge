// The local proof that files dropped on the window reach the shell by their paths,
// run inside the window by the `e2e` build (desktop/src/e2e.rs), as random_run.js is.
//
//   1. The shell sees Tauri's API (withGlobalTauri) with the dialogs and the window's
//      own events (capabilities/main.json).
//   2. A drop on the window, sent as the window sends one, reaches the project panel
//      with its path: a file that does not exist is refused by name, which only the
//      analysis service can say, so the path went the whole way.
//
// The drop is sent as the `tauri://drag-drop` event the window emits for a real one;
// the operating system's part, dragging from the Finder, is not driven from here.
(async () => {
	const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args);
	const say = (line) => invoke('e2e_log', { line: String(line) }).catch(() => {});
	const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
	const tid = (id) => document.querySelector(`[data-testid="${id}"]`);
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

	try {
		await until('the analysis service connected', () => tid('conn-analysis')?.getAttribute('data-status') === 'open');
		const api = window.__TAURI__;
		check("the shell sees Tauri's API", !!api?.dialog?.open && !!api?.webview?.getCurrentWebview && !!api?.event?.emitTo);

		const missing = '/no/such/folder/dropped.unv';
		await api.event.emitTo({ kind: 'Webview', label: 'main' }, 'tauri://drag-drop', { paths: [missing], position: { x: 40, y: 40 } });
		await until('the drop refused by name', () => (tid('project-error')?.textContent ?? '').includes('dropped.unv'), 30000);
		check('a dropped path reaches the analysis service', true, tid('project-error').textContent.trim());
	} catch (e) {
		say(`FAIL  ${e && e.message ? e.message : e}`);
	}
	await invoke('e2e_quit');
})();
