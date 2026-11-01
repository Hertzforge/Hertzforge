<script lang="ts">
	/**
	 * Where control data comes from, impossible to miss. LIVE only when the service says the data
	 * is from hardware and the station is physical NI devices; the simulator and NI's
	 * driver-simulated devices are SIM, named as such. LIVE is solid with a dot; SIM is hatched.
	 */
	import type { DataSource, StationClass } from '$lib/protocol/types';
	import SimBadge from './SimBadge.svelte';

	interface Props {
		source: DataSource;
		cls: StationClass;
		size?: 'md' | 'lg';
		compact?: boolean;
	}
	let { source, cls, size = 'md', compact = false }: Props = $props();

	const live = $derived(source === 'HARDWARE' && cls === 'ni_real');
	const simText = $derived(
		cls === 'ni_simulated' ? (compact ? 'NI simulated' : 'Driver-simulated NI devices, not hardware') : compact ? 'Simulated' : 'Simulator, not hardware'
	);
</script>

{#if live}
	<span class="live {size}" data-testid="live-badge" role="status" aria-label="Live: measured on hardware.">
		<span class="tag">
			<svg viewBox="0 0 10 10" width="8" height="8" aria-hidden="true"><circle cx="5" cy="5" r="4" fill="currentColor" /></svg>
			LIVE
		</span>
		<span class="text">{compact ? 'Hardware' : 'Measured on NI hardware'}</span>
	</span>
{:else}
	<SimBadge {size} {compact} text={simText} />
{/if}

<style>
	.live {
		flex: none;
		display: inline-flex;
		align-items: stretch;
		height: 26px;
		overflow: hidden;
		border: 1.5px solid var(--blue-2);
		border-radius: var(--radius-md);
		font-size: var(--text-xs);
		white-space: nowrap;
	}

	.tag {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		padding: 0 9px;
		font-family: var(--font-mono);
		font-weight: 800;
		letter-spacing: 0.14em;
		color: var(--text-on-accent);
		background: var(--blue-2);
	}

	.text {
		display: inline-flex;
		align-items: center;
		padding: 0 10px;
		font-weight: var(--weight-medium);
		color: var(--blue-1);
		background: var(--blue-weak);
	}

	.lg {
		height: 30px;
		font-size: var(--text-sm);
	}
</style>
