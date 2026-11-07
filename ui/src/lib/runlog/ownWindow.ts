/**
 * The run log in a window of its own, for a second monitor, as Visual Dynamics' dock floats. The
 * log the service holds is this connection's, so the window is not a second shell: the same panel
 * and the same state, mounted into a blank window with this document's styles and theme.
 */
import { mount, unmount } from 'svelte';
import RunLogPanel from './RunLogPanel.svelte';
import type { RunLogState } from './store.svelte';

/** This document's stylesheets, by address so a blank window can load them, into `to`. */
export function copyStyles(from: Document, to: Document): void {
	for (const node of from.head.querySelectorAll('style, link[rel="stylesheet"]')) {
		const copy = to.importNode(node, true) as HTMLElement;
		if (copy instanceof to.defaultView!.HTMLLinkElement || copy.tagName === 'LINK') {
			copy.setAttribute('href', (node as HTMLLinkElement).href);
		}
		to.head.appendChild(copy);
	}
	for (const { name, value } of Array.from(from.documentElement.attributes)) {
		to.documentElement.setAttribute(name, value);
	}
}

/** Open the log's window. False when the window was refused. */
export function openLogWindow(rl: RunLogState): boolean {
	const w = window.open('', 'hertzforge-run-log', 'width=760,height=900');
	if (!w) return false;
	w.document.title = 'Run Log';
	copyStyles(document, w.document);
	w.document.body.style.cssText = 'margin:0;height:100vh;overflow:hidden;background:var(--bg-0, #0b0d10)';
	const target = w.document.createElement('div');
	target.style.cssText = 'height:100%;display:flex;flex-direction:column';
	w.document.body.appendChild(target);
	const panel = mount(RunLogPanel, { target, props: { rl } });
	rl.windowed = true;
	rl.closeWindow = () => w.close();
	const closeWith = () => w.close();
	window.addEventListener('beforeunload', closeWith);
	w.addEventListener('pagehide', () => {
		window.removeEventListener('beforeunload', closeWith);
		void unmount(panel);
		rl.windowed = false;
		rl.closeWindow = null;
	});
	return true;
}
