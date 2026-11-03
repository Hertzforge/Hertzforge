<script lang="ts">
	/**
	 * Report: Visual Dynamics' report on the project, generated from a starter template bound to its
	 * objects and shown as the page the core writes, in a sandboxed frame, exactly as a reader of the
	 * saved file sees it. In the desktop app Save opens the dialog; in a browser it asks for a path.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import { chooseReportPath, hasNativeFiles } from '$lib/project/files';
	import { theme } from '$lib/theme/theme.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const r = $derived(app.report);
	const reports = $derived((app.project.summary?.objects ?? []).filter((o) => o.kind === 'report').map((o) => o.name));

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
				<p class="muted">Open a project or import your files in Analyze; a report is built from the project's objects.</p>
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
			</section>
		{/if}
		{#if r.busy}<p class="muted pad" role="status">{r.busy}.</p>{/if}
		{#if r.error}<p class="error pad" role="alert" data-testid="report-error">{r.error}</p>{/if}
	</aside>

	<main class="stage" aria-label="Report page">
		{#if r.html}
			<!-- the core's own page: its scripts run, isolated from the shell -->
			<iframe title={r.name ?? 'Report'} sandbox="allow-scripts" srcdoc={r.html} data-testid="report-frame"></iframe>
		{:else}
			<div class="placeholder">Generate a report, or pick one in the project.</div>
		{/if}
	</main>
</div>

<style>
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
		min-width: 0;
		min-height: 0;
	}

	iframe {
		width: 100%;
		height: 100%;
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
