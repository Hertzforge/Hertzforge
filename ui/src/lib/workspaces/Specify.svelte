<script lang="ts">
	/**
	 * Specify: test specifications from measured data, as Visual Dynamics' Derive Specification
	 * writes them. The tools run on the project, as Analyze's do, and their results are Analyze's.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import Analyze from './Analyze.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();
</script>

<Analyze {app} only="specify" />
