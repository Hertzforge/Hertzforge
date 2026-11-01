<script lang="ts">
	/**
	 * The test-type chooser, where Acquire & Control opens: one card per test type, with what the
	 * test is in a line. A type still to come says so in words, keeps its place in the keyboard
	 * order, and cannot be opened. While a run is live, its card says so and returns to it.
	 */
	import { onMount } from 'svelte';
	import type { TestTypeId, TestTypeView } from './testTypes.svelte';

	interface Props {
		tests: TestTypeView;
		/** The live run's state in words, for its card, or null. */
		liveWord: string | null;
	}
	let { tests, liveWord }: Props = $props();

	let list: HTMLElement;

	// Back from a test type's screen, the keyboard lands on the card it came from.
	onMount(() => {
		if (!tests.takeFocus()) return;
		const id = tests.last ?? tests.types.find((t) => t.ready)?.id;
		list.querySelector<HTMLButtonElement>(`[data-type="${id}"]`)?.focus();
	});

	/** Arrow keys, Home and End move between the cards, as on the workspace rail. */
	function onKey(e: KeyboardEvent) {
		const step: Record<string, number> = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 };
		if (!(e.key in step) && e.key !== 'Home' && e.key !== 'End') return;
		const cards = [...list.querySelectorAll<HTMLButtonElement>('button.card')];
		const i = cards.indexOf(document.activeElement as HTMLButtonElement);
		if (i < 0) return;
		e.preventDefault();
		const n = e.key === 'Home' ? 0 : e.key === 'End' ? cards.length - 1 : (i + step[e.key] + cards.length) % cards.length;
		cards[n].focus();
	}

	function pick(id: TestTypeId) {
		tests.choose(id);
	}
</script>

