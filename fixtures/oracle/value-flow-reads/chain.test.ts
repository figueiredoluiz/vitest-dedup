import { test } from 'vitest'
const noop = (..._args: unknown[]) => {}
const none: any = undefined
;(none?.missing ?? test)('short-circuit', () => {})
;(none?.a.b.c ?? test)('short-circuit past later links', () => {})
;(none?.make() ?? test)('short-circuit before a call', () => {})
;(({ m: noop })?.m ?? test)('present link', () => {})
const o = { r: { run: test } }
o?.r?.run('optional links to the api', () => {})
