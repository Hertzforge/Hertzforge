import type { UpdateProgress } from './updates';

/** The update being downloaded or installed, or null when none is: set by `checkForUpdates`. */
export const updating = $state<{ now: UpdateProgress | null }>({ now: null });
