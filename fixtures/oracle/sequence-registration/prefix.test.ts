import { expect, test } from 'vitest'
function run(title, callback) { test(title, callback) }
;(test('prefix one', () => expect(6).toBe(6)), run)('after prefix', () => expect(7).toBe(7))
