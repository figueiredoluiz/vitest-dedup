import { beforeEach } from 'vitest'
import { b } from './b'

/** Registers a hook, then calls into the cycle. */
export function a(again: boolean) {
  beforeEach(() => {})
  if (again) b(false)
}
