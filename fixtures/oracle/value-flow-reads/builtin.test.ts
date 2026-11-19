import { test } from 'vitest'
const noop = (..._args: unknown[]) => {}
;(''.at ? test : noop)('string member', () => {})
;([].includes ? test : noop)('array member', () => {})
;((1).toExponential ? test : noop)('number member', () => {})
;('ab'[0] ? test : noop)('string index', () => {})
;((''as any).missing ? test : noop)('absent string member', () => {})
;((true as any).toFixed ? test : noop)('absent boolean member', () => {})
