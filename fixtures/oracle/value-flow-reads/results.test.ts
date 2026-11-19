import { test } from 'vitest'
const noop = (..._args: unknown[]) => {}
/** A promise of `false`. */ async function later() { return false }
;(later() ? noop : test)('an async call is a promise', () => {})
/** An iterator finishing with `false`. */ function* steps() { return false }
;(steps() ? noop : test)('a generator call is an iterator', () => {})
/** `false`. */ function now() { return false }
;(now() ? noop : test)('a plain call is its result', () => {})
/** A promise of the test API. */ async function make() { return test }
;(await make())('an awaited api is the api', () => {})
/** `noop`; its second return is unreachable. */ function first() { return noop; return test }
first()('a return after a return is never taken', () => {})
/** The test API; its second return is unreachable. */ function second() { return test; return noop }
second()('the first return is taken', () => {})
