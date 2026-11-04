<script lang="ts">
	/**
	 * The console along the bottom edge of the views, as Visual Dynamics' is: a tab that opens and
	 * closes it, and the session as Python inside, read-only, selectable and copyable. A tab, not
	 * a menu, so the console shows that it exists; read-only by decision, because the journal's
	 * value is copying out.
	 */
	import type { ConsoleState } from '$lib/app/state.svelte';

	interface Props {
		console: ConsoleState;
	}
	let { console: c }: Props = $props();

	let copied = $state(false);
	async function copy() {
		try {
			await navigator.clipboard.writeText(c.text);
			copied = true;
			setTimeout(() => (copied = false), 1500);
		} catch {
			// the page selects the text instead
		}
	}
</script>

<div class="console" class:open={c.shown} data-testid="console">
	<div class="tabbar">
		<button class="tab" data-testid="console-tab" aria-expanded={c.shown} onclick={() => c.toggle()} title="The session as Python: every act as the line that replays it, to copy into a script">
			Console{#if c.shown && c.lines.length}<span class="num"> {c.lines.length}</span>{/if}
		</button>
		{#if c.shown}
			<button class="btn ghost sm" data-testid="console-copy" onclick={copy} disabled={!c.lines.length}>{copied ? 'Copied' : 'Copy'}</button>
		{/if}
	</div>
	{#if c.shown}
		<pre class="scrollback" data-testid="console-text">{#if c.error}{c.error}{:else}{c.text}{/if}</pre>
	{/if}
</div>

<style>
	.console {
		flex: 0 0 auto;
		display: flex;
		flex-direction: column;
		border-top: 1px solid var(--line-1);
		background: var(--bg-1);
	}

	.tabbar {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		padding: 0 var(--sp-3);
	}

	.tab {
		padding: 3px 10px;
		font-size: var(--text-xs);
		color: var(--text-3);
		background: none;
		border: 0;
		border-bottom: 2px solid transparent;
		cursor: pointer;
	}

	.tab:hover,
	.open .tab {
		color: var(--text-1);
	}

	.open .tab {
		border-bottom-color: var(--accent, var(--text-1));
	}

	.scrollback {
		height: 180px;
		margin: 0;
		padding: var(--sp-2) var(--sp-3);
		overflow: auto;
		font-family: var(--font-mono);
		font-size: var(--text-xs);
		line-height: 1.5;
		color: var(--text-2);
		background: var(--bg-plot);
		border-top: 1px solid var(--line-1);
		user-select: text;
		white-space: pre;
	}
</style>
