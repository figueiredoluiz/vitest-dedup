import { register } from './case.test'

/** Registers through what the test file exports. */
export function cases() {
  register('registered back through the test file', () => {})
}
