/**
 * The Measure screen's single keys, for a person with a hammer in one hand: Enter or A accepts the
 * latest hit, Backspace or R rejects it, U undoes the last accepted hit, the Left and Right arrows
 * move to the previous and next point, and Space arms or disarms. They work wherever the focus is,
 * with no button focused, and in both accept modes (in automatic mode they override the service's
 * verdict until the next hit lands).
 *
 * They never fire while a text field, a list or an editable region has the focus, so typing a
 * value never accepts a hit; nor with Ctrl, Cmd or Alt held, so the shell's own keys (the abort
 * key among them) keep their meaning; nor on a held key's repeats. Enter and Space on a focused
 * button or other control are left to that control, as everywhere else in the shell.
 */

export type ModalKeyAction = 'accept' | 'reject' | 'undo' | 'next' | 'previous' | 'arm';

/** The words on the buttons and in the guide, by action. */
export const MODAL_KEY_WORDS: Record<ModalKeyAction, string> = {
	accept: 'Enter or A',
	reject: 'Backspace or R',
	undo: 'U',
	previous: 'Left arrow',
	next: 'Right arrow',
	arm: 'Space'
};

export interface KeyLike {
	key: string;
	ctrlKey?: boolean;
	metaKey?: boolean;
	altKey?: boolean;
	repeat?: boolean;
	target?: EventTarget | null;
}

const TEXT_INPUTS = new Set(['text', 'number', 'search', 'email', 'password', 'tel', 'url', 'date', 'datetime-local', 'time', 'month', 'week']);

/** The focus is in a place that takes typing: a text field, a list, an editable region. */
export function takesTyping(target: EventTarget | null | undefined): boolean {
	const el = target as (HTMLElement & { type?: string; isContentEditable?: boolean }) | null | undefined;
	if (!el || typeof el !== 'object' || !('tagName' in el)) return false;
	const tag = String(el.tagName).toUpperCase();
	if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
	if (tag === 'INPUT') return TEXT_INPUTS.has(String(el.type || 'text').toLowerCase());
	return !!el.isContentEditable;
}

/** The focus is on a control that answers Enter and Space itself. */
function isControl(target: EventTarget | null | undefined): boolean {
	const el = target as HTMLElement | null | undefined;
	if (!el || typeof el !== 'object' || !('tagName' in el)) return false;
	const tag = String(el.tagName).toUpperCase();
	return tag === 'BUTTON' || tag === 'A' || tag === 'INPUT' || tag === 'SUMMARY' || el.getAttribute?.('role') === 'button' || el.getAttribute?.('role') === 'tab';
}

/** The action a key press asks for on the Measure screen, or null when it is not one of its keys. */
export function modalKeyAction(e: KeyLike): ModalKeyAction | null {
	if (e.ctrlKey || e.metaKey || e.altKey || e.repeat) return null;
	if (takesTyping(e.target)) return null;
	const k = e.key;
	if ((k === 'Enter' || k === ' ' || k === 'Spacebar') && isControl(e.target)) return null;
	if (k === 'Enter' || k === 'a' || k === 'A') return 'accept';
	if (k === 'Backspace' || k === 'r' || k === 'R') return 'reject';
	if (k === 'u' || k === 'U') return 'undo';
	if (k === 'ArrowLeft') return 'previous';
	if (k === 'ArrowRight') return 'next';
	if (k === ' ' || k === 'Spacebar') return 'arm';
	return null;
}
