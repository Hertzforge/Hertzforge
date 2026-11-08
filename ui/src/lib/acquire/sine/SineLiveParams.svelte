<script lang="ts">
	/**
	 * The run's law's parameters during the run: those the law declares live can be changed while the
	 * run runs or holds for review, and the change applies from the next tick; every other parameter
	 * is shown as it was set, fixed for the run. The value shown is the one in force.
	 */
	import { formatNumber } from '$lib/viz/format';
	import NumberField from '../NumberField.svelte';
	import type { SineStore } from './sineStore.svelte';
	import { liveValueError } from './sineVerbs';

	interface Props {
		sine: SineStore;
	}
	let { sine }: Props = $props();

	const params = $derived(sine.liveParams);
	const law = $derived(sine.runLaw);
	/** What the operator is typing for each live parameter, before it is sent. */
	let drafts = $state<Record<string, number | null>>({});

	async function send(key: string) {
		const v = drafts[key];
		if (v === null || v === undefined || !Number.isFinite(v)) return;
		const res = await sine.adjust(key, v);
		if (res) delete drafts[key];
	}
</script>

<section class="params" aria-label="Law parameters" data-testid="sine-live-params">
	<h3>{law ? `${law.name} ${law.version}` : 'Control Law'}</h3>
	{#if !params.length}
		<p class="aq-small aq-muted">The law has no number parameters.</p>
	{:else}
		<ul>
			{#each params as p (p.key)}
				{@const draft = drafts[p.key] ?? p.value}
				{@const err = p.live && drafts[p.key] !== undefined ? liveValueError(p, draft) : null}
				<li class:fixed={!p.live} data-testid="sine-live-{p.key}" data-live={p.live} data-enabled={p.enabled}>
					{#if p.live}
						<div class="row">
							<NumberField id="sine-live-{p.key}-v" label={p.label} unit={p.unit} step={p.step} min={p.min} max={p.max} value={draft} onchange={(v) => (drafts[p.key] = v)} errors={err ? [err] : []} disabled={!p.enabled} testid="sine-live-{p.key}-input" help={p.enabled ? `${p.help} From ${p.min} to ${p.max}.` : (p.reason ?? undefined)} />
							<button class="btn sm primary" disabled={!p.enabled || drafts[p.key] === undefined || !!err || sine.busy !== null} onclick={() => send(p.key)} data-testid="sine-live-{p.key}-set">Change</button>
						</div>
					{:else}
						<span class="lbl">{p.label}</span>
						<span class="num val">{p.value === null ? '-' : `${formatNumber(p.value, 4)}${p.unit ? ` ${p.unit}` : ''}`}</span>
						<span class="why aq-small">{p.reason}</span>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</section>

<style>
	.params {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	h3 {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	ul {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.row {
		display: flex;
		align-items: flex-end;
		gap: var(--sp-2);
	}

	.row > :global(:first-child) {
		flex: 1;
		min-width: 0;
	}

	.fixed {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		gap: 0 var(--sp-2);
		color: var(--text-3);
	}

	.lbl {
		font-size: var(--text-xs);
	}

	.val {
		font-size: var(--text-sm);
		text-align: right;
	}

	.why {
		grid-column: 1 / -1;
		color: var(--text-4);
	}
</style>
