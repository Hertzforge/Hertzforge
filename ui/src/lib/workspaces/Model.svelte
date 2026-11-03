<script lang="ts">
	/**
	 * Model: the steps between importing a mesh and solving it, as Visual Dynamics' Model screen takes
	 * them: view the geometry, say what its element groups are made of, merge coincident nodes, and
	 * Solve Modes. The tools run on the project, as Analyze's do, and their results are Analyze's.
	 */
	import type { AppState } from '$lib/app/state.svelte';
	import Analyze from './Analyze.svelte';

	interface Props {
		app: AppState;
	}
	let { app }: Props = $props();
</script>

<Analyze {app} only="model" />
