<script lang="ts">
	/**
	 * Fitting modes by hand: Find Mode puts the cursor on the most prominent residual peak inside
	 * the frequencies on screen (the zoom is the search band), and Confirm Mode adds the mode there.
	 * Wherever the cursor lands (Find Mode, a click on the plot, a drag of the cursor) the mode is
	 * fitted there, its dashed synthesis to judge against the measured CMIF: there is no Fit Mode
	 * to press, as the earlier toolset had none (Brandon, 2026-11-14).
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
	/**
	 * The modes picked in the table: a click picks a row, Shift picks to it, Cmd (Ctrl elsewhere)
	 * adds or takes one away, and Delete takes the picked back (Brandon, 2026-11-14).
	 */
	let picked = $state<number[]>([]);
	let anchor: number | null = null;

	/** Find the next peak and fit the mode there. */
	async function findMode() {
		if (await fit.find()) await fit.fit();
	}

	/** Move the cursor where the plot was clicked and fit the mode there. */
	async function moveTo(x: number) {
		if (await fit.move(x)) await fit.fit();
	}

	async function confirm() {
		await fit.confirm();
	}

	function pickRow(e: MouseEvent, i: number) {
		if ((e.target as HTMLElement).closest('input, button')) return;
		if (e.shiftKey && anchor !== null) {
			const [a, b] = anchor < i ? [anchor, i] : [i, anchor];
			picked = Array.from({ length: b - a + 1 }, (_, k) => a + k);
			return;
		}
		if (e.metaKey || e.ctrlKey) picked = picked.includes(i) ? picked.filter((k) => k !== i) : [...picked, i].sort((x, y) => x - y);
		else picked = [i];
		anchor = i;
	}

	async function tableKey(e: KeyboardEvent) {
		if ((e.target as HTMLElement).closest('input')) return;
		if ((e.key === 'Delete' || e.key === 'Backspace') && picked.length && fit.busy === null) {
			e.preventDefault();
			const gone = picked;
			picked = [];
			anchor = null;
			await fit.remove(gone);
		} else if (e.key === 'Escape') picked = [];
	}

	// another set of modes (one confirmed, one taken back) is not the rows picked
	$effect(() => {
		void s?.modes.length;
		picked = [];
		anchor = null;
	});

	/** Where the half-power reading crosses, drawn beside the cursor. */
	const crossings = $derived([s?.half_power?.low, s?.half_power?.high].filter((x): x is number => x !== null && x !== undefined));

	/** The plot's whole height is the damping: 0.01 % at the top, 10 % at the bottom, log-linear
	 * between. */
	const DAMPING_TOP = 1e-4;
	const DAMPING_BOTTOM = 0.1;
	let dragged = $state<number | null>(null);

	/** How long the cursor rests before the mode is fitted where it is, as the earlier toolset waited. */
	const SETTLE_MS = 220;
	/** The newest place the drag has reached and not yet sent; one request is out at a time. */
	let queued: { x?: number; damping?: number } | null = null;
	let sending = false;
	let settle: ReturnType<typeof setTimeout> | undefined;

	async function send() {
		if (sending || !queued) return;
		sending = true;
		const next = queued;
		queued = null;
		if (next.x !== undefined) await fit.move(next.x, true);
		if (next.damping !== undefined) await fit.holdDamping(next.damping, true);
		sending = false;
		void send();
	}

	/** Fit the mode where the cursor came to rest, once what the drag sent has landed. */
	async function fitWhereResting() {
		while (sending || queued) await new Promise((r) => setTimeout(r, 30));
		await fit.fit(true);
	}

	/**
	 * The cursor dragged: across moves it, up and down holds a damping, sent as it goes so the
	 * pending mode follows the hand, and the mode is fitted when the cursor rests (Brandon,
	 * 2026-11-14: the earlier toolset's cursor; the fit waited for the release before).
	 */
	function dragCursor(x: number, fraction: number, moved: { x: boolean; y: boolean }, done: boolean) {
		const damping = DAMPING_TOP * (DAMPING_BOTTOM / DAMPING_TOP) ** fraction;
		if (moved.y) dragged = damping;
		if (moved.x || moved.y) {
			queued = { ...queued, ...(moved.x ? { x } : {}), ...(moved.y ? { damping } : {}) };
			void send();
			clearTimeout(settle);
			settle = setTimeout(() => void fitWhereResting(), done ? 0 : SETTLE_MS);
		}
		if (done) dragged = null;
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

	/** Where the cursor waits to be moved back into view, once the zoom or pan settles. */
	let inView: ReturnType<typeof setTimeout> | undefined;

	/**
	 * The frequencies on screen are Find Mode's search band, and the cursor stays among them: a
	 * zoom or pan that would leave it outside carries it to just inside the near edge, as the
	 * earlier toolset's did, rather than suggesting a new place (Brandon, 2026-11-14).
	 */
	function keepCursorInView(low: number, high: number) {
		fit.range = [low, high];
		const at = s?.pending.frequency;
		clearTimeout(inView);
		if (at === undefined || at === null || !(high > low)) return;
		const inset = (high - low) * 0.01;
		const edge = at < low ? low + inset : at > high ? high - inset : null;
		if (edge !== null) inView = setTimeout(() => void fit.move(edge), 120);
	}
</script>

{#if s}
	<section class="fit" aria-label="Fit modes on {s.frf}" data-testid="fit-panel">
		<div class="bar" role="toolbar" aria-label="Fit">
			<button class="btn sm" data-testid="fit-find" onclick={findMode} disabled={fit.busy !== null}>Find Mode</button>
			<button class="btn primary sm" data-testid="fit-confirm" onclick={confirm} disabled={fit.busy !== null}>Confirm Mode</button>
			<span class="sep"></span>
			{#if s.modes.length}
				<button class="btn ghost sm" data-testid="fit-refine" onclick={() => fit.refine()} disabled={fit.busy !== null}>Refine</button>
			{/if}
			{#if s.modes.length}
				<label class="toggle" title="Draw the residual's own CMIF: what is left after the confirmed modes, the curve Find Mode hunts">
					<input type="checkbox" data-testid="fit-residual" checked={s.residual ?? false} onchange={(e) => fit.showResidual(e.currentTarget.checked)} disabled={fit.busy !== null} />
					Residual
				</label>
			{/if}
			{#if s.shapes}
				<!-- kept in the project as modes are confirmed: nothing to publish, and leaving loses nothing -->
				<span class="saved" data-testid="fit-saved" title="Saved in the project as each mode is confirmed, deleted or edited; select anything in the tree to leave the fit">Saved to {s.shapes}</span>
			{/if}
		</div>

		<div class="readout" data-testid="fit-pending">
			<span>Cursor <b class="num">{formatFixed(s.pending.frequency, 4)}</b> Hz</span>
			<span>Damping <b class="num" data-testid="fit-damping">{formatFixed((dragged ?? s.pending.damping) * 100, 3)}</b> %{s.pending.held || dragged !== null ? ' (held)' : ''}</span>
			{#if s.preview}<span class="preview">Previewing the mode at {formatFixed(s.preview.frequency, 4)} Hz</span>{/if}
			{#if s.coherence}<span data-testid="fit-weighted" title="Each channel at each line counts by its coherence, γ²/(1 − γ²): a noisy line pulls the fit less">Weighted by {s.coherence}</span>{/if}
			{#if s.half_power}
				{@const hp = s.half_power}
				<span data-testid="fit-half-power" title={hp.notes.join('; ') || 'Q from the half-power width of the residual CMIF peak under the cursor'}>
					Half-power {#if hp.q !== null && hp.damping !== null}Q <b class="num">{formatFixed(hp.q, 1)}</b>, <b class="num">{formatFixed(hp.damping * 100, 3)}</b> %{:else}not read{/if}
				</span>
			{/if}
		</div>
		<!-- one line that is always there, so what it says never moves the plot -->
		<div class="status">
			{#if fit.error}<p class="error" role="alert" data-testid="fit-error">{fit.error}</p>
			{:else if fit.busy}<p class="muted" role="status">{fit.busy}.</p>{/if}
		</div>

		<div class="plot">
			<SpectrumPane
				view={s.view}
				minHeight={320}
				marker={s.pending.frequency}
				guides={crossings}
				onMarkerDrag={dragCursor}
				onPick={moveTo}
				onRange={keepCursorInView}
				rangeOn={(series) => series.role === 'measured' && series.follows === undefined}
			/>
		</div>

		<!-- the MAC and the modes each have their place under the plot from the start, so the plot keeps its size as modes are confirmed -->
		<div class="under">
		<div class="mac" data-testid="fit-mac">
			{#if s.mac}<ViewRenderer view={s.mac} />{:else}<p class="muted empty">The MAC of the confirmed modes shows here once there are two.</p>{/if}
		</div>

		<div class="list">
		<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions (the table takes Delete for its picked rows) -->
		<table class="modes" data-testid="fit-modes" aria-label="Confirmed modes" tabindex="0" onkeydown={tableKey}>
			<thead><tr><th>Mode</th><th>Frequency (Hz)</th><th>Damping (%)</th><th>Description</th><th></th></tr></thead>
			<tbody>
				{#each s.modes as md, i (`${i}:${md.frequency}:${md.damping}:${md.description}`)}
					<tr data-testid="fit-mode-{i}" class:on={picked.includes(i)} aria-selected={picked.includes(i)} onclick={(e) => pickRow(e, i)}>
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
					<tr><td colspan="5" class="muted">No mode confirmed yet. Find Mode, then Confirm Mode.</td></tr>
				{/each}
			</tbody>
		</table>
		</div>
		</div>
		<p class="hint">Click the plot to move the cursor. Drag to pan; right-drag or the wheel zooms, over an axis that axis alone; double-click or A views all. Find Mode searches what is on screen.</p>
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

	.saved {
		margin-left: auto;
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.preview {
		color: var(--text-2);
	}

	/* a scrolling column: each part keeps its own height rather than shrinking under the next */
	.fit > :global(*) {
		flex-shrink: 0;
	}

	/* the pane fills its box: the rotated mode labels need room under the cells */
	.status {
		flex: none;
		height: 1.5em;
		overflow: hidden;
	}

	.status p {
		margin: 0;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	/* the plot has the top three fifths of what the bar and the readout leave, and what is under it the rest: neither changes size as modes are confirmed. Two thirds left the MAC a strip too low to read */
	.plot {
		display: flex;
		flex: 3 1 0;
		flex-direction: column;
		min-height: 300px;
	}

	.plot > :global(*) {
		flex: 1 1 0;
		min-height: 0;
	}

	.under {
		display: grid;
		flex: 2 1 0;
		grid-template-columns: minmax(240px, 1fr) minmax(0, 2fr);
		gap: var(--sp-4);
		min-height: 260px;
	}

	.mac,
	.list {
		min-width: 0;
		min-height: 0;
	}

	.mac {
		display: flex;
		flex-direction: column;
	}

	.mac > :global(*) {
		flex: 1 1 0;
		min-height: 0;
	}

	.empty {
		padding: var(--sp-2);
	}

	/* the modes scroll in their place */
	.list {
		overflow-y: auto;
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

	.modes:focus {
		outline: none;
	}

	.modes tbody tr {
		cursor: default;
	}

	.modes tbody tr.on {
		background: var(--select-bg, var(--bg-3));
	}

	.modes:focus-within tbody tr.on {
		box-shadow: inset 2px 0 0 var(--accent);
	}

	/* a row's damping and description are typed in place */
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
