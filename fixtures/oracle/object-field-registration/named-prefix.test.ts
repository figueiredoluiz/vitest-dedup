import { expect, test } from 'vitest'

;({ prefix: test('named field prefix', () => expect(18).toBe(18)), run: test.extend({}) }).run('after named prefix', () => expect(19).toBe(19))
