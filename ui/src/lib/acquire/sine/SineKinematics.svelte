<script lang="ts">
	/**
	 * The profile's kinematics as the control service reads them: its peak acceleration, velocity
	 * and displacement over the test's band, where it changes quantity, and, when a shaker is given,
	 * the verdict against its ratings. A computation from the profile, never a measurement.
	 */
	import type { SineCrossover, SineKinematics } from '$lib/protocol/sine';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';
	import { QUANTITIES } from './sineSetup';
	import { hz } from './sineWords';

	interface Props {
		kinematics: SineKinematics | null;
		crossovers: SineCrossover[];
	}
	let { kinematics, crossovers }: Props = $props();

	const word = (q: string) => QUANTITIES.find((x) => x.id === q)?.label.toLowerCase() ?? q;
	const tone = $derived(kinematics?.verdict === 'exceeded' ? 'fail' : kinematics?.verdict === 'advisory' ? 'warn' : kinematics?.verdict === 'pass' ? 'pass' : 'info');
</script>

<section class="kin" aria-label="Kinematics" data-testid="sine-kinematics">
	<h3>Kinematics</h3>
	{#if kinematics}
		<dl class="nums">
			<div><dt>Peak acceleration</dt><dd class="num" data-testid="sine-kin-accel">{formatNumber(kinematics.peak_accel_g, 4)} g pk</dd></div>
			<div><dt>Peak velocity</dt><dd class="num" data-testid="sine-kin-velocity">{formatNumber(kinematics.peak_velocity_ips, 4)} in/s pk</dd></div>
			<div><dt>Peak displacement</dt><dd class="num" data-testid="sine-kin-disp">{formatNumber(kinematics.peak_disp_pp_in, 4)} in pk-pk</dd></div>
		</dl>
		<p class="verdict" data-testid="sine-kin-verdict" data-verdict={kinematics.verdict}><StatusMark status={tone} word={false} /> {kinematics.message}</p>
	{:else}
		<p class="aq-muted aq-small">The kinematics are read from the profile once it holds numbers.</p>
	{/if}
	{#if crossovers.length}
		<ul class="cross" data-testid="sine-crossovers">
			{#each crossovers as c, i (i)}<li>At {hz(c.freq_hz)} the profile changes from {word(c.from)} to {word(c.to)}.</li>{/each}
		</ul>
	{/if}
	<p class="aq-small aq-muted">Computed from the profile by the control service, never a measurement.</p>
</section>

<style>
	.kin {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	h3 {
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.nums {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		gap: var(--sp-2);
		margin: 0;
	}

	.nums dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.nums dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.verdict {
		display: flex;
		align-items: center;
		gap: 6px;
		margin: 0;
		font-size: var(--text-sm);
	}

	.cross {
		margin: 0;
		padding-left: 1.2em;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	p {
		margin: 0;
	}
</style>
