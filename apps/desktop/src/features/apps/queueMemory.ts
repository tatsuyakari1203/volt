/** An app of a queue that did not run to its end. */
export interface PendingApp {
  id: string;
  name: string;
}

/** Where the apps still to remove are kept, so the next open of the page can say what was skipped. */
const KEY = 'volt.apps.queue';

function store(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/**
 * Remembers the apps of the queue that have not finished (the one running and those waiting).
 * An empty list forgets. Storage that is missing or full is ignored: the notice is a courtesy.
 */
export function rememberQueue(apps: ReadonlyArray<PendingApp>): void {
  try {
    if (apps.length === 0) store()?.removeItem(KEY);
    else store()?.setItem(KEY, JSON.stringify(apps.map(({ id, name }) => ({ id, name }))));
  } catch {
    // Nothing to do: the queue itself is unaffected.
  }
}

/** Forgets the remembered queue. */
export function forgetQueue(): void {
  rememberQueue([]);
}

/** The apps a queue left unfinished when the page or the window was closed; empty when none. */
export function readRememberedQueue(): PendingApp[] {
  try {
    const raw = store()?.getItem(KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (item): item is PendingApp =>
        typeof item === 'object' &&
        item !== null &&
        typeof (item as PendingApp).id === 'string' &&
        typeof (item as PendingApp).name === 'string',
    );
  } catch {
    return [];
  }
}
