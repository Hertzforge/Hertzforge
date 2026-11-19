/**
 * Moving through a list from the keyboard, as a menu or a row of tabs does: the arrows step and
 * wrap, Home and End go to the ends, and a letter goes to the next entry starting with it.
 */

/** The index a key moves to from `at` among `count` entries (`at` -1 when none has focus); null for a key that does not move. */
export function stepIndex(key: string, at: number, count: number, axis: 'vertical' | 'horizontal' = 'vertical'): number | null {
	if (count <= 0) return null;
	const [back, forward] = axis === 'vertical' ? ['ArrowUp', 'ArrowDown'] : ['ArrowLeft', 'ArrowRight'];
	if (key === forward) return at < 0 ? 0 : (at + 1) % count;
	if (key === back) return at < 0 ? count - 1 : (at - 1 + count) % count;
	if (key === 'Home') return 0;
	if (key === 'End') return count - 1;
	return null;
}

/** The next entry after `at` whose words start with `char` (any case), wrapping; null when none does. */
export function typeAhead(words: string[], at: number, char: string): number | null {
	if (char.length !== 1 || !/\S/.test(char)) return null;
	const c = char.toLowerCase();
	for (let k = 1; k <= words.length; k++) {
		const i = (at + k + words.length) % words.length;
		if (words[i].trim().toLowerCase().startsWith(c)) return i;
	}
	return null;
}
