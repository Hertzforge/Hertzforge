<script lang="ts">
	/**
	 * The profile before anything is armed: the reference and its alarm and abort bands at full
	 * level on the analysis lines, drawn as the control service drew them (its dry run of the
	 * draft, or the setup it holds). The shell computes none of it.
	 */
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';
	import type { AcquireState } from './store.svelte';

	interface Props {
		acq: AcquireState;
	}
	let { acq }: Props = $props();

	const view = $derived(acq.reference);
	const held = $derived(acq.setupStatus === 'applied');
</script>

<section class="aq-panel preview" aria-labelledby="preview-title" data-testid="reference-preview" data-source={view ? (held ? 'service' : 'dry-run') : 'none'}>
	<div class="aq-head">
		<h2 id="preview-title">Reference</h2>
		<span class="aq-sub">{held ? 'The setup the service holds, at full level, on the lines it judges.' : 'At full level, on the analysis lines the service will judge.'}</span>
	</div>
	{#if view}
		<ViewRenderer {view} showTitle={false} minHeight={220} />
	{:else}
		<p class="aq-muted aq-small empty">
			{#if acq.checking}
				Checking the setup.
			{:else if !acq.connected}
				The profile is drawn by the control service once it is connected.
			{:else}
				The profile is drawn once the setup and the channel table have no errors.
			{/if}
		</p>
	{/if}
</section>

<style>
	.empty {
		display: grid;
		place-items: center;
		min-height: 120px;
		margin: 0;
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
	}
</style>
