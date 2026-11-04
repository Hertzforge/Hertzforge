<script lang="ts">
	/**
	 * What the service said no to, by its reason, with its own words verbatim and the facts behind
	 * it; and short notes about what just happened (control resumed, a table applied).
	 */
	import Icon from '$lib/shell/Icon.svelte';
	import type { NoticesHost } from './actions';

	interface Props {
		acq: NoticesHost;
	}
	let { acq }: Props = $props();

	const r = $derived(acq.refusal);
	const n = $derived(acq.note);
	const lost = $derived(!acq.connected && acq.run !== null && acq.isLive);
</script>

{#if lost}
	<div class="aq-notice warn" role="alert" data-testid="connection-lost">
		<svg viewBox="0 0 14 14" width="14" height="14" aria-hidden="true"><path d="M7 1.6l5.6 10H1.4z" fill="none" stroke="var(--amber-2)" stroke-width="1.4" stroke-linejoin="round" /><path d="M7 5.4v3" stroke="var(--amber-2)" stroke-width="1.5" stroke-linecap="round" /></svg>
		<div class="grow">
			<strong>The connection to the control service is down.</strong>
			The run continues on the service under its own protection. This screen reconnects by itself and takes control back if it held it.
		</div>
	</div>
{:else if !acq.connected}
	<div class="aq-notice" role="status">
		<div class="grow">The control service is not connected ({acq.control.url}). {acq.control.detail ?? ''}</div>
	</div>
{/if}

{#if acq.syncError}
	<div class="aq-notice warn" role="alert">
		<div class="grow"><strong>The control service did not answer the station and run requests.</strong> {acq.syncError}</div>
	</div>
{/if}

{#if r}
	<div class="aq-notice warn refusal" role="alert" data-testid="refusal" data-reason={r.reason ?? r.code}>
		<svg viewBox="0 0 14 14" width="16" height="16" aria-hidden="true"><path d="M4.5 1h5L13 4.5v5L9.5 13h-5L1 9.5v-5z" fill="var(--status-fail-fill)" /><path d="M4.8 7h4.4" stroke="var(--status-fail-text)" stroke-width="1.7" stroke-linecap="round" /></svg>
		<div class="grow">
			<p class="title"><strong>{r.title}</strong> <span class="verb">{r.verb} was not done.</span></p>
			<p class="msg" data-testid="refusal-message">{r.message}</p>
			{#if r.facts.length}
				<ul class="facts">
					{#each r.facts as f (f)}<li>{f}</li>{/each}
				</ul>
			{/if}
		</div>
		<button class="btn ghost icon sm" onclick={() => acq.dismissRefusal()} aria-label="Dismiss"><Icon name="close" size={14} /></button>
	</div>
{/if}

{#if n}
	<div class="aq-notice {n.tone}" role="status" data-testid="note">
		<div class="grow">{n.text}</div>
		<button class="btn ghost icon sm" onclick={() => acq.dismissNote()} aria-label="Dismiss"><Icon name="close" size={14} /></button>
	</div>
{/if}

<style>
	.refusal {
		border-width: 1.5px;
		border-color: var(--amber-2);
	}

	.title {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 4px 10px;
	}

	.verb {
		font-size: var(--text-xs);
		color: var(--text-3);
	}

	.msg {
		margin-top: 2px;
		color: var(--text-1);
	}

	.facts {
		margin: 4px 0 0;
		padding-left: 18px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}
</style>
