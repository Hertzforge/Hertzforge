<script lang="ts">
	/**
	 * Fitting modes by hand, as Visual Dynamics' fitting screen does: the bar reads left to right as
	 * the loop does. Find Mode puts the cursor on the most prominent residual peak inside the
	 * frequencies on screen (the zoom is the search band), Fit Mode previews the mode there so its
	 * dashed synthesis can be judged against the measured CMIF, and Confirm Mode adds it. A click on
	 * the plot moves the cursor; a drag zooms.
	 */
	import type { FitState } from '$lib/app/state.svelte';
	import { formatFixed } from '$lib/viz/format';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';

	interface Props {
		fit: FitState;
	}
	let { fit }: Props = $props();

	const s = $derived(fit.state);
	let description = $state('');
	let heldPercent = $state('');

	async function confirm() {
		if (await fit.confirm(description.trim() || undefined)) description = '';
	}

	function hold(e: SubmitEvent) {
		e.preventDefault();
		const percent = Number(heldPercent);
		if (Number.isFinite(percent) && percent > 0) void fit.holdDamping(percent / 100);
	}
</script>

{#if s}
	<section class="fit" aria-label="Fit modes on {s.frf}" data-testid="fit-panel">
		<div class="bar" role="toolbar" aria-label="Fit">
			<button class="btn sm" data-testid="fit-find" onclick={() => fit.find()} disabled={fit.busy !== null}>Find Mode</button>
			<button class="btn sm" data-testid="fit-fit" onclick={() => fit.fit()} disabled={fit.busy !== null}>Fit Mode</button>
			<button class="btn primary sm" data-testid="fit-confirm" onclick={confirm} disabled={fit.busy !== null}>Confirm Mode</button>
			<span class="sep"></span>
			{#if s.modes.length}
				<button class="btn ghost sm" data-testid="fit-refine" onclick={() => fit.refine()} disabled={fit.busy !== null}>Refine</button>
				<button class="btn ghost sm" data-testid="fit-publish" onclick={() => fit.publish()} disabled={fit.busy !== null}>
					{s.shapes ? `Publish to ${s.shapes}` : 'Publish'}
				</button>
			{/if}
			<button class="btn ghost sm" data-testid="fit-end" onclick={() => fit.end()}>End Fit</button>
		</div>

		<div class="readout" data-testid="fit-pending">
			<span>Cursor <b class="num">{formatFixed(s.pending.frequency, 4)}</b> Hz</span>
			<span>Damping <b class="num">{formatFixed(s.pending.damping * 100, 3)}</b> %{s.pending.held ? ' (held)' : ''}</span>
			{#if s.preview}<span class="preview">Previewing the mode at {formatFixed(s.preview.frequency, 4)} Hz</span>{/if}
			<form class="hold" onsubmit={hold}>
				<label for="fit-hold">Hold damping</label>
				<input id="fit-hold" class="field mono" bind:value={heldPercent} placeholder="%" inputmode="decimal" />
			</form>
			<input class="field desc" aria-label="Description of the next mode" bind:value={description} placeholder="Description (optional)" />
		</div>
		{#if fit.busy}<p class="muted" role="status">{fit.busy}.</p>{/if}
		{#if fit.error}<p class="error" role="alert" data-testid="fit-error">{fit.error}</p>{/if}

		<div class="plot">
			<SpectrumPane
				view={s.view}
				minHeight={320}
				marker={s.pending.frequency}
				onPick={(x) => fit.move(x)}
				onRange={(low, high) => (fit.range = [low, high])}
			/>
		</div>

		<table class="modes" data-testid="fit-modes" aria-label="Confirmed modes">
			<thead><tr><th>Mode</th><th>Frequency (Hz)</th><th>Damping (%)</th><th>Description</th></tr></thead>
			<tbody>
				{#each s.modes as md, i (i)}
					<tr>
						<td class="num">{i + 1}</td>
						<td class="num">{formatFixed(md.frequency, 5)}</td>
						<td class="num">{formatFixed(md.damping * 100, 3)}</td>
						<td>{md.description ?? ''}</td>
					</tr>
				{:else}
					<tr><td colspan="4" class="muted">No mode confirmed yet. Find Mode, Fit Mode, then Confirm Mode.</td></tr>
				{/each}
			</tbody>
		</table>
		<p class="hint">Click the plot to move the cursor; drag to zoom, and Find Mode searches what is on screen.</p>
	</section>
{/if}

<style>
	.fit {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-4);
		min-height: 0;
		overflow-y: auto;
		height: 100%;
	}

	.bar,
	.readout {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-2);
	}

	.sep {
		flex: 1;
	}

	.readout {
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.readout b {
		color: var(--text-1);
		font-weight: normal;
	}

	.preview {
		color: var(--text-2);
	}

	.hold {
		display: flex;
		align-items: center;
		gap: 4px;
	}

	.hold input {
		width: 5em;
	}

	.desc {
		flex: 1;
		min-width: 10em;
	}

	.plot {
		min-height: 320px;
	}

	.modes {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--text-xs);
	}

	.modes th {
		text-align: left;
		font-weight: normal;
		color: var(--text-3);
		border-bottom: 1px solid var(--line-1);
	}

	.modes td {
		padding: 2px 6px 2px 0;
	}

	.muted,
	.hint {
		font-size: var(--text-xs);
		color: var(--text-4);
	}

	.error {
		font-size: var(--text-xs);
		color: var(--danger, #d55e00);
	}
</style>
