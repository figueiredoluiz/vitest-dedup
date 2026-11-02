import { test, expect } from 'vitest'
import leaf from './leaf'
export function run(label) {
  function captured() {
    test(`${label} local`, () => {
      expect(label.trim().toLowerCase()).toBe('page')
    })
  }
  function shadow(label) {
    test(`${label} shadow`, () => {
      expect(label.trim().toLowerCase()).toBe('other')
    })
  }
  captured()
  shadow('other')
  return leaf(label)
  test('dead tail', () => { expect('dead').toBe('live') })
}
