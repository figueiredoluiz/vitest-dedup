import { afterEach, beforeEach } from 'vitest'
import { cases } from './cases'

export function closed() {
  beforeEach(() => {})
  afterEach(() => {})
}

// One call away from `closed`: it may register tests, and does.
export function open() {
  beforeEach(() => {})
  cases()
}
