<script lang="ts">
	/**
	 * The way to Configure, at the upper right of every Acquire & Control screen: the station every
	 * test type runs on is set there, once, and the test types take it from there.
	 */
	interface Props {
		onclick: () => void;
	}
	let { onclick }: Props = $props();
</script>

<button class="btn ghost sm configure" data-testid="configure-open" title="Set the station every test type runs on: the simulator and its models, or NI-DAQmx devices." {onclick}>
	<svg viewBox="0 0 16 16" width="13" height="13" aria-hidden="true">
		<path d="M2 4h7M12 4h2M2 8h2M7 8h7M2 12h5M10 12h4" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
		<circle cx="10.5" cy="4" r="1.6" fill="none" stroke="currentColor" stroke-width="1.4" />
		<circle cx="5.5" cy="8" r="1.6" fill="none" stroke="currentColor" stroke-width="1.4" />
		<circle cx="8.5" cy="12" r="1.6" fill="none" stroke="currentColor" stroke-width="1.4" />
	</svg>
	Configure
</button>

<style>
	.configure {
		gap: 5px;
		flex: none;
	}
</style>
