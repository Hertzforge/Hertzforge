<script lang="ts">
	/**
	 * The MIMO run as it happens, on the test screen's frame: the page's plots on their grid with the
	 * page tabs under them, and the control dock with the run's cards. The Run card holds the verbs
	 * that apply now, the review hold after identification (with the way to the prediction), the
	 * run's end, and any fault. Level, Drives, Prediction and Station follow. The matrix grid, the
	 * error heatmap, the achievable reference and the geometry draw their own bodies; every other
	 * plot is lines. The window on another monitor shows the same, compact.
	 */
	import type { Snippet } from 'svelte';
	import type { LawEntry } from '$lib/protocol/mimo';
	import type { MimoGeometry } from '$lib/viz/geometry';
	import type { RowBand } from '$lib/viz/scales';
	import type { PlotPanelSpec } from '../../plots/types';
	import PulseRunView from '../../pulse/PulseRunView.svelte';
	import RunLine from '../../pulse/RunLine.svelte';
	import type { BodyPanel, PlotExtras } from '../../screen/bodies';
	import type { PlotPanel } from '../../screen/screen.svelte';
	import type { CardKind } from '../../screen/types';
	import type { MimoState } from '../mimoStore.svelte';
	import MimoChoices from './MimoChoices.svelte';
	import MimoDrivesCard from './MimoDrivesCard.svelte';
	import MimoGeometryBody from './MimoGeometryBody.svelte';
	import MimoGridBody from './MimoGridBody.svelte';
	import MimoHeatmapBody from './MimoHeatmapBody.svelte';
	import MimoLevelCard from './MimoLevelCard.svelte';
	import MimoPredictionCard from './MimoPredictionCard.svelte';
	import { coherenceModeOffered, diagChoice } from './mimoScreen';
	import type { MimoScreenState } from './mimoScreenState.svelte';

	interface Props {
		mimo: MimoState;
		screen: MimoScreenState;
		/** The station panel. */
		station?: Snippet;
		onpopout?: (pageId: string) => boolean;
		geometry?: MimoGeometry | null;
		bands?: RowBand[];
		ceilings?: number[] | null;
		diagnostics?: LawEntry['diagnostics'];
		abortKeys: string;
		compact?: boolean;
	}
	let { mimo, screen, station, onpopout, geometry = null, bands = [], ceilings = null, diagnostics = [], abortKeys, compact = false }: Props = $props();

	/** The run's state as the MIMO store holds it; the screen's own when it has none (a window that only watches). */
	const status = $derived(mimo.status ?? screen.status);
	const runState = $derived(status?.state ?? null);
	const streaming = $derived(runState === 'running' || runState === 'review' || runState === 'stopping' || (runState === 'finished' && status?.elapsed_s !== null));
	const virtual = $derived(status?.shape?.space === 'virtual');

	const REVIEW: Record<string, string> = {
		awaiting_review: 'Identification passed. Nothing drives at test level until you proceed; the prediction shows what the law would ask of each drive.',
		gate_failed: 'A gate failed at identification. The run engages only through an override, from the prediction.',
		prediction_exceeded: 'The prediction asks a drive for more than its ceiling. The run engages only through an override, from the prediction.'
	};
	const review = $derived(runState === 'review' ? (REVIEW[status?.phase ?? ''] ?? REVIEW.awaiting_review) : null);

	const IDENTIFIED = new Set(['frf', 'sv', 'cond', 'mcoh']);

	function empty(p: PlotPanel): string {
		const q = p.plot.quantity;
		if (q === 'law_diag' && !diagChoice(p.plot.select).key) return diagnostics.length ? "Choose a diagnostic in the plot's options." : 'The control law declares no diagnostics.';
		if (q === 'dof_rms' && status?.shape && !virtual) return 'Shown under virtual control.';
		if (q === 'drive_coh' && status?.shape && status.shape.drive_labels.length < 2) return 'Needs two drives.';
		if (!streaming) return 'Appears when a run starts.';
		if (q.startsWith('pred_')) return 'Appears with the prediction, after identification.';
		if (IDENTIFIED.has(q)) return 'Appears after identification.';
		if (!screen.frame(p.id)) return 'Waiting for the control service.';
		return 'Appears with the first control loop.';
	}

	// Each line plot's width, so the service decimates to what is drawn. The plots that draw their
	// own bodies measure their own parts.
	let host = $state<HTMLDivElement | null>(null);
	const shape = $derived(screen.page.panels.map((p) => `${p.id}:${p.at.x},${p.at.y},${p.at.w},${p.at.h}:${p.kind === 'plot' ? p.plot.quantity : ''}`).join('|'));
	$effect(() => {
		void shape;
		void screen.maximized;
		const el = host;
		if (!el || typeof ResizeObserver === 'undefined') return;
		const ro = new ResizeObserver((entries) => {
			for (const e of entries) {
				const t = e.target as HTMLElement;
				if (t.dataset.plot) screen.setWidth(t.dataset.plot, 'main', Math.max(64, t.clientWidth - 80));
			}
		});
		for (const s of el.querySelectorAll<HTMLElement>('[data-plot]')) {
			if (!screen.typeOf(s.dataset.quantity ?? '')?.own) ro.observe(s);
		}
		return () => ro.disconnect();
	});
