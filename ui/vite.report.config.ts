/**
 * The exported report's viewer, built alone into one self-contained HTML file: its script,
 * styles and fonts inlined, three.js included (no chunk is loaded later), nothing fetched from a
 * network. Written to the Python package (`src/hertzforge/report/viewer.html`), which fills in the
 * report's title and data when it exports a report.
 *
 *     npm --prefix ui run build:report
 */
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import { defineConfig, type Plugin } from 'vite';

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));
const OUT = here('../src/hertzforge/report/viewer.html');

/** Inline the built script and stylesheet into the page, and write it to the package. */
function oneFile(): Plugin {
	return {
		name: 'hertzforge-report-one-file',
		apply: 'build',
		enforce: 'post',
		generateBundle(_options, bundle) {
			const page = Object.values(bundle).find((item) => item.fileName.endsWith('.html'));
			if (!page || page.type !== 'asset') throw new Error('the viewer build made no page');
			let html = String(page.source);
			for (const [name, item] of Object.entries(bundle)) {
				if (item.type === 'chunk' && item.isEntry) {
					// a closing tag inside the script would end it early; and with every dynamic
					// import inlined there is nothing to preload, which Vite leaves as a placeholder
					const code = item.code.replace(/<\/script/gi, '<\\/script').replace(/__VITE_PRELOAD__/g, 'void 0');
					html = html.replace(new RegExp(`<script[^>]*src="[^"]*${escape(name)}"[^>]*></script>`), () => `<script type="module">${code}</script>`);
					delete bundle[name];
				} else if (item.type === 'asset' && name.endsWith('.css')) {
					const css = String(item.source).replace(/<\/style/gi, '<\\/style');
					html = html.replace(new RegExp(`<link[^>]*href="[^"]*${escape(name)}"[^>]*>`), () => `<style>${css}</style>`);
					delete bundle[name];
				}
			}
			if (/<script[^>]*\ssrc=|<link[^>]*stylesheet/.test(html)) throw new Error('the viewer still loads a file');
			page.source = html;
		},
		writeBundle(options) {
			const built = readFileSync(`${options.dir}/report-viewer/index.html`, 'utf-8');
			writeFileSync(OUT, built);
		}
	};
}

function escape(text: string): string {
	return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export default defineConfig({
	root: here('.'),
	plugins: [svelte(), oneFile()],
	resolve: { alias: { $lib: here('./src/lib') } },
	build: {
		outDir: here('./build-report'),
		emptyOutDir: true,
		// fonts and any image as data URIs, inside the page
		assetsInlineLimit: 100_000_000,
		cssCodeSplit: false,
		modulePreload: { polyfill: false },
		chunkSizeWarningLimit: 2000,
		rollupOptions: {
			input: here('./report-viewer/index.html'),
			output: { inlineDynamicImports: true }
		}
	}
});
