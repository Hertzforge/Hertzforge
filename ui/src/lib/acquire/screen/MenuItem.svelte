<script lang="ts">
	/** One item of a screen menu: a plain item, or one that is checked (a tick and its weight show it). */
	import './screen.css';

	interface Props {
		label: string;
		/** `check` toggles; `radio` is one of a set. */
		kind?: 'item' | 'check' | 'radio';
		checked?: boolean;
		disabled?: boolean;
		/** Why the item cannot be chosen now. */
		reason?: string | null;
		/** A key that does the same, shown at the right. */
		keys?: string;
		danger?: boolean;
		testid?: string;
		onclick: () => void;
	}
	let { label, kind = 'item', checked = false, disabled = false, reason = null, keys, danger = false, testid, onclick }: Props = $props();
</script>

<button
	type="button"
	class="ws-mi"
	class:ws-danger={danger}
	role={kind === 'check' ? 'menuitemcheckbox' : kind === 'radio' ? 'menuitemradio' : 'menuitem'}
	aria-checked={kind === 'item' ? undefined : checked}
	aria-disabled={disabled}
	title={disabled && reason ? reason : undefined}
	data-testid={testid}
	onclick={() => {
		if (!disabled) onclick();
	}}
>
	{#if kind !== 'item'}
		<span class="ws-check" aria-hidden="true">
			{#if checked}
				{#if kind === 'radio'}
					<svg viewBox="0 0 12 12" width="10" height="10"><circle cx="6" cy="6" r="3.5" fill="currentColor" /></svg>
				{:else}
					<svg viewBox="0 0 12 12" width="12" height="12"><path d="M2.2 6.4l2.4 2.4 5.2-5.6" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" /></svg>
				{/if}
			{/if}
		</span>
	{/if}
	<span>{label}</span>
	{#if keys}<span class="ws-key">{keys}</span>{/if}
</button>
