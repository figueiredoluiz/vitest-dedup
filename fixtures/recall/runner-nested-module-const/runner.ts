import { test, expect } from 'vitest'
export function run(label) {
  return local(label)
}
const local = label => {
  test(`${label} first`, () => { expect(label.trim().toLowerCase()).toBe('page') })
  test(`${label} second`, () => { expect(label.trim().toLowerCase()).toBe('page') })
}
