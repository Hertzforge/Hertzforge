<script lang="ts">
	/**
	 * The Mixed Mode run at a glance: what it is doing in plain words, the level the drive is at
	 * against the level commanded, parts time and the time at this level, the composite against its
	 * reference (shown, not judged), the summed drive, each law's unusable calls and capped loops or
	 * ticks, the identification, and, once the run ends, how and on which part. Every number is the
	 * service's; shape and word carry each state, color only follows.
	 */
	import type { MixedLiveHead, MixedState } from '$lib/protocol/mixed';
	import { formatNumber } from '$lib/viz/format';
	import { clock, db } from '../words';
	import { identificationWords } from '../sine/sineWords';
	import { mixedLevelStatus, mixedTripFacts, MIXED_TRIP_WORDS } from './mixedWords';

	interface Props {
		status: MixedState | null;
		live: MixedLiveHead | null;
		partName: (id: string) => string;
		compact?: boolean;
	}
	let { status, live, partName, compact = false }: Props = $props();

	const st = $derived(mixedLevelStatus(status, live));
	const level = $derived(live?.level_db ?? null);
	const commanded = $derived(live?.commanded_db ?? status?.level_db ?? null);
	const total = $derived(live?.total ?? status?.total ?? null);
	const trip = $derived(status?.end?.trip ?? null);
	const idWords = $derived(identificationWords(status?.identification ?? null));
	const law = $derived(status?.law ?? null);
	const num = (v: number | null | undefined, unit: string, sig = 4) => (v === null || v === undefined ? '-' : `${formatNumber(v, sig)} ${unit}`);
</script>

<section class="levelcard" class:compact aria-label="Level" data-testid="mixed-level-card">
	<div class="statusline {st.tone}" role="status" data-testid="mixed-level-status">
		<span class="mk" aria-hidden="true">
			<svg viewBox="0 0 14 14" width="14" height="14">
				{#if st.mark === 'control'}
					<circle cx="7" cy="7" r="5.5" fill="currentColor" />
				{:else if st.mark === 'identify'}
					<circle cx="7" cy="7" r="5" fill="none" stroke="currentColor" stroke-width="1.5" /><circle cx="7" cy="7" r="1.8" fill="currentColor" />
				{:else if st.mark === 'hold'}
					<rect x="3.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" /><rect x="8.2" y="2.5" width="2.6" height="9" rx="0.8" fill="currentColor" />
				{:else if st.mark === 'ramp'}
					<path d="M2 11L12 3v8z" fill="currentColor" />
				{:else if st.mark === 'stop'}
					<path d="M2 3.5L7 11l5-7.5z" fill="currentColor" />
				{:else if st.mark === 'done'}
					<circle cx="7" cy="7" r="6" fill="none" stroke="currentColor" stroke-width="1.3" /><path d="M4.2 7.2l1.9 1.9 3.8-4" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
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
		{#if status?.mode}<span class="mode">{status.mode === 'auto' ? 'Automatic' : 'Manual'}</span>{/if}
	</div>

	<div class="big num" data-testid="mixed-level-db">{db(level ?? commanded)}{#if level !== null && commanded !== null && Math.abs(level - commanded) > 0.05}<span class="sub"> to {db(commanded)}</span>{/if}{#if status?.held}<span class="tag">Held</span>{/if}</div>

	<dl class="nums">
		<div><dt>Parts time</dt><dd class="num" data-testid="mixed-parts-s">{status?.parts_s === null || status?.parts_s === undefined ? 'Not yet' : clock(status.parts_s)}</dd></div>
		<div><dt>At this level</dt><dd class="num">{clock(status?.at_level_s)}{#if status?.remaining_at_level_s !== null && status?.remaining_at_level_s !== undefined}<span class="sub">, {clock(status.remaining_at_level_s)} left</span>{/if}</dd></div>
		<div><dt>Composite</dt><dd class="num" data-testid="mixed-total">{num(total?.rms_g, 'g rms')}<span class="sub"> against {num(total?.reference_rms_g, 'g rms')}</span></dd></div>
		<div><dt>Summed drive</dt><dd class="num" data-testid="mixed-drive">{num(total?.drive_vpk, 'V pk')}<span class="sub">, {num(total?.drive_vrms, 'V rms')}</span></dd></div>
	</dl>
	{#if status?.limiting}<p class="line warn" data-testid="mixed-limiting">A cap, a limit channel or the drive ceiling holds a part down.</p>{/if}

	{#if law && !compact}
		<p class="aq-small law" data-testid="mixed-law-counts">Floor law {law.floor.name}: {law.floor.misses === 0 ? 'every call answered' : `${law.floor.misses} unusable in a row`}, a cap bound on {law.floor.capped} {law.floor.capped === 1 ? 'loop' : 'loops'}. Tone law {law.tones.name}: {law.tones.misses === 0 ? 'every call answered' : `${law.tones.misses} unusable in a row`}, a cap bound on {law.tones.capped} {law.tones.capped === 1 ? 'tick' : 'ticks'}.</p>
	{/if}
	{#if idWords && !compact}<p class="aq-small aq-muted" data-testid="mixed-identification-facts">{idWords}</p>{/if}

	{#if status?.end}
		<div class="end" data-testid="mixed-end">
			<p class="aq-small">{status.end.message}</p>
			{#if status.end.ramp_s !== null}<p class="aq-small aq-muted">Every part ramped down in {formatNumber(status.end.ramp_s, 3)} s of station time.</p>{/if}
			{#if trip}
				<h4>{MIXED_TRIP_WORDS[trip.kind]}</h4>
				<dl class="facts" data-testid="mixed-trip" data-part={trip.part ?? ''}>
					{#each mixedTripFacts(trip, partName) as f (f.label)}<div><dt>{f.label}</dt><dd class="num">{f.value}</dd></div>{/each}
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

	.mode {
		font-size: var(--text-2xs);
		font-weight: var(--weight-medium);
		color: var(--text-3);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.big {
		font-size: 26px;
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	.compact .big {
		font-size: 20px;
	}

	.tag {
		margin-left: var(--sp-2);
		padding: 1px 6px;
		font-size: var(--text-2xs);
		color: var(--text-2);
		border: 1px solid var(--line-2);
		border-radius: 8px;
		text-transform: uppercase;
		vertical-align: middle;
	}

	.nums,
	.facts {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
		margin: 0;
	}

	dt {
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	dd {
		margin: 0;
		font-size: var(--text-sm);
	}

	.sub {
		font-size: var(--text-sm);
		font-weight: normal;
		color: var(--text-3);
	}

	.line {
		margin: 0;
		font-size: var(--text-sm);
	}

	.warn {
		color: var(--amber-1);
	}

	.law {
		margin: 0;
		color: var(--text-2);
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
