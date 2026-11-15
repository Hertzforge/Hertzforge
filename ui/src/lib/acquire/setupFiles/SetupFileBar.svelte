<script lang="ts">
	/**
	 * One bar on every test's setup screen: Save Setup (the draft into the open project), Open
	 * Setup (a saved setup from the project) and Open Setup From Run (the setup a run's record in
	 * the project holds), with where the draft came from; and, on a test an analysis object can
	 * seed, Seed From Project: a specification, a time history or a pulse in the project fills part
	 * of the draft, every member it changes listed first and each protective number it gives typed
	 * by the operator before it is taken. Opening fills the draft only; the screen
	 * checks it as it checks an edit, and nothing is applied or armed by opening it. The browser's
	 * storage keeps the draft as a cache; the project is where a setup is saved.
	 */
	import { untrack } from 'svelte';
	import type { AppState } from '$lib/app/state.svelte';
	import { MODULE_TITLES, type SetupModule } from './setupFile';
	import { SetupFiles } from './setupFiles.svelte';
	import { hostFor } from './setupHosts';
	import { originFor, SetupOriginState } from './setupOrigin.svelte';
	import { typedForm } from './seed';

	interface Props {
		app: AppState;
		module: SetupModule;
	}
	let { app, module }: Props = $props();

	function storage(): Storage | null {
		try {
			return typeof localStorage === 'undefined' ? null : localStorage;
		} catch {
			return null;
		}
	}

	// One bar per screen: the screen's app and module do not change while it is mounted.
	const files = untrack(() => {
		const host = hostFor(app, module);
		const origin = originFor(host.store, () => new SetupOriginState(() => host.key(), app.config.popout ? null : storage(), `hertzforge.acquire.${module}.setup.origin.v1`));
		return new SetupFiles(host, app.analysis, app.project, origin);
	});
	const host = files.host;
	const origin = files.origin;

	let open = $state<'save' | 'setup' | 'run' | 'seed' | null>(null);
	/** The channel (or record) each offered object is read at, by its name. */
	let readAt = $state<Record<string, string>>({});
	/** Random and SRS read a specification at a channel; Classical Shock and TWR a record. */
	const byRecord = $derived(module === 'classical_shock' || module === 'twr');
	/** The differences listed before the rest are counted. */
	const SHOWN = 12;

	async function show(which: 'save' | 'setup' | 'run' | 'seed') {
		files.refused = null;
		files.cancel();
		open = open === which ? null : which;
		if (open === 'save' && !files.saveName) files.saveName = host.title();
		if (open === 'seed') await files.refreshSeeds();
		else if (open) await files.refresh();
	}

	function seedOptions(name: string, channels: string[], tones?: string): { channel?: string; record?: number; tones?: string } {
		const at = readAt[name];
		const beside = tones ? { tones } : {};
		if (!at || channels.length < 2) return beside;
		return { ...beside, ...(byRecord ? { record: Math.max(0, channels.indexOf(at)) } : { channel: at }) };
	}

	const saveWhy = $derived(files.saveBlocked);
	const openWhy = $derived(files.openBlocked);
	const pending = $derived(files.pending);
	const service = $derived(files.serviceWords);
	const seedPending = $derived(files.seedPending);
	const untyped = $derived(files.untyped);
</script>

