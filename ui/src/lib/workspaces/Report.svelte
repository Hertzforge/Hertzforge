<script lang="ts">
	/**
	 * Report: Visual Dynamics' report on the project, generated from a starter template bound to its
	 * objects and edited as VD's report editor edits it. The page is the core's edit page in a
	 * sandboxed frame; a click on a block selects it, and every act (insert, move, delete, a source,
	 * the caption, the text, the title and marking) is on the bar and the pane here, as on VD's. In
	 * the desktop app Save opens the dialog; in a browser it asks for a path.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import { choosePdf, chooseReportPath, hasNativeFiles } from '$lib/project/files';
	import { theme } from '$lib/theme/theme.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import ActionTable from './ActionTable.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const r = $derived(app.report);
	const reports = $derived((app.project.summary?.objects ?? []).filter((o) => o.kind === 'report').map((o) => o.name));

	// the report shown is the selection the action table works on: Report Features, Identity,
	// the findings and chronology, every report operation, offered as the module's acts
	$effect(() => {
		const name = r.name;
		if (!name || !app.project.summary || app.project.selection.objects.includes(name)) return;
		app.project.clickObject(name, app.project.summary.objects.map((o) => o.name));
	});
	// a report operation run on the project gives a reading (as Visual Dynamics' acts do, until
	// Back); it shows here, and the page follows what the act changed
	const a = $derived(app.analyze);
	$effect(() => {
		if (a.reading && r.name) void r.show(r.name);
	});

	const TEMPLATES = [
		['modal', 'Modal test'],
		['random', 'Random vibration'],
		['shock', 'Shock'],
		['transient', 'Transient'],
		['sine', 'Sine sweep'],
		['sysid', 'System identification'],
		['mimo', 'MIMO random'],
		['empty', 'Empty']
	] as const;
	let template = $state('modal');
	let inserting = $state('markdown');
	let frame = $state<HTMLIFrameElement | null>(null);
	const ed = $derived(r.editor);
	const block = $derived(ed?.block ?? null);

	// the page says which block was clicked, and how far it is scrolled
	$effect(() => {
		const heard = (e: MessageEvent) => {
			if (!frame || e.source !== frame.contentWindow) return;
			const said = e.data as { hforgeReport?: { op: string; at: number | null }; hforgeScroll?: number };
			if (typeof said?.hforgeScroll === 'number') r.scroll = said.hforgeScroll;
			if (said?.hforgeReport?.op === 'select') void r.select(said.hforgeReport.at ?? null);
		};
		window.addEventListener('message', heard);
		return () => window.removeEventListener('message', heard);
	});

	// the page re-rendered by an edit opens where it was scrolled
	const page = $derived(
		r.html ? r.html.replace(/<\/body>/i, `<script>addEventListener('load', () => scrollTo(0, ${Math.round(r.scroll)}));<\/script></body>`) : null
	);

	// typing rests before the page re-renders, as VD's editor waits
	let text = $state('');
	let typing: ReturnType<typeof setTimeout> | null = null;
	$effect(() => {
		text = block?.text ?? '';
	});
	function typed() {
		if (typing) clearTimeout(typing);
		const at = block?.index;
		typing = setTimeout(() => {
			if (at !== undefined) void r.operate('text', { at, value: text });
		}, 400);
	}

	let editor = $state<HTMLTextAreaElement | null>(null);
	function reference(caption: string, label: string) {
		if (!editor) return;
		const token = `{{${label.toLowerCase().startsWith('table') ? 'table' : 'figure'}:${caption}}}`;
		const at = editor.selectionStart ?? text.length;
		text = text.slice(0, at) + token + text.slice(editor.selectionEnd ?? at);
		typed();
	}

	function insert() {
		void r.operate('insert', block ? { kind: inserting, at: block.index + 1 } : { kind: inserting });
	}

	function move(step: number) {
		if (!block) return;
		const to = block.index + step;
		if (to < 0 || ed === null || to >= ed.blocks) return;
		// the lifted block lands before `to`, so a step down is two past its own index
		void r.operate('move', { from: block.index, to: step < 0 ? to : to + 1 });
	}
	let asking = $state(false);
	let path = $state('');

	// the page follows the shell's theme, as the rest of the window does
	let shownTheme = theme.resolved;
	$effect(() => {
		const now = theme.resolved;
		if (now !== shownTheme && r.name) void r.show(r.name);
		shownTheme = now;
	});

	// a report removed from the project leaves the stage
	$effect(() => {
		if (r.name && !reports.includes(r.name)) {
			r.name = null;
			r.html = null;
		}
	});

	async function save() {
		if (!r.name) return;
		if (hasNativeFiles()) {
			const chosen = await chooseReportPath(r.name);
			if (chosen) await r.export(chosen);
			return;
		}
		asking = !asking;
	}

	let pdf = $state('');
	async function recordIssue(e?: SubmitEvent) {
		e?.preventDefault();
		let path = pdf.trim();
		if (!path && hasNativeFiles()) path = (await choosePdf()) ?? '';
		if (path) await r.issue('record', path);
	}

		async function submit(e: SubmitEvent) {
		e.preventDefault();
		if (!path.trim()) return;
		await r.export(path.trim());
		if (!r.error) asking = false;
	}
</script>

<div class="report">
	<aside class="side" aria-label="Reports">
		<section class="block">
			<h2 class="eyebrow">Report</h2>
			{#if !app.project.summary}
				<p class="muted">Open a project or import your files in any analysis module; a report is built from the project's objects.</p>
			{:else}
				<label class="lbl" for="report-template">Template</label>
				<select id="report-template" class="field" bind:value={template} data-testid="report-template">
					{#each TEMPLATES as [value, label] (value)}<option {value}>{label}</option>{/each}
				</select>
				<button class="btn primary sm go" data-testid="report-generate" onclick={() => r.generate(template)} disabled={r.busy !== null || !app.analysis.isOpen}>
					Generate Report
				</button>
			{/if}
		</section>
		{#if reports.length}
			<section class="block">
				<h2 class="eyebrow">In the project</h2>
				<div class="choices" role="radiogroup" aria-label="Report" data-testid="report-list">
					{#each reports as name (name)}
						<button class="choice" role="radio" aria-checked={r.name === name} class:on={r.name === name} data-testid="report-{name}" onclick={() => r.show(name)}>
							{name}
						</button>
					{/each}
				</div>
			</section>
		{/if}
		{#if r.name}
			<section class="block">
				<button class="btn ghost sm" data-testid="report-save" onclick={save} disabled={r.busy !== null}>Save as Web Page</button>
				{#if asking}
					<form class="ask" onsubmit={submit} data-testid="report-path-form">
						<label class="lbl" for="report-path">Save the page as</label>
						<input id="report-path" class="field mono" bind:value={path} placeholder="/path/to/report.html" data-testid="report-path" spellcheck="false" />
						<button class="btn primary sm" type="submit" disabled={!path.trim() || r.busy !== null}>Save</button>
					</form>
				{/if}
				{#if r.exported}<p class="muted" data-testid="report-saved">Saved to <span class="mono">{r.exported}</span>.</p>{/if}
				{#if r.held}
					<div class="ask held" data-testid="report-held" role="alertdialog" aria-label="The report is not ready">
						{#if r.held.owed.length}
							<p class="muted">{r.name} still owes:</p>
							<ul class="owed">{#each r.held.owed as line (line)}<li>{line}</li>{/each}</ul>
						{/if}
						{#if r.held.index.length}
							<p class="muted">{r.held.index.length} {r.held.index.length === 1 ? 'file' : 'files'} in the data index of {r.name} {r.held.index.length === 1 ? 'does' : 'do'} not match the checksum indexed:</p>
							<ul class="owed">{#each r.held.index as entry (entry.name)}<li>{entry.name}: {entry.status}</li>{/each}</ul>
						{/if}
						<div class="row">
							<button class="btn sm danger" data-testid="report-anyway" onclick={() => r.anyway()} disabled={r.busy !== null}>{{ export: 'Export anyway', print: 'Print anyway', issue: 'Issue anyway' }[r.held.act]}</button>
							<button class="btn ghost sm" onclick={() => (r.held = null)}>Not yet</button>
						</div>
					</div>
				{/if}
				<button class="btn ghost sm" data-testid="report-print" onclick={() => r.print()} disabled={r.busy !== null}
					title="Print the page as a reader sees it; the print dialog saves it as a PDF">Print or Save as PDF</button>
				<button class="btn ghost sm" data-testid="report-issue" onclick={() => r.issue('begin')} disabled={r.busy !== null || r.issuing !== null}
					title="Print the page as issued, then name the PDF, which issues the report and records its hash">Issue</button>
				{#if r.issuing}
					<form class="ask" onsubmit={recordIssue} data-testid="report-issue-form">
						<p class="muted">Not issued yet. Save the page from the print dialog as a PDF, then name it here to record the issue.</p>
						<label class="lbl" for="report-pdf">The PDF printed</label>
						<input id="report-pdf" class="field mono" bind:value={pdf} placeholder={hasNativeFiles() ? 'Leave empty to choose it' : '/path/to/report.pdf'} data-testid="report-pdf" spellcheck="false" />
						<button class="btn primary sm" type="submit" disabled={r.busy !== null || (!pdf.trim() && !hasNativeFiles())}>Record the Issue</button>
						<button class="btn ghost sm" type="button" onclick={() => r.issue('cancel')} disabled={r.busy !== null}>Cancel</button>
					</form>
				{/if}
				{#if r.issued}<p class="muted" data-testid="report-issued">Issued {r.issued.number} revision {r.issued.revision}, PDF {r.issued.sha256?.slice(0, 12)}.</p>{/if}
			</section>
		{/if}
		{#if r.name && app.project.summary}
			<ActionTable {app} only="report" />
			{#if a.reading}
				<section class="block" data-testid="report-result" aria-label="The reading">
					<div class="phead">
						<h2 class="eyebrow">{a.reading.title}</h2>
						<button class="btn ghost sm" data-testid="report-reading-back" onclick={() => a.endReading()}>Back</button>
					</div>
					{#each a.reading.views as v, i (i)}
						<ViewRenderer view={v} layout="strip" showTitle={v.view !== 'reading'} minHeight={160} />
					{/each}
				</section>
			{/if}
		{/if}
		{#if ed}
			<section class="block pane" data-testid="report-pane" aria-label="Selection">
				{#if !block}
					<h2 class="eyebrow">This report</h2>
					<label class="lbl" for="report-title">Title</label>
					<input id="report-title" class="field" value={ed.title} data-testid="report-title"
						onchange={(e) => r.operate('title', { value: e.currentTarget.value })} />
					<label class="lbl" for="report-marking">Marking</label>
					<input id="report-marking" class="field" value={ed.marking} placeholder="no marking" data-testid="report-marking"
						onchange={(e) => r.operate('marking', { value: e.currentTarget.value })} />
					<label class="lbl" for="report-marking-color">Marking color</label>
					<select id="report-marking-color" class="field" value={ed.marking_color} data-testid="report-marking-color"
						onchange={(e) => r.operate('marking_color', { value: e.currentTarget.value })}>
						<option value="ink">Black/White</option>
						<option value="red">Red</option>
					</select>
					<p class="muted">Click a block in the page to edit it; Insert adds one after the selection, or at the end.</p>
				{:else if block.kind === 'text'}
					<h2 class="eyebrow">Block {block.index + 1}, text</h2>
					{#if block.special}<p class="muted">This is the report's {block.special} block; its own pane is still to come, and its text is edited here.</p>{/if}
					<textarea class="field mono text" bind:this={editor} bind:value={text} oninput={typed} data-testid="report-text"
						placeholder="Markdown: # heading, **bold**, - list"></textarea>
					{#if ed.references?.length}
						<label class="lbl" for="report-reference">Reference</label>
						<select id="report-reference" class="field" data-testid="report-reference"
							onchange={(e) => { const i = Number(e.currentTarget.value); const ref = ed.references?.[i]; if (ref) reference(ref[1], ref[0]); e.currentTarget.value = '-1'; }}>
							<option value="-1">Insert a figure or table number</option>
							{#each ed.references ?? [] as [label, caption], i (i)}<option value={i}>{label}: {caption}</option>{/each}
						</select>
					{/if}
				{:else}
					<h2 class="eyebrow">Block {block.index + 1}, {block.kind}{block.mode ? ` (${block.mode})` : ''}</h2>
					{#each block.fields as f (f.field)}
						<label class="lbl" for="report-field-{f.field}">{f.label}</label>
						<select id="report-field-{f.field}" class="field" value={f.value} data-testid="report-field-{f.field}"
							onchange={(e) => r.operate('field', { at: block.index, field: f.field, value: e.currentTarget.value })}>
							{#each f.options as [value, label] (value)}<option {value}>{label}</option>{/each}
						</select>
					{/each}
					<label class="lbl" for="report-caption">Caption</label>
					<input id="report-caption" class="field" value={block.caption ?? ''} placeholder="caption" data-testid="report-caption"
						onchange={(e) => r.operate('field', { at: block.index, field: 'caption', value: e.currentTarget.value })} />
				{/if}
			</section>
		{/if}
		{#if r.busy}<p class="muted pad" role="status">{r.busy}.</p>{/if}
		{#if r.error}<p class="error pad" role="alert" data-testid="report-error">{r.error}</p>{/if}
	</aside>

	<main class="stage" aria-label="Report page">
		{#if ed}
			<div class="bar" role="toolbar" aria-label="Report blocks" data-testid="report-bar">
				<select class="field" bind:value={inserting} aria-label="What to insert" data-testid="report-insert-kind">
					{#each ed.insert as [kind, label] (kind)}<option value={kind}>{label}</option>{/each}
				</select>
				<button class="btn sm" data-testid="report-insert" onclick={insert} disabled={r.busy !== null}>Insert</button>
				<span class="sep"></span>
				<button class="btn ghost sm" data-testid="report-up" onclick={() => move(-1)} disabled={!block || block.index === 0 || r.busy !== null}>Move Up</button>
				<button class="btn ghost sm" data-testid="report-down" onclick={() => move(1)} disabled={!block || block.index >= ed.blocks - 1 || r.busy !== null}>Move Down</button>
				<button class="btn ghost sm danger" data-testid="report-delete" onclick={() => block && r.operate('remove', { at: block.index })} disabled={!block || r.busy !== null}>Delete</button>
			</div>
		{/if}
		{#if page}
			<!-- the core's own page: its scripts run, isolated from the shell -->
			<iframe bind:this={frame} title={r.name ?? 'Report'} sandbox="allow-scripts" srcdoc={page} data-testid="report-frame"></iframe>
		{/if}
		{#if r.printing}
			<!-- the page as printed, out of sight: it asks for the print dialog once its figures are drawn -->
			{#key r.printing}
				<iframe class="printing" title="Printing" sandbox="allow-scripts allow-modals" srcdoc={r.printing} data-testid="report-print-frame"></iframe>
			{/key}
		{:else}
			<div class="placeholder">Generate a report, or pick one in the project.</div>
		{/if}
	</main>
</div>

<style>
	.phead {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		margin-bottom: var(--sp-2);
	}

	.held {
		margin-top: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--amber-2);
		border-radius: var(--radius-md);
	}

	.owed {
		margin: var(--sp-1) 0 var(--sp-2);
		padding-left: var(--sp-4);
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.held .row {
		display: flex;
		gap: var(--sp-2);
	}

	.report {
		display: grid;
		grid-template-columns: 316px minmax(0, 1fr);
		height: 100%;
		min-height: 0;
	}

	.side {
		display: flex;
		flex-direction: column;
		min-height: 0;
		overflow-y: auto;
		background: var(--bg-1);
		border-right: 1px solid var(--line-1);
	}

	.block {
		padding: var(--sp-4);
		border-bottom: 1px solid var(--line-1);
	}

	.block h2.eyebrow {
		margin-bottom: var(--sp-2);
	}

	.lbl {
		display: block;
		margin: var(--sp-2) 0 2px;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	select,
	input {
		width: 100%;
	}

	.go {
		margin-top: var(--sp-2);
	}

	.choices {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.choice {
		text-align: left;
		padding: 6px 8px;
		border: 1px solid var(--line-1);
		border-radius: var(--radius-sm);
		background: none;
		color: var(--text-2);
		cursor: pointer;
	}

	.choice.on {
		border-color: var(--accent, var(--line-2));
		color: var(--text-1);
	}

	.ask {
		margin-top: var(--sp-2);
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.muted {
		font-size: var(--text-xs);
		color: var(--text-4);
	}

	.error {
		font-size: var(--text-xs);
		color: var(--danger, #d55e00);
	}

	.pad {
		padding: 0 var(--sp-4);
	}

	.stage {
		display: flex;
		flex-direction: column;
		min-width: 0;
		min-height: 0;
	}

	.bar {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--line-1);
	}

	.bar select {
		width: auto;
		max-width: 260px;
	}

	.sep {
		flex: 1;
	}

	.printing {
		position: absolute;
		width: 816px;
		height: 1056px;
		left: -10000px;
		top: 0;
		flex: none;
	}

	.text {
		width: 100%;
		min-height: 220px;
		resize: vertical;
	}

	iframe {
		flex: 1;
		width: 100%;
		min-height: 0;
		border: 0;
		background: var(--bg-0);
	}

	.placeholder {
		display: grid;
		place-items: center;
		height: 100%;
		color: var(--text-4);
		font-size: var(--text-sm);
	}
</style>
