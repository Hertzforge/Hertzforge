/**
 * Time plots that share one time axis: plots given the same link key show the same window of
 * time, zoom and pan together, and freeze and run again together. A page links its time plots
 * with one key. Nothing here is saved: a link holds what is on screen now.
 */

export interface TimeLink {
	/** The x range every plot of the link holds, or null for each plot's own. */
	x: [number, number] | null;
	frozen: boolean;
	/** The window shown, seconds (null for what the service sends); undefined until a plot sets it. */
	window: number | null | undefined;
}

const fresh = (): TimeLink => ({ x: null, frozen: false, window: undefined });

export class TimeLinks {
	private links = $state<Record<string, TimeLink>>({});
	private members = new Map<string, number>();

	/** The link's state, or a fresh one when no plot has joined it. */
	get(key: string): TimeLink {
		return this.links[key] ?? fresh();
	}

	/** Change the link's state; each plot of the link follows. */
	set(key: string, patch: Partial<TimeLink>): void {
		const was = this.links[key] ?? fresh();
		const next = { ...was, ...patch };
		if (next.x === was.x && next.frozen === was.frozen && next.window === was.window) return;
		this.links[key] = next;
	}

	/** A plot joins the link; the function returned takes it out, and the last one out ends the link. */
	join(key: string): () => void {
		this.members.set(key, (this.members.get(key) ?? 0) + 1);
		return () => {
			const n = (this.members.get(key) ?? 1) - 1;
			if (n > 0) this.members.set(key, n);
			else {
				this.members.delete(key);
				delete this.links[key];
			}
		};
	}

	/** How many plots are in the link. */
	size(key: string): number {
		return this.members.get(key) ?? 0;
	}
}

/** The links of this window's test screens. */
export const timeLinks = new TimeLinks();
