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
	import Icon from '$lib/shell/Icon.svelte';
	import SpectrumPane from '$lib/viz/SpectrumPane.svelte';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';

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

	/** A damping typed into a mode's row, in percent; refused words come back from the service. */
	function editDamping(i: number, text: string) {
		const percent = Number(text);
		if (Number.isFinite(percent) && percent !== s!.modes[i].damping * 100) void fit.edit(i, { damping: percent / 100 });
	}

	function editDescription(i: number, text: string) {
		if (text.trim() !== (s!.modes[i].description ?? '')) void fit.edit(i, { description: text });
	}

	const enter = (e: KeyboardEvent) => {
		if (e.key === 'Enter') (e.currentTarget as HTMLInputElement).blur();
	};

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
			{#if s.modes.length}
				<label class="toggle" title="Draw the residual's own CMIF: what is left after the confirmed modes, the curve Find Mode hunts">
					<input type="checkbox" data-testid="fit-residual" checked={s.residual ?? false} onchange={(e) => fit.showResidual(e.currentTarget.checked)} disabled={fit.busy !== null} />
					Residual
				</label>
			{/if}
			<button class="btn ghost sm" data-testid="fit-end" onclick={() => fit.end()}>End Fit</button>
		</div>

		<div class="readout" data-testid="fit-pending">
			<span>Cursor <b class="num">{formatFixed(s.pending.frequency, 4)}</b> Hz</span>
			<span>Damping <b class="num">{formatFixed(s.pending.damping * 100, 3)}</b> %{s.pending.held ? ' (held)' : ''}</span>
			{#if s.preview}<span class="preview">Previewing the mode at {formatFixed(s.preview.frequency, 4)} Hz</span>{/if}
			{#if s.half_power}
				{@const hp = s.half_power}
				<span data-testid="fit-half-power" title={hp.notes.join('; ') || 'Q from the half-power width of the residual CMIF peak under the cursor'}>
					Half-power {#if hp.q !== null && hp.damping !== null}Q <b class="num">{formatFixed(hp.q, 1)}</b>, <b class="num">{formatFixed(hp.damping * 100, 3)}</b> %{:else}not read{/if}
				</span>
			{/if}
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

		{#if s.mac}
			<div class="mac" data-testid="fit-mac"><ViewRenderer view={s.mac} /></div>
		{/if}

		<table class="modes" data-testid="fit-modes" aria-label="Confirmed modes">
			<thead><tr><th>Mode</th><th>Frequency (Hz)</th><th>Damping (%)</th><th>Description</th><th></th></tr></thead>
			<tbody>
				{#each s.modes as md, i (`${i}:${md.frequency}:${md.damping}:${md.description}`)}
					<tr data-testid="fit-mode-{i}">
						<td class="num">{i + 1}</td>
						<td class="num">{formatFixed(md.frequency, 5)}</td>
						<td>
							<input class="cell num" aria-label="Damping of mode {i + 1}, percent" data-testid="fit-mode-damping-{i}" value={formatFixed(md.damping * 100, 3)}
								onchange={(e) => editDamping(i, e.currentTarget.value)} onkeydown={enter} disabled={fit.busy !== null} />
						</td>
						<td>
							<input class="cell" aria-label="Description of mode {i + 1}" data-testid="fit-mode-description-{i}" value={md.description ?? ''}
								onchange={(e) => editDescription(i, e.currentTarget.value)} onkeydown={enter} disabled={fit.busy !== null} />
						</td>
						<td>
							<button class="icon" data-testid="fit-mode-delete-{i}" title="Take mode {i + 1} back" aria-label="Take mode {i + 1} back" onclick={() => fit.remove([i])} disabled={fit.busy !== null}>
								<Icon name="trash" size={12} />
							</button>
						</td>
					</tr>
				{:else}
					<tr><td colspan="5" class="muted">No mode confirmed yet. Find Mode, Fit Mode, then Confirm Mode.</td></tr>
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

	/* a scrolling column: each part keeps its own height rather than shrinking under the next */
	.fit > :global(*) {
		flex-shrink: 0;
	}

	/* the pane fills its box: the rotated mode labels need room under the cells */
	.mac {
		max-width: 460px;
		height: 360px;
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

	/* a row's damping and description are typed in place, as in Visual Dynamics' fit table */
	.cell {
		width: 100%;
		padding: 1px 4px;
		font: inherit;
		color: var(--text-1);
		background: transparent;
		border: 1px solid transparent;
		border-radius: var(--radius-sm, 3px);
	}

	.cell:hover,
	.cell:focus {
		border-color: var(--line-2, var(--line-1));
		background: var(--bg-2);
	}

	.cell.num {
		width: 6em;
		text-align: right;
	}

	.toggle {
		display: flex;
		align-items: center;
		gap: 4px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.icon {
		background: none;
		border: none;
		color: var(--text-3);
		cursor: pointer;
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
