import { test, expect } from 'vitest'

export function run() {
  test('normalizes the label', () => {
    expect(' Label '.trim().toLowerCase()).toBe('label')
  })
}
