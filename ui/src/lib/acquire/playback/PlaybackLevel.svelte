<script lang="ts">
	/**
	 * The level the drive plays at, the drive ceiling every sample is judged against, the seconds
	 * recorded at zero drive before and after it, and the response aborts. The aborts are the
	 * channel table's own abort limits of each enabled input row, read on every sample by the
	 * control service: they are shown here as the table holds them and edited in the table, never
	 * here. Every number is judged by the control service in its dry run.
	 */
	import NumberField from '../NumberField.svelte';
	import { playbackRows } from './playbackSetup';
	import type { PlaybackStore } from './playbackStore.svelte';
	import { formatNumber } from '$lib/viz/format';

	interface Props {
		playback: PlaybackStore;
	}
	let { playback }: Props = $props();

	const d = $derived(playback.setup);
	const locked = $derived(playback.locked);
	const E = $derived(playback.fieldErrors);
	const W = $derived(playback.fieldWarnings);
	const errs = (k: string) => E[k] ?? [];
	const inputs = $derived(playbackRows(playback.rows).inputs);
	const dv = $derived(playback.derived);
	const limits = (r: (typeof inputs)[number]) => {
		const a = r.abort ?? { accel_g: null, velocity_ips: null, displacement_mil: null };
		const out: string[] = [];
		if (a.accel_g !== null && a.accel_g !== undefined) out.push(`${formatNumber(a.accel_g, 4)} g`);
		if (a.velocity_ips !== null && a.velocity_ips !== undefined) out.push(`${formatNumber(a.velocity_ips, 4)} in/s`);
		if (a.displacement_mil !== null && a.displacement_mil !== undefined) out.push(`${formatNumber(a.displacement_mil, 4)} mil`);
		return out.length ? out.join(', ') : 'None';
	};
</script>

<div class="stack" data-testid="playback-level">
	<section class="aq-panel" aria-labelledby="pb-lvl">
		<div class="aq-head"><h2 id="pb-lvl">Level and Ceiling</h2></div>
		<div class="aq-fields">
			<NumberField id="pb-level" label="Level" unit="dB" step={1} min={-40} max={0} bind:value={d.level_db} errors={errs('level_db')} warnings={W['level_db'] ?? []} disabled={locked} testid="pb-level" help="From -40 to 0 dB. Every sample of the file is multiplied by 10^(level/20); one run plays at one level." />
			<NumberField id="pb-ceiling" label="Drive ceiling" unit="V" step={0.1} min={0} bind:value={d.ceiling_v} errors={errs('ceiling_v')} disabled={locked} testid="pb-ceiling" help="Above 0, at most 10 V and each mapped row's range. Each channel's peak at the level must be within it, and every sample played is judged against it." />
		</div>
		{#if dv}
			<p class="aq-small" data-testid="pb-gain">At {formatNumber(d.level_db, 3)} dB the file plays at {formatNumber(dv.gain, 4)} times its values.</p>
		{/if}
	</section>

	<section class="aq-panel" aria-labelledby="pb-rolls">
		<div class="aq-head"><h2 id="pb-rolls">Rolls</h2></div>
		<div class="aq-fields">
			<NumberField id="pb-pre" label="Pre-roll" unit="s" step={0.1} min={0} max={60} bind:value={d.pre_roll_s} errors={errs('pre_roll_s')} disabled={locked} testid="pb-pre" help="Recorded at zero drive before the file, 0 to 60 s." />
			<NumberField id="pb-post" label="Post-roll" unit="s" step={0.1} min={0} max={60} bind:value={d.post_roll_s} errors={errs('post_roll_s')} disabled={locked} testid="pb-post" help="Recorded at zero drive after the file, 0 to 60 s." />
		</div>
		{#if dv}
			<p class="aq-small" data-testid="pb-total">The run records {formatNumber(dv.total_s, 4)} s: {dv.pre_roll_frames} frames before the file, {dv.samples} of the file and {dv.post_roll_frames} after, at {formatNumber(dv.fs_hz, 6)} Hz. Stop ramps the drive {formatNumber(dv.stop.depth_db, 3)} dB down over {formatNumber(dv.stop.ramp_s, 3)} s; Abort takes it to zero at once.</p>
		{/if}
	</section>

	<section class="aq-panel" aria-labelledby="pb-aborts" data-testid="pb-aborts">
		<div class="aq-head"><h2 id="pb-aborts">Response Aborts</h2></div>
		<p class="aq-small">Each enabled input row's abort limits, from the channel table, are read on every sample: a breach takes the drive to zero at once and ends the run. An input overload on a row with limits does the same. Set them in the channel table (Channels).</p>
		{#if inputs.length}
			<table class="aq-table">
				<thead><tr><th>Row</th><th>Description</th><th>Unit</th><th>Abort limits</th></tr></thead>
				<tbody>
					{#each inputs as r (r.index)}
						<tr><td class="num">{r.index}</td><td>{r.description}</td><td>{r.units}</td><td class="num">{limits(r)}</td></tr>
					{/each}
				</tbody>
			</table>
		{:else}
			<p class="aq-small aq-muted">No input row is enabled: the run records nothing to judge.</p>
		{/if}
	</section>
</div>

<style>
	.stack {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		max-width: 860px;
	}

	p {
		margin: var(--sp-2) 0 0;
	}
</style>
