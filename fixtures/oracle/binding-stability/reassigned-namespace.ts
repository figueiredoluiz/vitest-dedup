import * as vitest from 'vitest'
import { cases } from './support'

let api: any = vitest
api = { beforeEach: cases }

/** Registers through the namespace. */
export function closed() {
  vitest.beforeEach(() => {})
}

/** Registers through a rewritten alias of it. */
export function open() {
  api.beforeEach(() => {})
}
