<script lang="ts">
	/**
	 * A table's File menu: Open, Save, Save As, and Export in each form a table file has. Files go
	 * through the analysis service by path: in the desktop app the native dialogs give the path, and
	 * in a browser the path is typed. A breakpoint table is opened in the units the table holds where
	 * the file says none. What happened is handed to `onresult` for the table to show.
	 */
	import { hasNativeFiles } from '$lib/project/files';
	import { TABLE_FORMATS, chooseTableOpen, chooseTableSave, loadTable, pathName, saveTable, withForm, type FileResult, type LoadedTable, type TableData, type TableFileClient, type TableForm, type TableKind } from './tableFiles';

	interface Props {
		/** The analysis service; with none the menu says so and does nothing. */
		client: TableFileClient | null;
		kind: TableKind;
		/** Which breakpoint table, for `kind` breakpoints. */
		tableKind?: string | null;
		/** What the table is called, in titles and as a new file's name. */
		name: string;
		/** The table as it is to be written. */
		table: () => TableData;
		/**
		 * A file read, to take the table's place; what the table changed or left out in taking it, if
		 * anything. Throwing refuses the file, in the error's words, and leaves the table as it was.
		 */
		onopen: (loaded: LoadedTable, path: string) => void | FileResult[];
		/** The file the table was last opened from or saved to. */
		path?: string | null;
		disabled?: boolean;
		/** Why a file cannot be opened now, or null when it can. */
		openBlocked?: string | null;
		/** Asked before a file read takes the table's place; false keeps the table. */
		confirmOpen?: (path: string) => boolean | Promise<boolean>;
		onresult?: (items: FileResult[]) => void;
		/** Which edge of the button the menu lines up with: the end (right) or the start (left). */
		align?: 'start' | 'end';
		testid?: string;
	}
	let { client, kind, tableKind = null, name, table, onopen, path = $bindable(null), disabled = false, openBlocked = null, confirmOpen, onresult, align = 'end', testid = 'table-file' }: Props = $props();

	type Act = { act: 'open' } | { act: 'save' } | { act: 'save-as' } | { act: 'export'; form: TableForm };

	let open = $state(false);
	let busy = $state(false);
	let asking = $state<Act | null>(null);
	let typed = $state('');
	let menuButton = $state<HTMLButtonElement | null>(null);
	let menu = $state<HTMLDivElement | null>(null);
	let root = $state<HTMLDivElement | null>(null);

	const offline = $derived(!client || !client.isOpen);
	const why = $derived(offline ? 'The analysis service, which reads and writes table files, is not connected.' : null);

	const items = $derived<{ id: string; label: string; act: Act; off: string | null }[]>([
		{ id: 'open', label: 'Open…', act: { act: 'open' }, off: openBlocked },
		{ id: 'save', label: 'Save', act: { act: 'save' }, off: null },
		{ id: 'save-as', label: 'Save As…', act: { act: 'save-as' }, off: null },
		...TABLE_FORMATS.map((f) => ({ id: `export-${f.form}`, label: `Export as ${f.label}…`, act: { act: 'export', form: f.form } as Act, off: null }))
	]);

	const PROMPT: Record<Act['act'], string> = { open: 'Table file to open', save: 'Save the table as', 'save-as': 'Save the table as', export: 'Export the table as' };
	const VERB: Record<Act['act'], string> = { open: 'Open', save: 'Save', 'save-as': 'Save', export: 'Export' };

	function report(items: FileResult[]) {
		onresult?.(items);
	}

	function close(focusButton = false) {
		open = false;
		if (focusButton) menuButton?.focus();
	}

	function toggle() {
		open = !open;
		if (open) queueMicrotask(() => menu?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus());
	}

	async function choose(act: Act) {
		close();
		if (act.act === 'save' && path) return write(path, act);
		if (!hasNativeFiles()) {
			asking = act;
			typed = act.act === 'export' && path ? withForm(path, act.form) : (path ?? '');
			return;
		}
		const chosen = act.act === 'open' ? await chooseTableOpen(`Open ${name}`) : await chooseTableSave(act.act === 'export' ? `Export ${name}` : `Save ${name}`, name, act.act === 'export' ? act.form : null);
		if (chosen) await run(act, chosen);
	}

	async function run(act: Act, given: string): Promise<boolean> {
		if (act.act === 'open') return read(given);
		return write(act.act === 'export' ? withForm(given, act.form) : given, act);
	}

	async function write(target: string, act: Act): Promise<boolean> {
		if (!client) return false;
		busy = true;
		try {
			const written = await saveTable(client, kind, tableKind, table(), target);
			if (act.act !== 'export') path = written;
			report([{ level: 'note', message: `${act.act === 'export' ? 'Exported' : 'Saved'} to ${written}.` }]);
			return true;
		} catch (err) {
			report([{ level: 'error', message: `${pathName(target)} was not written. ${(err as Error).message}` }]);
			return false;
		} finally {
			busy = false;
		}
	}

	async function read(target: string): Promise<boolean> {
		if (!client) return false;
		busy = true;
		try {
			const units: Partial<TableData> = kind === 'breakpoints' ? table() : {};
			const loaded = await loadTable(client, kind, tableKind, target, { x_unit: units.x_unit, y_unit: units.y_unit });
			if (confirmOpen && !(await confirmOpen(target))) return false;
			const notes = onopen(loaded, target) ?? [];
			path = target;
			report([{ level: 'note', message: `Opened ${pathName(target)}.` }, ...loaded.warnings.map((message) => ({ level: 'warning' as const, message })), ...notes]);
			return true;
		} catch (err) {
			report([{ level: 'error', message: `${pathName(target)} was not opened. ${(err as Error).message}` }]);
			return false;
		} finally {
			busy = false;
		}
	}

	async function submit(e: SubmitEvent) {
		e.preventDefault();
		const given = typed.trim();
		if (!given || !asking) return;
		if (await run(asking, given)) {
			asking = null;
			typed = '';
		}
	}

	function onMenuKey(e: KeyboardEvent) {
		const buttons = [...(menu?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])];
		const at = buttons.indexOf(document.activeElement as HTMLButtonElement);
		if (e.key === 'Escape') {
			e.preventDefault();
			close(true);
		} else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
			e.preventDefault();
			const next = (at + (e.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
			buttons[next]?.focus();
		} else if (e.key === 'Home' || e.key === 'End') {
			e.preventDefault();
			buttons[e.key === 'Home' ? 0 : buttons.length - 1]?.focus();
		} else if (e.key === 'Tab') {
			close();
		}
	}

	function focused(node: HTMLInputElement) {
		node.focus();
		// the path it opens with is in place once the value is bound: select it, to type over
		setTimeout(() => node.select(), 0);
	}

	$effect(() => {
		if (!open && !asking) return;
		const away = (e: PointerEvent) => {
			if (root && !root.contains(e.target as Node)) {
				open = false;
				asking = null;
			}
		};
		window.addEventListener('pointerdown', away);
		return () => window.removeEventListener('pointerdown', away);
	});
