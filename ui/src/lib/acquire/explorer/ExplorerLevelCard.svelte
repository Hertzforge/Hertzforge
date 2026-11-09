<script lang="ts">
	/**
	 * The Explorer run at a glance: what the tone is doing in plain words, its frequency large with
	 * the target beside it when they differ, the frequency measured on the control channel, the
	 * drive in volts peak against its target and the ceiling, the control channel's rms and derived
	 * motion, the sweep, the survey's highest point, and, once the run ends, how and why. Every
	 * number is the service's; shape and word carry each state, color only follows.
	 */
	import type { ExplorerLiveHead, ExplorerState } from '$lib/protocol/explorer';
	import { formatNumber } from '$lib/viz/format';
	import { clock } from '../words';
	import { endWords, EXPLORER_STOP_WORDS, EXPLORER_TRIP_WORDS, explorerLevelStatus, explorerTripFacts, hz, motionWords, sweepWords, vpk } from './explorerWords';

	interface Props {
		/** The state with the live frame laid over it. */
		status: ExplorerState | null;
		/** The last frame's numbers, when they are this run's. */
		live: ExplorerLiveHead | null;
		compact?: boolean;
	}
	let { status, live, compact = false }: Props = $props();

	const st = $derived(explorerLevelStatus(status));
	const freq = $derived(status?.frequency_hz ?? null);
	const target = $derived(status?.target_hz ?? null);
	const drive = $derived(status?.drive_vpk ?? null);
	const targetV = $derived(status?.target_vpk ?? null);
	const control = $derived(live?.channels.find((c) => c.index === live?.control?.index) ?? live?.channels[0] ?? null);
	const motion = $derived(motionWords(live?.control ?? null));
	const sweep = $derived(sweepWords(status?.sweep ?? null, freq, status?.band_hz ?? null));
	const peak = $derived(live?.peak ?? null);
	const trip = $derived(status?.end?.trip ?? null);
	const endLine = $derived(status?.end ? endWords(status.end) : null);
	const n = (v: number, sig = 4) => formatNumber(v, sig);
</script>

<section class="levelcard" class:compact aria-label="Tone" data-testid="explorer-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="explorer-level-status">
		<span class="mk" aria-hidden="true">
			<svg viewBox="0 0 14 14" width="14" height="14">
				{#if st.mark === 'control'}
					<circle cx="7" cy="7" r="5.5" fill="currentColor" />
				{:else if st.mark === 'hold'}
					<rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
				{:else if st.mark === 'ramp'}
					<path d="M2 11L12 3v8z" fill="currentColor" />
				{:else if st.mark === 'stop'}
					<path d="M2 3.5L7 11l5-7.5z" fill="currentColor" />
				{:else if st.mark === 'ended'}
					<rect x="2.5" y="2.5" width="9" height="9" rx="1.3" fill="currentColor" />
				{:else if st.mark === 'armed'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><path d="M7 2a5 5 0 010 10z" fill="currentColor" />
				{:else}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2.2 2" />
				{/if}
			</svg>
		</span>
		<span class="words">{st.text}</span>
	</div>

	<div class="freq" data-testid="explorer-frequency" data-hz={freq ?? ''} data-target={target ?? ''}>
		<span class="big num">{hz(freq)}</span>
		{#if target !== null && freq !== null && Math.abs(target - freq) > 0.05}<span class="sub num">to {hz(target)}</span>{/if}
	</div>
	{#if sweep}<p class="line" data-testid="explorer-sweep-words">{sweep}</p>{/if}

	<dl class="nums">
		<div>
			<dt>Drive</dt>
			<dd class="num" data-testid="explorer-drive" data-vpk={drive ?? ''} data-target={targetV ?? ''}>{vpk(drive)}{#if targetV !== null && drive !== null && Math.abs(targetV - drive) > 5e-4}<span class="sub"> to {vpk(targetV)}</span>{/if}</dd>
		</div>
		<div><dt>Ceiling</dt><dd class="num">{vpk(status?.max_drive_vpk)}</dd></div>
		<div><dt>Measured</dt><dd class="num" data-testid="explorer-measured" data-hz={live?.measured_hz ?? ''}>{live?.measured_hz === null || live?.measured_hz === undefined ? 'Not yet' : hz(live.measured_hz)}</dd></div>
		<div><dt>Control rms</dt><dd class="num" data-testid="explorer-control-rms">{control?.rms === null || control?.rms === undefined ? '-' : `${n(control.rms)} ${control.unit}`}</dd></div>
		{#if !compact}
			<div><dt>Motion</dt><dd class="num" data-testid="explorer-motion">{motion ?? '-'}</dd></div>
			<div><dt>Station time</dt><dd class="num">{clock(status?.t_s)}</dd></div>
		{/if}
	</dl>

	{#if peak}
		<p class="line" data-testid="explorer-peak" data-hz={peak.hz}>Survey peak {hz(peak.hz)}, {n(peak.level)} {control?.unit ?? ''}</p>
	{/if}
	{#if status?.stop && status.state === 'stopping'}<p class="aq-small aq-muted" data-testid="explorer-stop-cause">{EXPLORER_STOP_WORDS[status.stop]}.</p>{/if}

	{#if status?.end}
		<div class="end" data-testid="explorer-end" data-outcome={status.end.outcome}>
			<p class="aq-small">{status.end.message}</p>
			{#if endLine}<p class="aq-small aq-muted">{endLine}</p>{/if}
			{#if trip}
				<h4>{EXPLORER_TRIP_WORDS[trip.kind]}</h4>
				<dl class="facts" data-testid="explorer-trip">
					{#each explorerTripFacts(trip) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
				</dl>
			{/if}
		</div>
	{/if}
</section>

<style>
	.levelcard {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}

	.statusline {
		display: flex;
		align-items: center;
		gap: 6px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.statusline .mk {
		display: inline-flex;
	}

	.statusline.pass .mk {
		color: var(--blue-2);
	}

	.statusline.warn .mk,
	.statusline.fail .mk {
		color: var(--amber-1);
	}

	.statusline.info .mk {
		color: var(--text-3);
	}

	.words {
		flex: 1;
		min-width: 0;
	}

	.freq {
		display: flex;
		align-items: baseline;
		gap: var(--sp-2);
	}

	.big {
		font-size: 28px;
		font-weight: var(--weight-strong);
		color: var(--text-1);
		letter-spacing: -0.01em;
	}

	.compact .big {
		font-size: 22px;
	}

	.sub {
		color: var(--text-3);
	}

	.line {
		margin: 0;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.nums,
	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
		margin: 0;
	}

	.nums dt,
	.facts dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.nums dd,
	.facts dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.end {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--line-1);
	}

	.end p {
		margin: 0 0 4px;
	}

	h4 {
		margin: var(--sp-2) 0 4px;
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	p {
		margin: 0;
	}
</style>
