/**
 * Theme preference. `system` follows the OS (dark when the OS says nothing). Canvas and WebGL
 * consumers read colors from CSS tokens, so they watch `version` and repaint when it changes.
 */

export type ThemePref = 'system' | 'dark' | 'light';
export type ResolvedTheme = 'dark' | 'light';

const STORAGE_KEY = 'sds.theme';

class Theme {
	pref = $state<ThemePref>('system');
	resolved = $state<ResolvedTheme>('dark');
	/** Increments on every effective change. */
	version = $state(0);

	private media: MediaQueryList | null = null;

	init(override?: ThemePref | null): void {
		if (typeof window === 'undefined') return;
		let stored: ThemePref | null = null;
		try {
			stored = localStorage.getItem(STORAGE_KEY) as ThemePref | null;
		} catch {
			/* storage blocked: fall back to the OS */
		}
		this.pref = override ?? (stored === 'dark' || stored === 'light' || stored === 'system' ? stored : 'system');
		this.media = window.matchMedia('(prefers-color-scheme: light)');
		this.media.addEventListener('change', () => this.apply());
		this.apply();
	}

	set(pref: ThemePref): void {
		this.pref = pref;
		try {
			localStorage.setItem(STORAGE_KEY, pref);
		} catch {
			/* not persisted; still applied */
		}
		this.apply();
	}

	cycle(): void {
		this.set(this.pref === 'system' ? 'dark' : this.pref === 'dark' ? 'light' : 'system');
	}

	private apply(): void {
		const root = document.documentElement;
		if (this.pref === 'system') root.removeAttribute('data-theme');
		else root.setAttribute('data-theme', this.pref);
		this.resolved = this.pref === 'system' ? (this.media?.matches ? 'light' : 'dark') : this.pref;
		this.version += 1;
	}
}

export const theme = new Theme();

/** Current value of a CSS custom property on <html>. */
export function cssVar(name: string): string {
	if (typeof document === 'undefined') return '';
	return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}
