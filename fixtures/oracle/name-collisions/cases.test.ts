import { describe, expect, test } from 'vitest'

// Equal names do not require equal bodies.
test('collision', () => expect(1).toBe(1))
test('collision', () => expect(2).toBe(2))
// Different suite segments and literal separators must not alias.
describe('left', () => test('leaf', () => expect(3).toBe(3)))
describe('right', () => test('leaf', () => expect(4).toBe(4)))
test('left > leaf', () => expect(5).toBe(5))
// One finding for each rendered name across declarations; never self-pair rows.
test.each([[1], [2]])('round %i', value => expect(value).toBeGreaterThan(0))
test.each([[1], [2]])('round %i', value => expect(value).toBeLessThan(3))
test.each([[1], [2]])('one declaration', value => expect(value).toBeTruthy())
// Composite literals use JavaScript string coercion in templates.
test(`object ${{ a: 1 }}`, () => expect(6).toBe(6))
test('object [object Object]', () => expect(7).toBe(7))
test('object {a:1}', () => expect(8).toBe(8))
// Empty/todo name collisions stay excluded until their separate task.
test.todo('held')
test.todo('held')
test('empty', () => {})
test('empty', () => {})
