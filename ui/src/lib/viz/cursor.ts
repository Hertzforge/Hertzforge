/**
 * Linked cursors. Panes that share a `link` name share one cursor position on x: hover one and
 * the others draw the line and readout at the same x. Framework-free so uPlot hooks can drive it
 * synchronously and tests can exercise it without a DOM.
 */

export interface CursorState {
	/** x in data units, or null when no pane in the group is hovered. */
	x: number | null;
	/** Id of the pane the pointer is in. */
	source: string | null;
}

type Listener = (state: CursorState) => void;

export class CursorHub {
	private groups = new Map<string, CursorState>();
	private listeners = new Map<string, Set<Listener>>();

	get(group: string): CursorState {
		return this.groups.get(group) ?? { x: null, source: null };
	}

	/** Pane `source` moved the cursor to `x`. */
	set(group: string, x: number, source: string): void {
		if (!Number.isFinite(x)) return this.clear(group, source);
		const prev = this.groups.get(group);
		if (prev && prev.x === x && prev.source === source) return;
		this.emit(group, { x, source });
	}

	/** Pane `source` lost the pointer. Ignored if another pane has taken the cursor since. */
	clear(group: string, source: string): void {
		const prev = this.groups.get(group);
		if (!prev || prev.x === null || prev.source !== source) return;
		this.emit(group, { x: null, source: null });
	}

	subscribe(group: string, listener: Listener): () => void {
		let set = this.listeners.get(group);
		if (!set) this.listeners.set(group, (set = new Set()));
		set.add(listener);
		listener(this.get(group));
		return () => {
			set!.delete(listener);
			if (set!.size === 0) {
				this.listeners.delete(group);
				this.groups.delete(group);
			}
		};
	}

	private emit(group: string, state: CursorState): void {
		this.groups.set(group, state);
		for (const l of [...(this.listeners.get(group) ?? [])]) l(state);
	}
}

/** The shell's one hub. */
export const cursors = new CursorHub();