<section class="chooser" data-testid="test-type-chooser" aria-labelledby="tt-title">
	<h1 id="tt-title">Choose a Test Type</h1>

	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<ul class="cards" bind:this={list} onkeydown={onKey}>
		{#each tests.types as t (t.id)}
			{@const live = tests.live === t.id}
			{@const why = tests.whyNot(t.id)}
			{@const state = live ? 'live' : !t.ready ? 'coming' : why ? 'waiting' : 'ready'}
			<li>
				<button
					class="card {state}"
					data-type={t.id}
					data-testid="test-type-{t.id}"
					data-state={state}
					aria-disabled={why ? 'true' : 'false'}
					aria-labelledby="tt-{t.id}-name tt-{t.id}-tag"
					aria-describedby="tt-{t.id}-sum"
					onclick={() => pick(t.id)}
				>
					<span class="glyph" aria-hidden="true">
						<svg viewBox="0 0 44 28" width="44" height="28">
							{#if t.id === 'random'}
								<path d="M2 14l2.5-5 2.5 9 2.5-12 2.5 14 2.5-9 2.5 5 2.5-12 2.5 18 2.5-13 2.5 8 2.5-9 2.5 7 2.5-4 2.5 6 2.5-5 2.5 2" />
							{:else if t.id === 'sine'}
								<path d="M2 14c2.7-10.7 5.3-10.7 8 0s5.3 10.7 8 0 5.3-10.7 8 0 5.3 10.7 8 0 5.3-10.7 8 0" />
							{:else if t.id === 'shock'}
								<path d="M2 22h12c2.5 0 3.5-17 8-17s5.5 17 8 17h12" />
							{:else if t.id === 'srs'}
								<path d="M3 24L17 9h24" />
								<circle cx="17" cy="9" r="2" class="fill" />
								<circle cx="3" cy="24" r="2" class="fill" />
								<circle cx="41" cy="9" r="2" class="fill" />
							{:else if t.id === 'twr'}
								<path d="M2 14q2-3 4 0t4 0q2-8 4 0t4 0q2-11 4 0t4 0q2-6 4 0t4 0q2-4 4 0t4 0" />
							{:else if t.id === 'recorder'}
								<circle cx="8" cy="14" r="5" class="fill" />
								<path d="M17 14l2.5-3 2.5 5 2.5-4 2.5 3 2.5-2 2.5 1h7" />
							{:else}
								<path d="M2 14c2.7-10.7 5.3-10.7 8 0s5.3 10.7 8 0 5.3-10.7 8 0 5.3 10.7 8 0 5.3-10.7 8 0" class="faint" />
								<path d="M30 2v24" />
								<circle cx="30" cy="14" r="3.2" class="fill" />
							{/if}
						</svg>
					</span>
					<span class="head">
						<span class="name" id="tt-{t.id}-name">{t.title}</span>
						<span class="tag" id="tt-{t.id}-tag">
							{#if live}
								<svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg>
								{liveWord ?? 'Live'}
							{:else if state === 'coming'}
								Coming
							{:else if state === 'ready' && tests.last === t.id}
								Last used
							{/if}
						</span>
						{#if state === 'ready' || state === 'live'}
							<svg class="go" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true"><path d="M4.5 2.5L8 6l-3.5 3.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" /></svg>
						{/if}
					</span>
					<span class="sum" id="tt-{t.id}-sum">
						{t.summary}{#if live}{' '}<span class="foot">Return to the run.</span>{:else if state === 'waiting'}{' '}<span class="foot">{why}</span>{/if}
					</span>
				</button>
			</li>
		{/each}
	</ul>
</section>

<style>
	.chooser {
		height: 100%;
		overflow-y: auto;
		padding: var(--sp-5) var(--sp-5) var(--sp-6);
	}

	h1 {
		margin-bottom: var(--sp-4);
		font-size: var(--text-xl);
		font-weight: var(--weight-strong);
		letter-spacing: -0.01em;
	}

	.cards {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
		gap: var(--sp-3);
		max-width: 1180px;
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.cards li {
		display: flex;
		min-width: 0;
	}

	.card {
		display: flex;
		flex: 1;
		flex-direction: column;
		align-items: stretch;
		gap: var(--sp-2);
		min-width: 0;
		padding: var(--sp-4);
		text-align: left;
		color: var(--text-1);
		background: var(--bg-1);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-lg);
		transition:
			background var(--dur-fast) var(--ease),
			border-color var(--dur-fast) var(--ease);
	}

	.card.ready:hover,
	.card.live:hover {
		background: var(--bg-2);
		border-color: var(--line-3);
	}

	.card:focus-visible {
		border-radius: var(--radius-lg);
	}

	/* A type still to come: dashed edge, quieter text, the word "Coming", no arrow. */
	.card.coming,
	.card.waiting {
		cursor: default;
		color: var(--text-3);
		background: transparent;
		border-style: dashed;
	}

	/* The live run's card: a heavier edge and the pulsing dot beside its state word. */
	.card.live {
		border-color: var(--blue-line);
		border-width: 2px;
		padding: calc(var(--sp-4) - 1px);
	}

	.glyph {
		display: inline-grid;
		place-items: center;
		width: 56px;
		height: 38px;
		color: var(--blue-2);
		background: var(--blue-weak);
		border: 1px solid var(--blue-line);
		border-radius: var(--radius-md);
	}

	.glyph svg {
		fill: none;
		stroke: currentColor;
		stroke-width: 1.6;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.glyph .fill {
		fill: currentColor;
		stroke: none;
	}

	.glyph .faint {
		opacity: 0.45;
	}

	.coming .glyph,
	.waiting .glyph {
		color: var(--text-4);
		background: var(--bg-2);
		border: 1px dashed var(--line-2);
	}

	.head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}

	.name {
		font-size: var(--text-lg);
		font-weight: var(--weight-strong);
	}

	.coming .name,
	.waiting .name {
		color: var(--text-2);
	}

	.tag {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		font-size: var(--text-2xs);
		font-weight: var(--weight-strong);
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--text-3);
		white-space: nowrap;
	}

	.tag:empty {
		display: none;
	}

	.coming .tag,
	.waiting .tag {
		padding: 1px 7px;
		color: var(--text-2);
		border: 1px dashed var(--line-3);
		border-radius: 10px;
	}

	.live .tag {
		color: var(--blue-1);
	}

	.live .tag svg {
		animation: pulse 1.2s ease-in-out infinite;
	}

	.go {
		margin-left: auto;
		color: var(--text-3);
		flex-shrink: 0;
	}

	.sum {
		font-size: var(--text-sm);
		line-height: 1.45;
		color: var(--text-2);
	}

	.coming .sum,
	.waiting .sum {
		color: var(--text-3);
	}

	.foot {
		display: block;
		margin-top: var(--sp-1);
		font-weight: var(--weight-strong);
		color: var(--text-1);
	}

	@keyframes pulse {
		50% {
			opacity: 0.35;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.live .tag svg {
			animation: none;
		}
	}

	@media (max-width: 760px) {
		.chooser {
			padding: var(--sp-4) var(--sp-3) var(--sp-5);
		}

		.cards {
			grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
		}
	}
</style>
