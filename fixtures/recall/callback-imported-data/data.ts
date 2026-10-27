export const KEY = Symbol('key')
export const ROWS = [{ id: 1 }, { id: 2 }]

// A load-time write the registration proof cannot clear, so only the values' shape keeps the file
// comparable.
;(globalThis as Record<string, unknown>).dataLoaded = true
