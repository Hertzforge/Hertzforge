import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [sveltekit()],
	server: { host: '127.0.0.1', port: 5191, strictPort: true },
	// three.js is one lazy chunk of about 550 kB, loaded only when a 3-D view first opens.
	build: { chunkSizeWarningLimit: 700 },
	test: {
		include: ['src/**/*.test.ts'],
		environment: 'node'
	}
});
