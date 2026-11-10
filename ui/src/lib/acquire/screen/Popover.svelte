<script lang="ts">
	/**
	 * A small window anchored to the button that opened it: a menu, or a panel of options. It stays
	 * on screen, closes on Escape, Tab out of a menu or a click outside, and gives the keyboard back
	 * to its button. Arrow keys move between menu items.
	 */
	import { onMount, tick, type Snippet } from 'svelte';
	import './screen.css';

	interface Props {
		anchor: HTMLElement | null;
		label: string;
		/** A menu of items; otherwise a dialog of options. */
		menu?: boolean;
		/** Which edge of the anchor it lines up with. */
		align?: 'start' | 'end';
		/** Opens upward first (from a bar at the bottom of the screen). */
		up?: boolean;
		testid?: string;
		onclose: () => void;
		children: Snippet;
	}
	let { anchor, label, menu = false, align = 'start', up = false, testid, onclose, children }: Props = $props();

	let el = $state<HTMLElement | null>(null);
	let left = $state(0);
	let top = $state(0);
	let placed = $state(false);

	const GAP = 4;
	const EDGE = 8;

	function place() {
		if (!el || !anchor) return;
		const r = anchor.getBoundingClientRect();
		const w = el.offsetWidth;
		const h = el.offsetHeight;
		const vw = window.innerWidth;
		const vh = window.innerHeight;
		let y = up ? r.top - h - GAP : r.bottom + GAP;
		if (!up && y + h > vh - EDGE && r.top - h - GAP >= EDGE) y = r.top - h - GAP;
		if (up && y < EDGE && r.bottom + GAP + h <= vh - EDGE) y = r.bottom + GAP;
		let x = align === 'end' ? r.right - w : r.left;
		left = Math.round(Math.max(EDGE, Math.min(vw - w - EDGE, x)));
		top = Math.round(Math.max(EDGE, Math.min(vh - h - EDGE, y)));
		placed = true;
	}

	function items(): HTMLElement[] {
		return el ? [...el.querySelectorAll<HTMLElement>('[role^="menuitem"]')].filter((b) => !b.hasAttribute('disabled') && b.getAttribute('aria-disabled') !== 'true') : [];
	}

	function close(refocus = true) {
		onclose();
		if (refocus) anchor?.focus();
	}

	function onkeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') {
			e.preventDefault();
			e.stopPropagation();
			close();
			return;
		}
		if (e.defaultPrevented) return;
		const list = items();
		if (!list.length) return;
		const i = list.indexOf(document.activeElement as HTMLElement);
		let next = -1;
		if (e.key === 'ArrowDown') next = (i + 1) % list.length;
		else if (e.key === 'ArrowUp') next = (i - 1 + list.length) % list.length;
		else if (e.key === 'Home') next = 0;
		else if (e.key === 'End') next = list.length - 1;
		else if (e.key === 'Tab' && menu) close(false);
		if (next < 0) return;
		e.preventDefault();
		list[next].focus();
	}

	onMount(() => {
		place();
		// Once it is drawn (a hidden element takes no focus): the first item, or the first field.
		void tick().then(() => {
			const first = menu ? items()[0] : el?.querySelector<HTMLElement>('input, select, textarea, button, [tabindex]:not([tabindex="-1"])');
			first?.focus({ preventScroll: true });
		});
		const outside = (e: PointerEvent) => {
			const t = e.target as Node | null;
			if (!t || el?.contains(t) || anchor?.contains(t)) return;
			close(false);
		};
		// Escape closes it wherever the keyboard is.
		const escape = (e: KeyboardEvent) => {
			if (e.key !== 'Escape' || e.defaultPrevented) return;
			e.preventDefault();
			close(el?.contains(document.activeElement) ?? false);
		};
		const again = () => place();
		document.addEventListener('pointerdown', outside, true);
		window.addEventListener('keydown', escape);
		window.addEventListener('resize', again);
		const ro = typeof ResizeObserver === 'undefined' || !el ? null : new ResizeObserver(again);
		if (el) ro?.observe(el);
		return () => {
			document.removeEventListener('pointerdown', outside, true);
			window.removeEventListener('keydown', escape);
			window.removeEventListener('resize', again);
			ro?.disconnect();
		};
	});
</script>

<div
	class="pop"
	class:menu
	class:placed
	role={menu ? 'menu' : 'dialog'}
	aria-label={label}
	tabindex="-1"
	data-testid={testid}
	bind:this={el}
	style:left="{left}px"
	style:top="{top}px"
	{onkeydown}
>
	{@render children()}
</div>

<style>
	.pop {
		position: fixed;
		z-index: 60;
		min-width: 180px;
		max-width: min(420px, calc(100vw - 16px));
		max-height: calc(100vh - 16px);
		overflow-y: auto;
		padding: 4px;
		visibility: hidden;
		background: var(--bg-2);
		border: 1px solid var(--line-2);
		border-radius: var(--radius-md);
		box-shadow: var(--shadow-pop);
	}

	.pop.placed {
		visibility: visible;
	}

	.pop:not(.menu) {
		padding: var(--sp-3);
	}

	.pop:focus {
		outline: none;
	}
</style>