</script>

<div class="tfm" bind:this={root} data-testid={testid}>
	<button
		bind:this={menuButton}
		class="btn sm"
		aria-haspopup="menu"
		aria-expanded={open}
		disabled={disabled || offline || busy}
		title={why ?? (path ? pathName(path) : undefined)}
		data-testid="{testid}-menu"
		onclick={toggle}
	>
		File
		<svg viewBox="0 0 10 10" width="9" height="9" aria-hidden="true"><path d="M2 3.5l3 3 3-3" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" /></svg>
	</button>
	{#if open}
		<div class="menu" class:start={align === 'start'} role="menu" tabindex="-1" aria-label="{name} file" bind:this={menu} onkeydown={onMenuKey}>
			{#if path}<div class="current mono" title={path}>{pathName(path)}</div>{/if}
			{#each items as item, k (item.id)}
				{#if k === 3}<div class="sep" role="separator"></div>{/if}
				<button role="menuitem" class="item" disabled={item.off !== null} title={item.off ?? undefined} data-testid="{testid}-{item.id}" onclick={() => choose(item.act)}>{item.label}</button>
			{/each}
		</div>
	{/if}
	{#if asking}
		<form class="ask" class:start={align === 'start'} onsubmit={submit} data-testid="{testid}-path-form">
			<label for="{testid}-path">{asking.act === 'export' ? `${PROMPT.export} ${TABLE_FORMATS.find((f) => asking?.act === 'export' && f.form === asking.form)?.label}` : PROMPT[asking.act]}</label>
			<div class="row">
				<input
					id="{testid}-path"
					use:focused
					bind:value={typed}
					class="field mono"
					data-testid="{testid}-path"
					placeholder={asking.act === 'open' ? '/path/to/table.hforge' : '/path/to/table.hforge, .csv, .tsv or .xlsx'}
					spellcheck="false"
					autocomplete="off"
					onkeydown={(e) => e.key === 'Escape' && (asking = null)}
				/>
				<button class="btn primary sm" type="submit" disabled={!typed.trim() || busy} data-testid="{testid}-path-go">{VERB[asking.act]}</button>
			</div>
		</form>
	{/if}
</div>

<style>
	.tfm {
		position: relative;
		display: inline-flex;
	}

	.tfm > .btn {
		gap: 4px;
	}

	.menu {
		position: absolute;
		top: calc(100% + 4px);
		right: 0;
		z-index: 30;
		display: flex;
		flex-direction: column;
		min-width: 230px;
		padding: 4px;
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-pop);
	}

	.menu.start,
	.ask.start {
		right: auto;
		left: 0;
	}

	.current {
		padding: 4px 8px 6px;
		max-width: 320px;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		color: var(--text-3);
		font-size: var(--text-2xs);
	}

	.item {
		padding: 5px 8px;
		text-align: left;
		color: var(--text-1);
		font: inherit;
		font-size: var(--text-sm);
		background: none;
		border: 0;
		border-radius: 4px;
		cursor: pointer;
	}

	.item:hover:not(:disabled),
	.item:focus-visible {
		background: var(--bg-4);
		outline: none;
	}

	.item:disabled {
		color: var(--text-4);
		cursor: default;
	}

	.sep {
		height: 1px;
		margin: 4px 2px;
		background: var(--line-2);
	}

	.ask {
		position: absolute;
		top: calc(100% + 4px);
		right: 0;
		z-index: 30;
		width: 420px;
		max-width: 80vw;
		padding: var(--sp-3);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-pop);
	}

	label {
		display: block;
		margin-bottom: var(--sp-1);
		color: var(--text-2);
		font-size: var(--text-xs);
	}

	.row {
		display: flex;
		gap: var(--sp-2);
	}

	.row .field {
		flex: 1;
	}

	.mono {
		font-family: var(--font-mono);
	}
</style>
