import { expect, test } from 'vitest'

const original = test.extend
;({ earlier: (test.extend = () => () => {}), run: test.extend({}) }).run('after replacement', () => {})
test.extend = original
;({ run: test.extend({}), later: (test.extend = () => () => {}) }).run('captured before replacement', () => expect(14).toBe(14))
test.extend = original
