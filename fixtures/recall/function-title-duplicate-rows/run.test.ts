import { test, expect } from 'vitest'

test.each([1, 1, 2])(/** Supplies only the test title; its body must never execute. */ function row() { throw Error('title') }, n => expect(n).toBe(1))
