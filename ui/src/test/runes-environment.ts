/**
 * The test environment: Node, with modules transformed for the browser. Svelte then compiles
 * `.svelte.ts` modules the way the shell runs them (state is a deep proxy, a snapshot is a copy,
 * a derived follows reassignment); the server build would make every rune a plain value.
 */
import type { Environment } from 'vitest/environments';

const environment: Environment = {
	name: 'node-runes',
	transformMode: 'web',
	setup: () => ({ teardown() {} })
};

export default environment;
