import { test } from 'vitest'
/** Writes `test` to its argument's `reg`. */ function attach(f: any) { f.reg = test }
/** Hands its argument to `attach`. */ function relay(g: any) { attach(g) }
/** Writes `test` to its first argument's `reg` through `arguments`. */ function viaArguments(_f: any) { arguments[0].reg = test }
/** Writes `test` to its first argument's `reg`. */ function second(f: any, _g: any) { f.reg = test }
/** Would register a test; never called. */ function first() { test('never called', () => {}) }
attach(first)
;(first as any).reg('written by a callee', () => {})
/** Would register a test; never called. */ function other() { test('never called', () => {}) }
relay(other)
;(other as any).reg('written two hand-offs away', () => {})
/** Would register a test; never called. */ function third() { test('never called', () => {}) }
viaArguments(third)
;(third as any).reg('written through arguments', () => {})
/** Would register a test; never called. */ function fourth() { test('never called', () => {}) }
second(...[], fourth)
;(fourth as any).reg('written after a spread', () => {})
test('direct', () => {})
