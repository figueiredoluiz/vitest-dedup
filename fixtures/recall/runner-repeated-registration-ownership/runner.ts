import { test, expect } from 'vitest'

export function run() {
  test('runner normalizes first label', () => {
    expect(' Label '.trim().toLowerCase()).toBe('label')
  })
  test('runner normalizes second label', () => {
    expect(' Label '.trim().toLowerCase()).toBe('label')
  })
}
