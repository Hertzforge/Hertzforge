<script lang="ts">
	/**
	 * The parts during a run, one row each in the parts' order: whether it plays (a switch for a tone
	 * or a narrowband once the parts have come in), what it is doing, its frequency or center, its
	 * target against its measured value with the error in dB, its drive, and its state. A part in its
	 * alarm band is marked with a triangle and the word "Alarm"; a part not yet settled with a ring
	 * and "Settling": never by color alone. A follower says what it follows; a part not playing says
	 * whether it is off, waiting for its window or ended. A refused switch is said in one line under
	 * its row.
	 */
	import type { MixedStore } from './mixedStore.svelte';
	import { errDb, hz, partDetail, partDrive, partFreq, partMotion, partStateWord, partValue, PART_KIND_WORDS } from './mixedWords';

	interface Props {
		mixed: MixedStore;
		compact?: boolean;
	}
	let { mixed, compact = false }: Props = $props();

	const parts = $derived(mixed.parts);
	const held = $derived(!!mixed.now?.held);
	const ctx = $derived({ rows: parts, config: mixed.runConfig, parts_s: mixed.now?.parts_s ?? null });
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
						{@const detail = partDetail(p, ctx)}
						{@const idle = partStateWord(p, ctx)}
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
							<td class="aq-small">{partMotion(p, held, ctx)}{#if detail}<span class="sub"><br />{detail}</span>{/if}</td>
							<td class="num">{hz(partFreq(p, ctx))}</td>
							<td class="num">{partValue(p.target, p.unit)}</td>
							<td class="num">{partValue(p.measured, p.unit)}</td>
							<td class="num" class:warn={p.alarm}>{errDb(p.error_db)}</td>
							{#if !compact}<td class="num">{partDrive(p)}</td>{/if}
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
									<span class="mark" aria-hidden="true"><svg viewBox="0 0 12 12" width="12" height="12"><circle cx="6" cy="6" r="4.2" fill="none" stroke="currentColor" stroke-width="1.2" /></svg></span>{idle ?? 'Off'}
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
