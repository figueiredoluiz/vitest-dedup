// A hook at collection scope whose timeout runs `valueOf`.
import { beforeEach, describe, expect, test } from 'vitest'
import { coerced } from './support'

describe('hook-timeout', () => {
  beforeEach(() => {}, coerced as unknown as number)
  test('stays comparable', () => {
    expect(1).toBe(1)
  })
})
