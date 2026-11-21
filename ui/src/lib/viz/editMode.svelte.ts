/**
 * Whether an element table's 3-D view is adding elements (its `pick`, Add) or selecting them
 * (Select, the default). Kept here, not in the view, because each element added answers with a
 * new view and the mode carries over to it.
 */

let adding = $state(false);

export function isAdding(): boolean {
	return adding;
}

export function setAdding(on: boolean) {
	adding = on;
}
