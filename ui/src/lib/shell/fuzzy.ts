/**
 * Fuzzy matching for the command palette: the query's characters must appear in order. Scores
 * favor matches at word starts, consecutive runs and early positions. Returns matched indices for
 * highlighting.
 */

export interface FuzzyMatch {
	score: number;
	indices: number[];
}

function isBoundary(text: string, i: number): boolean {
	if (i === 0) return true;
	const prev = text[i - 1];
	const cur = text[i];
	return /[\s\-_./&,()]/.test(prev) || (prev === prev.toLowerCase() && cur !== cur.toLowerCase());
}

export function fuzzyMatch(query: string, text: string): FuzzyMatch | null {
	const q = query.trim().toLowerCase();
	if (!q) return { score: 0, indices: [] };
	const t = text.toLowerCase();

	// Exact substring: a strong, cheap path.
	const sub = t.indexOf(q);
	if (sub >= 0) {
		const indices = Array.from({ length: q.length }, (_, k) => sub + k);
		return { score: 100 + (isBoundary(text, sub) ? 40 : 0) - sub * 0.5 - (t.length - q.length) * 0.05, indices };
	}

	const tokens = q.split(/\s+/).filter(Boolean);
	const indices: number[] = [];
	let score = 0;
	let ti = 0;
	for (const token of tokens) {
		let prevMatch = -2;
		for (const ch of token) {
			let found = -1;
			// Prefer the next boundary occurrence of ch, else the next occurrence at all.
			for (let j = ti; j < t.length; j++) {
				if (t[j] === ch && isBoundary(text, j)) {
					found = j;
					break;
				}
			}
			const plain = t.indexOf(ch, ti);
			if (found < 0 || (plain >= 0 && plain === prevMatch + 1)) found = plain;
			if (found < 0) return null;
			score += 1;
			if (found === prevMatch + 1) score += 5;
			if (isBoundary(text, found)) score += 8;
			score -= Math.min(found - ti, 10) * 0.3;
			indices.push(found);
			prevMatch = found;
			ti = found + 1;
		}
	}
	return { score, indices };
}

/** Split text into runs for rendering highlights. */
export function highlightRuns(text: string, indices: number[]): { text: string; hit: boolean }[] {
	if (!indices.length) return [{ text, hit: false }];
	const set = new Set(indices);
	const runs: { text: string; hit: boolean }[] = [];
	for (let i = 0; i < text.length; i++) {
		const hit = set.has(i);
		const last = runs[runs.length - 1];
		if (last && last.hit === hit) last.text += text[i];
		else runs.push({ text: text[i], hit });
	}
	return runs;
}
