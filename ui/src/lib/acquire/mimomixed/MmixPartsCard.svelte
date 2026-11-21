<script lang="ts">
	/**
	 * The parts during a run, one row each in the parts' order: its id and kind, a switch for a tone
	 * or a narrowband once the parts have come in, its frequency now, whether its clock is held and
	 * whether the ceiling rule is limiting it (a tone at its scale, a narrowband held back while its
	 * drive is out of room) or a narrowband is folded into the floor; under it, a line per control (by
	 * its channel's name) with the target, the measured value, the error in dB and the alarm. A
	 * narrowband is stated in dB against the floor, so it has an error and no target. An alarm is a
	 * triangle and the word, a limiting part a bar and the word: never color alone. A refused switch
	 * is said under its row, with the service's reason.
	 */
	import type { MimoMixedPartRow } from '$lib/protocol/mimoMixed';
	import { errDb, hz, partValue, PART_KIND_WORDS, pct } from './mmixWords';
	import type { MimoMixedStore } from './mmixStore.svelte';

	interface Props {
		mm: MimoMixedStore;
		compact?: boolean;
	}
	let { mm, compact = false }: Props = $props();

	const parts = $derived(mm.now?.parts ?? []);
	const refused = $derived(mm.refusal && (mm.refusal.verb === 'Part On' || mm.refusal.verb === 'Part Off') ? mm.refusal.message : null);
	let last = $state<string | null>(null);

	function toggle(p: MimoMixedPartRow) {
		last = p.id;
		void mm.part(p.id, !p.enabled);
	}
</script>

<section class="partscard" aria-label="Parts" data-testid="mmix-parts-card">
	{#if !parts.length}
		<p class="aq-small aq-muted">The parts appear once the setup is applied.</p>
	{:else}
		<div class="aq-scroll">
			<table class="aq-table" data-testid="mmix-parts-table">
				<thead>
					<tr>
						<th>On</th>
						<th>Part</th>
						<th class="num">Frequency</th>
						<th>State</th>
						<th>Control</th>
						<th class="num">Target</th>
						<th class="num">Measured</th>
						<th class="num">Error</th>
						<th>Alarm</th>
					</tr>
				</thead>
				<tbody>
					{#each parts as p (p.id)}
						{@const t = mm.partToggle(p)}
						{@const cells = compact ? p.controls.slice(0, 1) : p.controls}
						{#each cells as c, k (k)}
							<tr data-testid={k === 0 ? `mmix-part-${p.id}` : undefined} data-on={p.on} data-enabled={p.enabled} data-limiting={p.limiting} data-held={p.held} class:first={k === 0}>
								{#if k === 0}
									<td rowspan={cells.length}>
										{#if p.kind === 'floor'}
											<span class="aq-small aq-muted">Always</span>
										{:else}
											<label class="aq-switch" title={t.reason ?? (p.enabled ? 'Switch the part off, as a ramp' : 'Switch the part on, as a ramp')}>
												<input type="checkbox" role="switch" checked={p.enabled} disabled={!t.enabled} onchange={(e) => { e.currentTarget.checked = p.enabled; toggle(p); }} aria-label="{p.name} {p.enabled ? 'on' : 'off'}" data-testid="mmix-part-{p.id}-switch" />
												<span class="track" aria-hidden="true"><span class="thumb"></span></span>
												<span class="word">{p.enabled ? 'On' : 'Off'}</span>
											</label>
										{/if}
									</td>
									<td rowspan={cells.length}><span class="kind">{PART_KIND_WORDS[p.kind]}</span> {p.name}<br /><span class="id mono">{p.id}</span></td>
									<td rowspan={cells.length} class="num">{p.kind === 'floor' ? '-' : hz(p.freq_hz)}</td>
									<td rowspan={cells.length} class="state" data-testid="mmix-part-{p.id}-state">
										{#if p.limiting}
											<span class="mark warn" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><rect x="1.5" y="5" width="9" height="2" fill="currentColor" /></svg></span>Limiting{p.kind === 'tone' && p.scale !== null ? `, ${pct(p.scale)}` : ''}
										{:else if p.held}
											<span class="mark" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><rect x="2.5" y="2" width="2.4" height="8" fill="currentColor" /><rect x="7" y="2" width="2.4" height="8" fill="currentColor" /></svg></span>Held
										{:else if p.on}
											<span class="mark ok" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="4.5" fill="currentColor" /></svg></span>{p.kind === 'narrowband' && p.scale !== null && p.scale < 0.999 ? 'Ramping' : 'Playing'}
										{:else}
											<span class="mark" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="4.2" fill="none" stroke="currentColor" stroke-width="1.2" /></svg></span>{mm.now?.state === 'finished' ? 'Ended' : p.enabled ? 'Waiting' : 'Off'}
										{/if}
										{#if p.folded}<br /><span class="sub" data-testid="mmix-part-{p.id}-folded">Folded into the floor</span>{/if}
									</td>
								{/if}
								<td class="ctl">{mm.channelLabel(c.channel) || `Control ${c.channel}`}</td>
								<td class="num">{partValue(c.target, p.unit)}</td>
								<td class="num">{partValue(c.measured, p.unit)}</td>
								<td class="num" class:warn={c.alarm}>{errDb(c.error_db)}</td>
								<td class="state">{#if c.alarm}<span class="mark warn" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><path d="M6 1l5 9.5H1z" fill="currentColor" /></svg></span>Alarm{:else}<span class="aq-muted">-</span>{/if}</td>
							</tr>
						{/each}
						{#if refused && last === p.id}
							<tr class="refused"><td></td><td colspan="8"><span role="alert" data-testid="mmix-part-{p.id}-refused">{refused}</span></td></tr>
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

	.kind,
	.sub,
	.id {
		color: var(--text-3);
		font-size: var(--text-xs);
	}

	td.num,
	.state,
	.ctl {
		white-space: nowrap;
	}

	tr.first td {
		border-top: 1px solid var(--line-1);
	}

	tr:not(.first) td {
		border-top: none;
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
