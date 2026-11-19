<script lang="ts">
	/** The run's control law in one line: its name and version, and its unusable calls in a row when there are any. */
	interface Props {
		law: { name: string; version: string; misses: number } | null | undefined;
		testid: string;
	}
	let { law, testid }: Props = $props();
</script>

{#if law}
	<p class="law aq-small" data-testid={testid}>{law.name} <span class="v">{law.version}</span>{#if law.misses > 0}<span class="miss">, {law.misses} {law.misses === 1 ? 'miss' : 'misses'} in a row</span>{/if}</p>
{/if}

<style>
	.law {
		margin: 0 0 var(--sp-2);
		color: var(--text-1);
	}

	.v {
		color: var(--text-3);
	}

	.miss {
		font-weight: var(--weight-strong);
		color: var(--amber-1);
	}
</style>
