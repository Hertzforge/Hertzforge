<script lang="ts">
	/** The update on its way in: a bar over the shell from the yes to the relaunch. Nothing else can be pressed meanwhile. */
	import { progressFraction, progressWords } from './updates';
	import { updating } from './updating.svelte';

	const fraction = $derived(updating.now ? progressFraction(updating.now) : null);
</script>

{#if updating.now}
	<div class="scrim" data-testid="update-progress">
		<div class="card" role="dialog" aria-modal="true" aria-label="Updating Hertzforge">
			<div class="title">Updating to Hertzforge {updating.now.version}</div>
			<div
				class="bar"
				class:unsized={fraction === null}
				role="progressbar"
				aria-valuemin="0"
				aria-valuemax="100"
				aria-valuenow={fraction === null ? undefined : Math.round(fraction * 100)}
			>
				<div class="fill" style:width={fraction === null ? undefined : `${fraction * 100}%`}></div>
			</div>
			<div class="words" aria-live="polite">
				<span>{progressWords(updating.now)}</span>
				{#if fraction !== null && updating.now.phase === 'downloading'}<span>{Math.round(fraction * 100)} %</span>{/if}
			</div>
		</div>
	</div>
{/if}

<style>
	.scrim {
		position: fixed;
		inset: 0;
		z-index: 1000;
		display: grid;
		place-items: center;
		background: var(--scrim);
	}
	.card {
		width: min(420px, calc(100vw - 2 * var(--sp-4)));
		padding: var(--sp-4);
		display: grid;
		gap: var(--sp-3);
		background: var(--bg-1);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-lg);
		box-shadow: var(--shadow-pop);
	}
	.title {
		font-size: var(--text-lg);
		color: var(--text-1);
	}
	.bar {
		height: 6px;
		border-radius: 3px;
		background: var(--bg-3);
		overflow: hidden;
	}
	.fill {
		height: 100%;
		border-radius: 3px;
		background: var(--blue-2);
		transition: width 120ms linear;
	}
	.unsized .fill {
		width: 30%;
		animation: sweep 1.2s ease-in-out infinite alternate;
	}
	.words {
		display: flex;
		justify-content: space-between;
		font-size: var(--text-xs);
		color: var(--text-3);
		font-variant-numeric: tabular-nums;
	}
	@keyframes sweep {
		from {
			transform: translateX(0);
		}
		to {
			transform: translateX(233%);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.unsized .fill {
			animation: none;
			width: 100%;
			opacity: 0.5;
		}
	}
</style>
