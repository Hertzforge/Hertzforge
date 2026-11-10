<script lang="ts">
	/**
	 * A geometry plot: the sensors and drives in three dimensions, placed from the channel table's
	 * positions and directions and drawn by the shape pane. Under physical control each sensor is
	 * shaded by its control's current error on the heatmap's lightness scale, and the list beside it
	 * names each control's band in words; drives are arrows along their directions, longer for more
	 * drive rms. Under virtual control the rigid-body motion is animated from the six degrees of
	 * freedom's rms (switched in the plot's options), and their errors are listed beside it.
	 */
	import type { ReadingView } from '$lib/protocol/types';
	import { cssVar, theme } from '$lib/theme/theme.svelte';
	import { formatDb, formatNumber } from '$lib/viz/format';
	import { arrowKey, buildGeometry, dofVector, nodeColors, type MimoGeometry } from '$lib/viz/geometry';
	import { BAND_WORDS, bandClass, DEFAULT_BAND, errorCss, errorGradient, type RowBand } from '$lib/viz/scales';
	import type { BodyPanel } from '../../screen/bodies';
	import { geometryChoice } from './mimoScreen';
	import type { MimoScreenState } from './mimoScreenState.svelte';

	interface Props {
		screen: MimoScreenState;
		panel: BodyPanel;
		/** Where the sensors and drives are, from the channel table; null when not entered. */
		geometry?: MimoGeometry | null;
		/** Each control's alarm and abort bands, in the setup's order. */
		bands?: RowBand[];
	}
	let { screen, panel, geometry = null, bands = [] }: Props = $props();

	const shape3d = () => import('$lib/viz/Shape3DPane.svelte');

	const s = $derived(geometryChoice(panel.plot.select));
	const status = $derived(screen.status);
	const virtual = $derived(status?.shape?.space === 'virtual');
	const errors = $derived(status?.control_error_db ?? []);
	const driveRms = $derived(status?.drive_rms_v ?? []);
	const dofReading = $derived((screen.plotData(panel.id)?.views[0] as ReadingView | undefined) ?? null);
	const dof = $derived(virtual && s.motion && dofReading ? dofVector(dofReading.rows) : null);
	const dofKey = $derived(dof ? dof.map((v) => v.toPrecision(2)).join(',') : '');
	const tk = $derived<'light' | 'dark'>(theme.resolved);

	/** The mesh is rebuilt only when the arrows visibly change or the motion's shape does. */
	const key = $derived(`${arrowKey(driveRms)}|${dofKey}`);
	const mesh = $derived.by(() => {
		void key;
		return geometry ? buildGeometry(geometry, driveRms, dof, 'Geometry') : null;
	});
	const colors = $derived.by(() => {
		void theme.version;
		if (!mesh) return null;
		const neutralCss = cssVar('--text-3') || '#74828f';
		return nodeColors(mesh, virtual ? [] : (geometry?.sensors.map((_, i) => errors[i] ?? null) ?? []), bands, tk, hexToRgb(neutralCss));
	});

	function hexToRgb(css: string): [number, number, number] {
		const m = /^#([0-9a-f]{6})$/i.exec(css.trim());
		if (!m) return [0.45, 0.5, 0.56];
		const n = parseInt(m[1], 16);
		return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
	}

	const labels = $derived(screen.labels);
	const band0 = $derived(bands[0] ?? DEFAULT_BAND);
</script>

<div class="geo" data-testid="mimo-geometry">
	<div class="scene">
		{#if mesh && colors}
			{#await shape3d()}
				<div class="waiting">Loading the 3-D view.</div>
			{:then { default: Shape3DPane }}
				{#key key}
					<Shape3DPane view={mesh.view} {colors}>
						{#snippet legend()}
							<div class="ltitle">{virtual ? 'Sensors and drives' : 'Sensor color: control error'}</div>
							{#if !virtual}
								<div class="bar" style:background={errorGradient(band0, tk)}></div>
								<div class="ticks num"><span>{formatDb(-2 * band0.abort_db[0], 0)}</span><span>0 dB</span><span>{formatDb(2 * band0.abort_db[1], 0)}</span></div>
							{/if}
							<div class="note">Arrows are drives, longer for more drive rms.</div>
						{/snippet}
					</Shape3DPane>
				{/key}
			{:catch err}
				<div class="waiting">The 3-D view failed to load: {err.message}</div>
			{/await}
		{:else}
			<div class="waiting" data-testid="geometry-empty">
				Sensor positions and directions are entered in the rigid-body geometry, under Reference in the setup. The geometry shows them here once every control has one.
			</div>
		{/if}
	</div>

	<aside class="list" aria-label={virtual ? 'Degrees of freedom' : 'Controls'}>
		<table>
			<thead><tr><th>{virtual ? 'DOF' : 'Control'}</th><th class="r">Error</th><th>Band</th>{#if dofReading}<th class="r">rms</th>{/if}</tr></thead>
			<tbody>
				{#each labels as l, i (i)}
					{@const e = errors[i]}
					{@const b = bands[i] ?? DEFAULT_BAND}
					<tr>
						<td><span class="sw" style:background={e === undefined ? 'transparent' : errorCss(e, b, tk)}></span>{l}</td>
						<td class="r num">{e === undefined ? '-' : formatDb(e, 2)}</td>
						<td class="band">{e === undefined ? '' : BAND_WORDS[bandClass(e, b)]}</td>
						{#if dofReading}
							{@const row = dofReading.rows.find((r) => r.label === l)}
							<td class="r num">{row ? (typeof row.value === 'number' ? formatNumber(row.value, 4) : row.value) : '-'}{row?.unit ? ` ${row.unit}` : ''}</td>
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
	</aside>
</div>

<style>
	.geo {
		display: grid;
		grid-template-columns: minmax(0, 1fr) 240px;
		gap: var(--sp-2);
		height: 100%;
		min-height: 0;
		container-type: inline-size;
	}

	.scene {
		min-width: 0;
		min-height: 0;
	}

	/* A narrow panel keeps the view and lists the controls under it. */
	@container (max-width: 560px) {
		.geo {
			grid-template-columns: minmax(0, 1fr);
			grid-template-rows: minmax(0, 1fr) minmax(0, 34%);
		}
	}

	.waiting {
		display: grid;
		place-items: center;
		height: 100%;
		padding: var(--sp-4);
		color: var(--text-3);
		font-size: var(--text-sm);
		text-align: center;
		background: var(--bg-plot);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-md);
	}

	.ltitle {
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	.bar {
		width: 180px;
		height: 9px;
		margin: 3px 0 1px;
		border: 1px solid var(--line-2);
		border-radius: 2px;
	}

	.ticks {
		display: flex;
		justify-content: space-between;
		width: 180px;
		font-size: var(--text-2xs);
		color: var(--text-2);
	}

	.note {
		margin-top: 3px;
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.list {
		min-height: 0;
		overflow-y: auto;
	}

	table {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--text-xs);
	}

	th {
		padding: 3px 4px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		color: var(--text-3);
		text-align: left;
		text-transform: uppercase;
		letter-spacing: 0.04em;
		border-bottom: 1px solid var(--line-2);
	}

	td {
		padding: 3px 4px;
		border-bottom: 1px solid var(--line-1);
	}

	.r {
		text-align: right;
	}

	.band {
		color: var(--text-2);
	}

	.sw {
		display: inline-block;
		width: 10px;
		height: 10px;
		margin-right: 6px;
		vertical-align: -1px;
		border: 1px solid var(--line-3);
		border-radius: 2px;
	}
</style>
