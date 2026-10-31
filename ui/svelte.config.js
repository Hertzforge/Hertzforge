import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		// A single-page app: the same build is served to a browser or wrapped by a desktop shell.
		adapter: adapter({ fallback: 'index.html', strict: false })
	}
};

export default config;
