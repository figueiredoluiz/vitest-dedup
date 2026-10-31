import { beforeEach } from 'vitest'

const kept: Function = beforeEach

/** Registers through an annotated hook alias never written. */
export function closed() {
  kept(() => {})
}

/** Registers a hook, annotated and never written. */
export const annotated: () => void = () => {
  beforeEach(() => {})
}
