import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [sveltekit()],
	server: { host: '127.0.0.1', port: 5191, strictPort: true },
	// three.js is one lazy chunk of about 550 kB, loaded only when a 3-D view first opens.
	build: { chunkSizeWarningLimit: 700 },
	test: {
		projects: [
			{ extends: true, test: { name: 'shell', include: ['src/**/*.test.ts'], exclude: ['**/node_modules/**', 'src/lib/acquire/**'], environment: 'node' } },
			// Stores built on runes are tested as the shell runs them: Node, with Svelte modules
			// compiled for the browser (the server build makes every rune a plain value).
			{ extends: true, test: { name: 'stores', include: ['src/lib/acquire/**/*.test.ts'], environment: './src/test/runes-environment.ts' } }
		]
	}
});
