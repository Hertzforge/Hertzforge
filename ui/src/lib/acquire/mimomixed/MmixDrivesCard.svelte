<script lang="ts">
	/**
	 * The drives' shared budget now: per drive (by its channel's name), the floor's random (its rms
	 * and its held peak), the tones' summed peak, the largest value played, the headroom left to the
	 * tones against the peak ceiling (in volts and dB), the peak the run aborts at, and a bar of the
	 * whole against the ceiling. Under it, the ceiling rule's common scale on the tones, in words.
	 */
	import { needWords, scaleWords, volts } from './mmixWords';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
	}
	let { mm }: Props = $props();

	const s = $derived(mm.now);
	const drives = $derived(s?.drives ?? []);
	const rule = $derived(mm.runConfig?.law?.ceiling_rule?.id ?? null);
	const tonesOn = $derived(!!s?.parts.some((p) => p.kind === 'tone' && p.on));
	const dB = (v: number | null) => (v === null || !Number.isFinite(v) ? '-' : `${v.toFixed(1)} dB`);
</script>

<section aria-label="Drives" data-testid="mmix-drives-card">
	{#if !drives.length}
		<p class="aq-small aq-muted">The drives appear once the setup is applied.</p>
	{:else}
		<div class="aq-scroll">
			<table class="aq-table" data-testid="mmix-drives-table">
				<thead><tr><th>Drive</th><th class="num">Random rms</th><th class="num">Held peak</th><th class="num">Tone sum</th><th class="num">Played peak</th><th class="num" title="The peak ceiling less the random part's held peak: the room left to the tones.">Headroom</th><th class="num" title="The peak the run aborts at: the drive's own, or its peak ceiling plus 1 dB.">Peak abort</th><th>Need against ceiling</th></tr></thead>
				<tbody>
					{#each drives as d, j (d.channel)}
						{@const need = (d.random_peak_v ?? 0) + (d.tone_sum_v ?? 0)}
						{@const ceiling = d.ceiling_v ?? 0}
						{@const short = ceiling > 0 && need >= ceiling * 0.98}
						{@const share = ceiling > 0 ? Math.min(1.2, need / ceiling) : 0}
						<tr data-testid="mmix-drive-live-{j}" data-short={short}>
							<td>{mm.channelLabel(d.channel) || `Drive ${d.channel}`}</td>
							<td class="num">{volts(d.random_rms_v)}</td>
							<td class="num">{volts(d.random_peak_v)}</td>
							<td class="num">{volts(d.tone_sum_v)}</td>
							<td class="num">{volts(d.peak_v)}</td>
							<td class="num">{volts(d.headroom_v)}{d.headroom_db !== null ? ` (${dB(d.headroom_db)})` : ''}</td>
							<td class="num" data-testid="mmix-drive-abort-{j}">{volts(d.abort_vpk)}</td>
							<td class="need">
								<span class="bar" aria-hidden="true"><span class="fill" class:short style:width="{(share * 80).toFixed(1)}%"></span><span class="ceil"></span></span>
								<span class="aq-small">{ceiling > 0 ? needWords(need, ceiling) : '-'}{short ? ', out of room' : ''}</span>
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</div>
		<p class="aq-small scale" data-testid="mmix-scale" data-scale={s?.scale ?? ''}>{tonesOn ? scaleWords(s?.scale ?? null, rule) : 'No tone is playing.'}</p>
	{/if}
</section>

<style>
	td.num {
		white-space: nowrap;
	}

	.need {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		white-space: nowrap;
	}

	.bar {
		position: relative;
		display: inline-block;
		width: 90px;
		height: 8px;
		background: var(--bg-3);
		border-radius: 3px;
	}

	.fill {
		position: absolute;
		inset: 0 auto 0 0;
		background: var(--blue-2);
		border-radius: 3px;
	}

	.fill.short {
		background: var(--amber-1);
	}

	.ceil {
		position: absolute;
		left: 80%;
		top: -2px;
		bottom: -2px;
		width: 2px;
		background: var(--text-1);
	}

	.scale {
		margin: var(--sp-2) 0 0;
	}
</style>
