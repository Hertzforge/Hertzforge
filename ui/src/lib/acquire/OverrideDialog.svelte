<script lang="ts">
	/**
	 * Arm past a feasibility exceedance. The acknowledgement is the operator's own words; the
	 * service logs it verbatim and keeps it with the run. Nothing is armed without it.
	 */
	import { tick } from 'svelte';
	import type { AcquireState } from './store.svelte';

	/** Another acknowledgement than arming past the shaker ratings: its words and what it sends. */
	interface OverrideVariant {
		title: string;
		lead: string;
		verb: string;
		confirm: string;
		send: (text: string) => Promise<boolean>;
		refusal: string | null;
		busy: boolean;
	}

	interface Props {
		acq: AcquireState;
		open: boolean;
		variant?: OverrideVariant;
	}
	let { acq, open = $bindable(), variant }: Props = $props();

	let text = $state('');
	let input = $state<HTMLTextAreaElement | null>(null);
	let restore: HTMLElement | null = null;

	$effect(() => {
		if (open) {
			restore = document.activeElement as HTMLElement | null;
			text = '';
			void tick().then(() => input?.focus());
		}
	});

	function close() {
		open = false;
		restore?.focus?.();
	}

	async function arm() {
		if (!text.trim()) return;
		const ok = variant ? await variant.send(text.trim()) : await acq.arm(text.trim());
		if (ok) close();
	}

	function onKey(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			close();
		} else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
			e.preventDefault();
			void arm();
		}
	}
</script>

{#if open}
	<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
	<div class="scrim" onclick={close}></div>
	<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="ovr-title" data-testid="override-dialog" tabindex="-1" onkeydown={onKey}>
		<h2 id="ovr-title">{variant?.title ?? 'Arm Past the Shaker Ratings'}</h2>
		<p class="lead">{variant ? variant.lead : (acq.run?.feasibility?.message ?? 'The profile exceeds the shaker ratings that were given.')}</p>
		<p class="lead">{variant?.verb ?? 'Arming'} anyway is your decision. Say why in your own words; it is logged with the run, as written.</p>
		<label for="ovr-text" class="lbl">Acknowledgement</label>
		<textarea id="ovr-text" class="field" rows="3" bind:this={input} bind:value={text} data-testid="override-text" placeholder="For example: ratings reviewed with the test lead; the article is a mass simulator."></textarea>
		{#if variant?.refusal}
			<p class="err" role="alert">{variant.refusal}</p>
		{:else if !variant && acq.refusal && acq.busy === null && acq.refusal.verb === 'Arm Past Feasibility'}
			<p class="err" role="alert">{acq.refusal.title}: {acq.refusal.message}</p>
		{/if}
		<div class="row">
			<button class="btn ghost" onclick={close}>Cancel</button>
			<button class="btn primary" data-testid="override-arm" disabled={!text.trim() || (variant ? variant.busy : acq.busy !== null)} onclick={arm}>{variant?.confirm ?? 'Arm With This Acknowledgement'}</button>
		</div>
	</div>
{/if}

<style>
	.scrim {
		position: fixed;
		inset: 0;
		z-index: 40;
		background: var(--scrim);
	}

	.dialog {
		position: fixed;
		z-index: 41;
		top: 14vh;
		left: 50%;
		width: min(560px, calc(100vw - 32px));
		transform: translateX(-50%);
		padding: var(--sp-5);
		background: var(--bg-2);
		border: 1.5px solid var(--amber-2);
		border-radius: var(--radius-lg);
		box-shadow: var(--shadow-pop);
	}

	h2 {
		margin-bottom: var(--sp-2);
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
	}

	.lead {
		margin-bottom: var(--sp-2);
		font-size: var(--text-sm);
		color: var(--text-2);
	}

	.lbl {
		display: block;
		margin: var(--sp-3) 0 4px;
		font-size: var(--text-xs);
		color: var(--text-2);
	}

	textarea {
		height: auto;
		padding: 8px;
		resize: vertical;
		line-height: 1.4;
	}

	.err {
		margin-top: var(--sp-2);
		font-size: var(--text-xs);
		color: var(--amber-1);
	}

	.row {
		display: flex;
		justify-content: flex-end;
		gap: var(--sp-2);
		margin-top: var(--sp-4);
	}
</style>