<section class="aq-panel sfb" aria-label="Setup file" data-testid="setup-file-{module}">
	<div class="line">
		<span class="origin" data-testid="setup-origin" data-kind={origin.origin.kind} data-edited={origin.edited}>{files.originWords}</span>
		<span class="aq-spacer"></span>
		<button class="btn" data-testid="setup-save" aria-expanded={open === 'save'} aria-disabled={!!saveWhy} title={saveWhy ?? 'Save this setup into the open project'} onclick={() => show('save')}>Save Setup</button>
		<button class="btn" data-testid="setup-open" aria-expanded={open === 'setup'} aria-disabled={!!openWhy} title={openWhy ?? 'Open a setup saved in the project'} onclick={() => show('setup')}>Open Setup</button>
		<button class="btn" data-testid="setup-open-run" aria-expanded={open === 'run'} aria-disabled={!!openWhy} title={openWhy ?? "Open the setup a run in the project ran with"} onclick={() => show('run')}>Open Setup From Run</button>
		{#if files.seedable}
			<button class="btn" data-testid="setup-seed" aria-expanded={open === 'seed'} aria-disabled={!!openWhy} title={openWhy ?? 'Fill part of this setup from an object in the project'} onclick={() => show('seed')}>Seed From Project</button>
		{/if}
	</div>

	{#if open === 'seed'}
		<div class="form" data-testid="setup-seed-list">
			{#if openWhy}
				<p class="aq-small" role="status">{openWhy}</p>
			{:else}
				{#if (files.seedOffers ?? []).length === 0}<p class="aq-small">The project holds no object that can fill this setup.</p>{/if}
				<ul>
					{#each files.seedOffers ?? [] as o (o.name)}
						<li>
							<button class="btn link" data-testid="setup-seed-item" disabled={files.busy !== null} onclick={() => files.chooseSeed(o.name, seedOptions(o.name, o.channels, o.tones))}>{o.name}</button>
							<span class="aq-small">{o.holds}; fills {o.fills}{o.tones ? `, with the tones of "${o.tones}"` : ''}</span>
							{#if o.channels.length > 1}
								<label class="aq-small">{byRecord ? 'Record' : 'Channel'}
									<select class="field" data-testid="setup-seed-at" value={readAt[o.name] ?? o.channels[0]} onchange={(e) => (readAt = { ...readAt, [o.name]: (e.currentTarget as HTMLSelectElement).value })}>
										{#each o.channels as ch, i (i)}<option value={ch}>{ch}</option>{/each}
									</select>
								</label>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}

	{#if open === 'save'}
		<div class="form" data-testid="setup-save-form">
			{#if saveWhy}
				<p class="aq-small" role="status">{saveWhy}</p>
			{:else}
				<label>Name <input class="field" data-testid="setup-save-name" bind:value={files.saveName} /></label>
				<label class="notes">Notes <textarea class="field" rows="2" data-testid="setup-save-notes" bind:value={files.saveNotes}></textarea></label>
				{#if files.replacing}
					<p class="aq-small" role="alert">The project already holds a setup named "{files.replacing}".</p>
					<button class="btn primary" data-testid="setup-save-replace" onclick={() => files.save(true)}>Replace It</button>
					<button class="btn" onclick={() => files.cancel()}>Cancel</button>
				{:else}
					<button class="btn primary" data-testid="setup-save-go" disabled={files.busy !== null} onclick={() => files.save()}>Save Into the Project</button>
				{/if}
			{/if}
		</div>
	{/if}

	{#if open === 'setup' || open === 'run'}
		<div class="form" data-testid="setup-open-list">
			{#if openWhy}
				<p class="aq-small" role="status">{openWhy}</p>
			{:else if open === 'setup'}
				{#if files.setups.length === 0}<p class="aq-small">The project holds no saved setup.</p>{/if}
				<ul>
					{#each files.setups as s (s.name)}
						<li>
							<button class="btn link" data-testid="setup-open-item" disabled={files.busy !== null} onclick={() => files.openSaved(s.name)}>{s.name}</button>
							<span class="aq-small">{s.module === module ? s.holds : `for ${MODULE_TITLES[s.module] ?? s.module}`}</span>
						</li>
					{/each}
				</ul>
			{:else}
				{#if files.runs.length === 0}<p class="aq-small">The project holds no run with a setup. Open a run's folder or its project to bring one in.</p>{/if}
				<ul>
					{#each files.runs as r (r.name)}
						<li>
							<button class="btn link" data-testid="setup-open-run-item" disabled={files.busy !== null} onclick={() => files.openRun(r.name)}>Run {r.run_id || r.run_uid}</button>
							<span class="aq-small">{r.title || r.name}, {r.module === module ? (r.outcome || 'did not end cleanly') : `a ${r.module} run`}{r.source ? `, ${r.source}` : ''}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{/if}

	{#if files.refused}
		<p class="msg refused" role="alert" data-testid="setup-refused"><strong>Not opened.</strong> {files.refused}</p>
	{/if}

	{#if pending}
		<div class="msg pending" role="alert" data-testid="setup-pending">
			<p><strong>Check before opening "{pending.name}".</strong> Nothing has changed yet.</p>
			{#if pending.vetting.table.length}
				<p>It was made with a different channel table:</p>
				<ul data-testid="setup-pending-table">
					{#each pending.vetting.table.slice(0, SHOWN) as line, i (i)}<li>{line}</li>{/each}
					{#if pending.vetting.table.length > SHOWN}<li>and {pending.vetting.table.length - SHOWN} more channels</li>{/if}
				</ul>
			{/if}
			{#if pending.vetting.config.length}
				<p>This screen cannot hold it exactly:</p>
				<ul data-testid="setup-pending-config">
					{#each pending.vetting.config.slice(0, SHOWN) as line, i (i)}<li>{line}</li>{/each}
					{#if pending.vetting.config.length > SHOWN}<li>and {pending.vetting.config.length - SHOWN} more</li>{/if}
				</ul>
			{/if}
			<button class="btn primary" data-testid="setup-pending-open" onclick={() => files.confirm()}>Open Into the Draft Anyway</button>
			<button class="btn" data-testid="setup-pending-cancel" onclick={() => files.cancel()}>Cancel</button>
		</div>
	{/if}

	{#if seedPending}
		<div class="msg pending" role="alert" data-testid="setup-seed-pending">
			<p><strong>Check before seeding from "{seedPending.name}".</strong> Nothing has changed yet.</p>
			{#if seedPending.vetting.changes.length}
				<p>The seed changes:</p>
				<ul data-testid="setup-seed-changes">
					{#each seedPending.vetting.changes.slice(0, SHOWN) as line, i (i)}<li>{line}</li>{/each}
					{#if seedPending.vetting.changes.length > SHOWN}<li>and {seedPending.vetting.changes.length - SHOWN} more</li>{/if}
				</ul>
			{:else}
				<p>The draft already holds what it gives.</p>
			{/if}
			{#if seedPending.vetting.lost.length}
				<p>This screen cannot hold it exactly:</p>
				<ul data-testid="setup-seed-lost">{#each seedPending.vetting.lost.slice(0, SHOWN) as line, i (i)}<li>{line}</li>{/each}</ul>
			{/if}
			{#each seedPending.answer.notes as line, i (i)}<p class="aq-small">{line}</p>{/each}
			{#if seedPending.answer.protective.length}
				<p>It gives protective numbers. Type each one as it is given to take it:</p>
				{#each seedPending.answer.protective as n (n.path)}
					<label class="typed">
						<span>{n.words}: <strong>{typedForm(n.value)}</strong></span>
						<input class="field" data-testid="setup-seed-typed" aria-invalid={untyped.includes(n.path)} bind:value={seedPending.typed[n.path]} />
						<span class="aq-small">{untyped.includes(n.path) ? 'Not typed yet' : 'Typed'}</span>
					</label>
				{/each}
			{/if}
			<button class="btn primary" data-testid="setup-seed-take" aria-disabled={untyped.length > 0 || files.busy !== null} title={untyped.length ? 'Type each protective number first' : 'Put the seed into the draft'} onclick={() => files.takeSeed()}>Take Into the Draft</button>
			<button class="btn" data-testid="setup-seed-cancel" onclick={() => files.cancel()}>Cancel</button>
		</div>
	{/if}

	{#if files.note}
		<p class="msg {files.note.tone}" role="status" data-testid="setup-note">{files.note.text}</p>
	{/if}

	{#if service.length}
		<div class="msg refused" role="alert" data-testid="setup-service-words">
			<p><strong>The control service refuses this setup as it stands:</strong></p>
			<ul>{#each service as line, i (i)}<li>{line}</li>{/each}</ul>
		</div>
	{/if}
</section>

<style>
	.sfb {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}
	.line {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}
	.origin {
		font-weight: 600;
	}
	.form {
		display: flex;
		flex-wrap: wrap;
		align-items: flex-end;
		gap: 8px;
	}
	.form ul,
	.msg ul {
		margin: 0;
		padding-left: 18px;
	}
	.form ul {
		list-style: none;
		padding-left: 0;
		width: 100%;
	}
	.form label {
		display: flex;
		flex-direction: column;
		gap: 2px;
	}
	.typed {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 6px;
		margin: 0 0 4px;
	}
	.form .notes {
		flex: 1 1 240px;
	}
	.msg {
		margin: 0;
		padding: 6px 8px;
		border-left: 3px solid var(--border, #888);
	}
	.msg p {
		margin: 0 0 4px;
	}
	.msg.refused,
	.msg.pending,
	.msg.warn {
		border-left-style: double;
		border-left-width: 4px;
	}
</style>
