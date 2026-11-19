import { test } from 'vitest'
const noop = (..._args: unknown[]) => {}
;(1n ? noop : test)('a nonzero bigint is truthy', () => {})
;(0n ? noop : test)('a zero bigint is falsy', () => {})
;('a'['0'] ? noop : test)('a canonical string index reads a character', () => {})
;(''['0'] ? noop : test)('an index past a string is undefined', () => {})
;([test]['00'] ?? noop)('a noncanonical index is a property name', () => {})
;([test]['0'] ?? noop)('a canonical index reads the element', () => {})
;((test as any).__defineGetter__ ? noop : test)('an inherited method is present', () => {})
;((test as any).__missing__ ? noop : test)('a missing member is undefined', () => {})
;((test.skipIf as any).nonexistent ? test : noop)('a builder not called yet lacks other members', () => {})
;((test.skipIf as any).nonexistent ? noop : test)('so the other branch is taken', () => {})
;(/a/ ? noop : test)('a regular expression is an object', () => {})