</script>

{#snippet notes()}
	{#if runState === 'review' && !compact}
		<button class="btn sm open" data-testid="mimo-open-predict" onclick={() => (mimo.step = 'predict')}>Open the Prediction</button>
	{/if}
	{#if status?.faults.length}
		<div class="faults" data-testid="mimo-faults">
			{#each status.faults as f, i (i)}<RunLine tone="warn" testid="mimo-fault-{i}" head="Fault" msg={f.message} />{/each}
		</div>
	{/if}
{/snippet}

{#snippet cards(kind: CardKind)}
	{#if kind === 'level'}
		<MimoLevelCard {status} {bands} {compact} />
		{#if status?.law}<p class="law aq-small">Law: {status.law.name} <span class="v">{status.law.version}</span></p>{/if}
	{:else if kind === 'drives'}
		<MimoDrivesCard {status} {ceilings} onpick={compact ? null : (k) => screen.showDrive(k)} />
	{:else if kind === 'prediction'}
		<MimoPredictionCard {status} onopen={compact ? null : () => (mimo.step = 'predict')} />
	{:else if kind === 'station'}
		{#if station}{@render station()}{:else}<p class="aq-muted">The station is shown in the main window.</p>{/if}
	{/if}
{/snippet}

{#snippet grid(panel: BodyPanel, _draw: Snippet<[PlotExtras]>)}
	<MimoGridBody {screen} {panel} />
{/snippet}

{#snippet heatmap(panel: BodyPanel, _draw: Snippet<[PlotExtras]>)}
	<MimoHeatmapBody {screen} {panel} />
{/snippet}

{#snippet shape3d(panel: BodyPanel, _draw: Snippet<[PlotExtras]>)}
	<MimoGeometryBody {screen} {panel} {geometry} {bands} />
{/snippet}

{#snippet data(spec: PlotPanelSpec, change: (spec: PlotPanelSpec) => void)}
	<MimoChoices {spec} onchange={change} labels={screen.labels} drives={screen.driveLabels} {diagnostics} coherence={coherenceModeOffered(screen.quantities)} {virtual} />
{/snippet}

<div class="mimo" bind:this={host}>
	<PulseRunView
		host={mimo}
		{screen}
		prefix="mimo"
		attrs={{ 'data-state': status?.state ?? '', 'data-phase': status?.phase ?? '', 'data-loop': status?.loop ?? 0 }}
		{review}
		{notes}
		{cards}
		bodies={{ sdm: grid, pred_sdm: grid, error_db: heatmap, geometry: shape3d }}
		{data}
		channels={[]}
		{empty}
		live={streaming}
		{onpopout}
		onoverride={() => (mimo.step = 'predict')}
		{abortKeys}
		{compact}
	/>
</div>

<style>
	.mimo {
		height: 100%;
		min-height: 0;
	}

	.open {
		align-self: flex-start;
	}

	.faults {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.law {
		margin: var(--sp-2) 0 0;
		color: var(--text-2);
	}

	.v {
		color: var(--text-3);
	}
</style>
