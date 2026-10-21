// Spies on a method.
import { vi } from 'vitest'

/** Setup helper `setupSpy` of this case, called from its test file(s). */
export function setupSpy() {
  vi.spyOn(console, 'info')
}
