import { test } from 'vitest'
const noop = (..._args: unknown[]) => {}
const o = { flag: false }
o.flag = true
;(o.flag ? noop : test)('a written member is read as written', () => {})
const p = { flag: false }
;(p.flag ? noop : test)('an unwritten member is read as declared', () => {})
const a: unknown[] = []
a.push(1)
;(a[0] ? noop : test)('an array a method changed is read as changed', () => {})
const b: unknown[] = []
;(b[0] ? noop : test)('an unchanged array is read as declared', () => {})
/** A function whose member is written below. */ function f() {}
;(f as any).flag = true
;((f as any).flag ? noop : test)('a written function member is read as written', () => {})
/** A function whose member is never written. */ function g() {}
;((g as any).flag ? noop : test)('an unwritten function member is undefined', () => {})
