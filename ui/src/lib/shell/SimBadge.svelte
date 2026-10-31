<script lang="ts">
	/** Shown whenever simulated control data is on screen. Hatched, labeled, and impossible to miss. */
	interface Props {
		size?: 'md' | 'lg';
		/** Short wording for tight spaces such as the top bar. */
		compact?: boolean;
	}
	let { size = 'md', compact = false }: Props = $props();
</script>

<span class="sim {size}" data-testid="sim-badge" role="status" aria-label="Simulated data. Not a hardware result.">
	<span class="tag">SIM</span>
	<span class="text">{compact ? 'Simulated' : 'Simulated data, not hardware'}</span>
</span>

<style>
	.sim {
		flex: none;
		display: inline-flex;
		align-items: stretch;
		height: 26px;
		overflow: hidden;
		border: 1.5px solid var(--amber-2);
		border-radius: var(--radius-md);
		font-size: var(--text-xs);
		white-space: nowrap;
	}

	.tag {
		display: inline-flex;
		align-items: center;
		padding: 0 9px;
		font-family: var(--font-mono);
		font-weight: 800;
		letter-spacing: 0.14em;
		color: var(--status-fail-text);
		background:
			repeating-linear-gradient(-45deg, transparent 0 5px, var(--sim-hatch) 5px 8px),
			var(--amber-2);
	}

	.text {
		display: inline-flex;
		align-items: center;
		padding: 0 10px;
		font-weight: var(--weight-medium);
		color: var(--amber-2);
		background: var(--amber-weak);
	}

	.lg {
		height: 30px;
		font-size: var(--text-sm);
	}
</style>
