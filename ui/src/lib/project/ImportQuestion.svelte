<script lang="ts">
	/**
	 * What an import asks before it reads a file (`project.import`'s `ask`): the service's question and
	 * one whole number, filled with the value that leaves the file as it is. The import button sends
	 * the import again with the number; Cancel skips the file.
	 */
	import type { ProjectState } from '$lib/app/state.svelte';

	interface Props {
		project: ProjectState;
	}
	let { project }: Props = $props();

	const q = $derived(project.question);
	let typed = $state('');
	let shown: unknown = null;
	$effect.pre(() => {
		if (q && q !== shown) {
			shown = q;
			typed = String(q.ask.value);
		}
	});
	const value = $derived(/^\s*\d+\s*$/.test(typed) ? Number(typed) : null);
	const usable = $derived(value !== null && value > 0);

	function focusAndSelect(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	function submit(e: SubmitEvent) {
		e.preventDefault();
		if (usable && value !== null) project.answer(value);
	}
</script>

{#if q}
	<form class="question" aria-labelledby="import-question-title" onsubmit={submit} data-testid="import-question">
		<h3 id="import-question-title" class="eyebrow">{q.ask.title}: {q.file}</h3>
		<p data-testid="import-question-text">{q.ask.question}</p>
		<label>
			<span>{q.ask.label}</span>
			<input
				class="field mono"
				inputmode="numeric"
				data-testid="import-question-value"
				bind:value={typed}
				use:focusAndSelect
				aria-invalid={!usable}
				onkeydown={(e) => e.key === 'Escape' && project.answer(null)}
			/>
		</label>
		{#if !usable}<p class="why" data-testid="import-question-why">A node is a positive whole number.</p>{/if}
		<div class="row">
			<button type="button" class="btn ghost sm" data-testid="import-question-cancel" onclick={() => project.answer(null)}>Cancel</button>
			<button type="submit" class="btn primary sm" data-testid="import-question-answer" disabled={!usable}>{q.ask.answer}</button>
		</div>
	</form>
{/if}

<style>
	.question {
		margin: var(--sp-2) 0;
		padding: var(--sp-3);
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
	}
	h3 {
		margin: 0 0 var(--sp-1);
	}
	p {
		margin: 0 0 var(--sp-2);
		color: var(--text-2);
		font-size: var(--text-sm);
	}
	label {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		font-size: var(--text-sm);
	}
	label .field {
		width: 8em;
	}
	.why {
		margin: var(--sp-1) 0 0;
		color: var(--amber-2);
		font-size: var(--text-xs);
	}
	.row {
		display: flex;
		justify-content: flex-end;
		gap: var(--sp-2);
		margin-top: var(--sp-2);
	}
</style>
