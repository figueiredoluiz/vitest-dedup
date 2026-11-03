/** Replaces Vitest's `unstubAllGlobals` through a run-time load of `vitest`, counting each call. */
export async function replaceReset(count: () => void) {
  const { vi: other } = await import('vitest')
  other.unstubAllGlobals = () => { count(); return other }
}
