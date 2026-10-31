<script lang="ts">
	/**
	 * Simulate: run the control loop against the simulated system and watch `random.display`.
	 * Everything here is SIM data and says so. A stop the shell did not ask for is shown as an
	 * abort, with the reason the service gave.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import ParamForm from '$lib/forms/ParamForm.svelte';
	import Icon from '$lib/shell/Icon.svelte';
	import SimBadge from '$lib/shell/SimBadge.svelte';
	import { formatFixed } from '$lib/viz/format';
	import ViewRenderer from '$lib/viz/ViewRenderer.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();

	const s = $derived(app.sim);
	const spectrum = $derived(s.display?.views.find((v) => v.view === 'spectrum') ?? null);
	const reading = $derived(s.display?.views.find((v) => v.view === 'reading') ?? null);
	const stateWord = $derived(s.runState === 'running' ? 'Running' : s.runState === 'stopped' ? (s.abort ? 'Aborted' : 'Stopped') : 'Idle');
</script>

<div class="simulate">
	<div class="bar">
		<div class="form">
			<ParamForm params={s.params} bind:values={s.values} layout="row" disabled={s.runState === 'running' || !app.control.isOpen} idPrefix="sim" testidPrefix="sim" />
		</div>
		<div class="actions">
			<button class="btn primary start" data-testid="sim-start" onclick={() => s.start()} disabled={!s.canStart}>
				<Icon name="play" size={13} /> Start
			</button>
			<button class="btn stop" data-testid="sim-stop" onclick={() => s.stop()} disabled={!s.canStop}>
				<Icon name="stop" size={12} /> Stop
			</button>
		</div>
		<div class="status">
			<span class="chip {s.runState}" class:aborted={!!s.abort} data-testid="sim-state" data-state={s.abort ? 'aborted' : s.runState} role="status" aria-label="Run state: {stateWord}">
				<svg viewBox="0 0 12 12" width="11" height="11" aria-hidden="true">
					{#if s.runState === 'running'}
						<circle cx="6" cy="6" r="4.3" fill="currentColor" />
					{:else if s.runState === 'stopped'}
						<rect x="2" y="2" width="8" height="8" rx="1.2" fill="currentColor" />
					{:else}
						<circle cx="6" cy="6" r="4" fill="none" stroke="currentColor" stroke-width="1.5" />
					{/if}
				</svg>
				{stateWord}
			</span>
			<dl class="live num">
				<div><dt>Elapsed</dt><dd>{s.display ? `${formatFixed(s.display.elapsed_s, 3)} s` : '-'}</dd></div>
				<div><dt>Level</dt><dd>{s.display ? `${s.display.level_db >= 0 ? '+' : ''}${s.display.level_db.toFixed(1)} dB` : '-'}</dd></div>
				<div><dt>Run</dt><dd>{s.runId ?? '-'}</dd></div>
			</dl>
			{#if s.display?.source === 'SIM'}<SimBadge size="lg" />{/if}
		</div>
	</div>

	{#if s.profile}<p class="profile">{s.profile.summary}</p>{/if}

	{#if !app.control.isOpen}
		<div class="notice" role="status">
			The control service is not connected ({app.control.url}). {app.control.detail ?? ''}
		</div>
	{/if}
	{#if s.error}<div class="notice warn" role="alert">{s.error}</div>{/if}
	{#if s.profilesError}<div class="notice warn" role="alert">{s.profilesError}</div>{/if}

	{#if s.abort}
		<div class="abort" data-testid="sim-abort" role="alert">
			<svg viewBox="0 0 14 14" width="18" height="18" aria-hidden="true">
				<path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="var(--status-fail-fill)" />
				<path d="M5 5l4 4M9 5l-4 4" stroke="var(--status-fail-text)" stroke-width="1.6" stroke-linecap="round" />
			</svg>
			<div>
				<strong>Run Aborted at {formatFixed(s.abort.atS, 3)} s</strong>
				<p>{s.abort.reason}</p>
			</div>
			<button class="btn ghost icon sm" onclick={() => (s.abort = null)} aria-label="Dismiss">
				<Icon name="close" size={14} />
			</button>
		</div>
	{/if}

	<div class="stage">
		<section class="chart" aria-label="Live spectrum">
			{#if spectrum}
				<ViewRenderer view={spectrum} live minHeight={260} />
			{:else}
				<div class="empty">
					<p>{s.runState === 'running' ? 'Waiting for the first display frame.' : 'Start a simulation to see control against reference, with tolerance and abort lines.'}</p>
				</div>
			{/if}
		</section>
		<aside class="readings" aria-label="Readings">
			{#if reading}
				<ViewRenderer view={reading} layout="list" />
			{:else}
				<h3 class="rtitle">Control Readings</h3>
				<p class="muted">Readings appear once a run is streaming.</p>
			{/if}
		</aside>
	</div>
</div>

<style>
	.simulate {
		display: flex;
		flex-direction: column;
		height: 100%;
		min-height: 0;
		padding: var(--sp-4) var(--sp-5) var(--sp-5);
		gap: var(--sp-3);
	}

	.bar {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: start;
		column-gap: var(--sp-5);
		padding: var(--sp-3) var(--sp-4) 0;
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.form {
		min-width: 0;
	}

	.form :global(.param:first-child) {
		min-width: 230px;
	}

	.actions {
		display: flex;
		gap: var(--sp-2);
		padding-top: 18px;
	}

	.start {
		min-width: 88px;
	}

	.stop:not(:disabled) {
		color: var(--amber-1);
		border-color: var(--amber-line);
		background: var(--amber-weak);
	}

	.status {
		grid-column: 1 / -1;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--sp-3) var(--sp-5);
		margin: 2px calc(-1 * var(--sp-4)) 0;
		padding: 10px var(--sp-4);
		background: var(--bg-2);
		border-top: 1px solid var(--line-1);
		border-radius: 0 0 var(--radius-lg) var(--radius-lg);
	}

	.chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: 26px;
		min-width: 92px;
		padding: 0 10px;
		font-size: var(--text-sm);
		font-weight: var(--weight-medium);
		color: var(--text-2);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: 13px;
	}

	.chip.running {
		color: var(--blue-1);
		border-color: var(--blue-line);
		background: var(--blue-weak);
	}

	.chip.running svg {
		animation: pulse 1.2s ease-in-out infinite;
	}

	.chip.aborted {
		color: var(--status-fail-text);
		background: var(--status-fail-fill);
		border-color: var(--status-fail-fill);
	}

	.live {
		display: flex;
		gap: var(--sp-5);
		margin: 0 auto 0 0;
	}

	.live div {
		display: flex;
		align-items: baseline;
		gap: 6px;
	}

	.live dt {
		font-family: var(--font-ui);
		font-size: var(--text-2xs);
		color: var(--text-3);
	}

	.live dd {
		margin: 0;
		min-width: 8ch;
		font-size: var(--text-sm);
		color: var(--text-1);
	}

	.profile {
		margin-top: -4px;
		padding-left: var(--sp-1);
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.notice {
		padding: 8px 12px;
		font-size: var(--text-sm);
		color: var(--text-2);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}

	.notice.warn {
		color: var(--amber-2);
		border-color: var(--amber-line);
		background: var(--amber-weak);
	}

	.abort {
		display: flex;
		align-items: flex-start;
		gap: var(--sp-3);
		padding: 10px 12px;
		background: var(--amber-weak);
		border: 1.5px solid var(--amber-2);
		border-radius: var(--radius-md);
	}

	.abort div {
		flex: 1;
	}

	.abort strong {
		font-size: var(--text-md);
		color: var(--text-1);
	}

	.abort p {
		margin-top: 2px;
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.stage {
		flex: 1;
		display: grid;
		grid-template-columns: minmax(0, 1fr) 340px;
		gap: var(--sp-3);
		min-height: 0;
	}

	.chart,
	.readings {
		min-height: 0;
		padding: var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-1);
		border-radius: var(--radius-lg);
	}

	.readings {
		overflow-y: auto;
	}

	.rtitle {
		margin-bottom: var(--sp-2);
		font-size: var(--text-sm);
		font-weight: var(--weight-strong);
	}

	.muted {
		color: var(--text-3);
		font-size: var(--text-sm);
	}

	.empty {
		display: grid;
		place-items: center;
		height: 100%;
		padding: var(--sp-6);
		color: var(--text-3);
		text-align: center;
		background: var(--bg-plot);
		border: 1px dashed var(--line-2);
		border-radius: var(--radius-md);
	}

	@keyframes pulse {
		50% {
			opacity: 0.45;
		}
	}

	@media (max-width: 1100px) {
		.stage {
			grid-template-columns: 1fr;
			grid-template-rows: minmax(320px, 1fr) auto;
		}
	}

	/* A narrow pane stacks the form above Start and Stop instead of letting the profile
	   picker's minimum width run under the buttons. */
	@media (max-width: 760px) {
		.bar {
			grid-template-columns: 1fr;
		}
		.form :global(.param:first-child) {
			min-width: 0;
		}
		.actions {
			padding-top: var(--sp-3);
		}
	}
</style>
