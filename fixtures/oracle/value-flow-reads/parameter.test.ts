import { test } from 'vitest'
const noop = (..._args: unknown[]) => {}
/** Returns `test`, whatever it is handed. */ function replaced(t: any) { t = test; return t }
/** Returns its argument, or `test` when that is falsy. */ function defaulted(t: any) { if (!t) t = test; return t }
/** Returns its argument. */ function kept(t: any) { return t }
replaced(noop)('parameter replaced', () => {})
defaulted(undefined)('parameter defaulted', () => {})
defaulted(noop)('parameter kept by the guard', () => {})
kept(noop)('parameter returned', () => {})
