<script lang="ts">
	/**
	 * The prepared section's kinematics at full level as the control service computes them: its peak
	 * acceleration, peak velocity and peak-to-peak displacement, integrated after the kinematics
	 * high-pass, and the verdict against the feasibility ceilings set. A computation from the
	 * reference, never a measurement.
	 */
	import type { TwrFeasibilityFacts, TwrKinematics } from '$lib/protocol/twr';
	import StatusMark from '$lib/viz/StatusMark.svelte';
	import { formatNumber } from '$lib/viz/format';

	interface Props {
		kinematics: TwrKinematics | null;
		feasibility: TwrFeasibilityFacts | null;
		/** The control unit the acceleration is in. */
		unit: string;
	}
	let { kinematics, feasibility, unit }: Props = $props();

	const tone = $derived(feasibility?.verdict === 'exceeded' ? 'fail' : feasibility?.verdict === 'advisory' ? 'warn' : feasibility?.verdict === 'pass' ? 'pass' : 'info');
	const CHECK_WORDS: Record<string, string> = { acceleration: 'Acceleration', velocity: 'Velocity', displacement: 'Displacement' };
</script>

<section class="kin" aria-label="Kinematics" data-testid="twr-kinematics">
	<h3>Kinematics at Full Level</h3>
	{#if kinematics}
		<dl class="nums">
			<div><dt>Peak acceleration</dt><dd class="num" data-testid="twr-kin-accel">{formatNumber(kinematics.peak_accel, 4)} {unit} pk</dd></div>
			<div><dt>Peak velocity</dt><dd class="num" data-testid="twr-kin-velocity">{formatNumber(kinematics.peak_velocity_ips, 4)} in/s pk</dd></div>
			<div><dt>Displacement</dt><dd class="num" data-testid="twr-kin-disp">{formatNumber(kinematics.disp_pkpk_in, 4)} in pk-pk</dd></div>
		</dl>
		<p class="aq-small aq-muted">Integrated after a {formatNumber(kinematics.kinematics_hp_hz, 3)} Hz high-pass.</p>
	{/if}
	{#if feasibility}
		<p class="verdict" data-testid="twr-kin-verdict" data-verdict={feasibility.verdict}><StatusMark status={tone} word={false} /> {feasibility.message}</p>
		{#if feasibility.checks.length}
			<ul class="checks">
				{#each feasibility.checks as c (c.name)}
					<li class:fail={!c.pass}><StatusMark status={c.pass ? 'pass' : 'fail'} word={false} /><span>{CHECK_WORDS[c.name] ?? c.name}: {formatNumber(c.value, 4)} against {formatNumber(c.limit, 4)}</span><span class="w">{c.pass ? 'Within' : 'Over'}</span></li>
				{/each}
			</ul>
		{/if}
	{/if}
</section>

<style>
	.kin {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-2) 0;
		border-top: 1px solid var(--line-1);
		border-bottom: 1px solid var(--line-1);
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
		text-transform: uppercase;
		letter-spacing: 0.04em;
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

	.checks {
		margin: 0;
		padding: 0;
		list-style: none;
		font-size: var(--text-xs);
	}

	.checks li {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.checks li.fail {
		font-weight: var(--weight-strong);
	}

	.w {
		margin-left: auto;
		color: var(--text-3);
	}
</style>
