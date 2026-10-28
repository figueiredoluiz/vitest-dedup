export const KEY = Symbol('key')
export const ROWS = [{ id: 1 }, { id: 2 }]

// A load-time write the registration proof cannot clear: judging these names would refuse the file.
;(globalThis as Record<string, unknown>).dataLoaded = true
