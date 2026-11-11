<script lang="ts">
	/**
	 * The parts during a run, one row each in the parts' order: whether it plays (a switch for a tone
	 * or a narrowband once the parts have come in), what it is doing, its frequency or center, its
	 * target against its measured value with the error in dB, its drive, and its state. A part in its
	 * alarm band is marked with a triangle and the word "Alarm"; a part not yet settled with a ring
	 * and "Settling": never by color alone. A refused switch is said in one line under its row.
	 */
	import type { MixedPartRow } from '$lib/protocol/mixed';
	import { formatNumber } from '$lib/viz/format';
	import type { MixedStore } from './mixedStore.svelte';
	import { errDb, hz, partMotion, partValue, PART_KIND_WORDS } from './mixedWords';

	interface Props {
		mixed: MixedStore;
		compact?: boolean;
	}
	let { mixed, compact = false }: Props = $props();

	const parts = $derived(mixed.parts);
	const held = $derived(!!mixed.now?.held);
	const drive = (p: MixedPartRow) => (p.drive_vpk !== null ? `${formatNumber(p.drive_vpk, 3)} V pk` : p.drive_vrms !== null ? `${formatNumber(p.drive_vrms, 3)} V rms` : '-');
	const lawWords = (p: MixedPartRow) => (p.law && (p.law.misses || p.law.capped) ? `${p.law.misses} unusable in a row, a cap bound ${p.law.capped} times` : '');
	const extra = (p: MixedPartRow) =>
		p.kind === 'tone'
			? lawWords(p)
			: p.kind === 'floor' ? ([p.lines ? `${p.lines.clean} of ${p.lines.in_band} lines clean, ${p.lines.alarm} in alarm, ${p.lines.abort} over abort; ${p.dof ?? '-'} dof` : '', lawWords(p)].filter(Boolean).join('; ')) : p.kind === 'narrowband' ? [p.width_hz !== null ? `${formatNumber(p.width_hz, 3)} Hz wide` : '', p.dof !== null ? `${p.dof} dof` : '', p.gain_db !== null ? `gain ${errDb(p.gain_db)}` : ''].filter(Boolean).join(', ') : '';
</script>

<section class="partscard" aria-label="Parts" data-testid="mixed-parts-card">
	{#if !parts.length}
		<p class="aq-small aq-muted">The parts appear once the setup is applied.</p>
	{:else}
		<div class="aq-scroll">
			<table class="aq-table" data-testid="mixed-parts-table">
				<thead>
					<tr>
						<th>On</th>
						<th>Part</th>
						<th>Doing</th>
						<th class="num">Frequency</th>
						<th class="num">Target</th>
						<th class="num">Measured</th>
						<th class="num">Error</th>
						{#if !compact}<th class="num">Drive</th>{/if}
						<th>State</th>
					</tr>
				</thead>
				<tbody>
					{#each parts as p (p.id)}
						{@const t = mixed.toggleOf(p)}
						<tr data-testid="mixed-part-{p.id}" data-on={p.on} data-enabled={p.enabled} data-alarm={p.alarm} data-settled={p.settled}>
							<td>
								{#if p.kind === 'floor'}
									<span class="aq-small aq-muted">Always</span>
								{:else}
									<label class="aq-switch" title={t.reason ?? (t.to ? 'Switch the part on, as a ramp' : 'Switch the part off, as a ramp')}>
										<input type="checkbox" role="switch" checked={p.enabled} disabled={!t.enabled} onchange={(e) => { e.currentTarget.checked = p.enabled; void mixed.part(p.id, t.to); }} aria-label="{p.name} {p.enabled ? 'on' : 'off'}" data-testid="mixed-part-{p.id}-switch" />
										<span class="track" aria-hidden="true"><span class="thumb"></span></span>
										<span class="word">{p.enabled ? 'On' : 'Off'}</span>
									</label>
								{/if}
							</td>
							<td><span class="kind">{PART_KIND_WORDS[p.kind]}</span> {p.name}</td>
							<td class="aq-small">{partMotion(p, held)}{#if extra(p)}<span class="sub"><br />{extra(p)}</span>{/if}</td>
							<td class="num">{p.freq_hz === null ? '-' : hz(p.freq_hz)}</td>
							<td class="num">{partValue(p.target, p.unit)}</td>
							<td class="num">{partValue(p.measured, p.unit)}</td>
							<td class="num" class:warn={p.alarm}>{errDb(p.error_db)}</td>
							{#if !compact}<td class="num">{drive(p)}</td>{/if}
							<td class="state">
								{#if p.alarm}
									<span class="mark warn" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1l5 9.5H1z" fill="currentColor" /></svg></span>Alarm
								{:else if p.limiting}
									<span class="mark warn" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><rect x="1.5" y="5" width="9" height="2" fill="currentColor" /></svg></span>Limiting
								{:else if p.on && !p.settled}
									<span class="mark" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="4.2" fill="none" stroke="currentColor" stroke-width="1.4" stroke-dasharray="2 1.6" /></svg></span>Settling
								{:else if p.on}
									<span class="mark ok" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="4.5" fill="currentColor" /></svg></span>In band
								{:else}
									<span class="mark" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="4.2" fill="none" stroke="currentColor" stroke-width="1.2" /></svg></span>Off
								{/if}
							</td>
						</tr>
						{#if mixed.partRefusal?.id === p.id}
							<tr class="refused"><td></td><td colspan="8"><span role="alert" data-testid="mixed-part-{p.id}-refused">{mixed.partRefusal.message}</span></td></tr>
						{/if}
					{/each}
				</tbody>
			</table>
		</div>
	{/if}
</section>

<style>
	.partscard {
		min-width: 0;
	}

	.kind {
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	.sub {
		color: var(--text-3);
	}

	.state {
		white-space: nowrap;
	}

	.mark {
		display: inline-flex;
		margin-right: 4px;
		vertical-align: -1px;
		color: var(--text-3);
	}

	.mark.ok {
		color: var(--blue-2);
	}

	.mark.warn,
	td.warn {
		color: var(--amber-1);
	}

	.refused td {
		font-size: var(--text-xs);
		color: var(--amber-1);
	}
</style>
