import { test, expect } from 'vitest'

/** Binding-based function names remain unresolved by the analyzer. */
function boundTitle() { throw Error('title') }
test(boundTitle, () => expect(1).toBe(1))
test(function () {}, () => expect(1).toBe(1))
test(() => {}, () => expect(1).toBe(1))
