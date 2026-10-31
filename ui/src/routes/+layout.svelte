<script lang="ts">
	import '@fontsource-variable/atkinson-hyperlegible-next';
	import '@fontsource-variable/atkinson-hyperlegible-mono';
	import '$lib/theme/tokens.css';
	import '$lib/theme/base.css';

	let { children } = $props();
</script>

{@render children()}
