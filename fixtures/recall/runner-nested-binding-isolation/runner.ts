import { test, expect } from 'vitest'
import leaf from './leaf'
export function run(label) {
  test(`${label} bound`, () => { expect(label.trim()).not.toBe('neither') })
  return leaf()
}
