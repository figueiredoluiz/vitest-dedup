// Registers a hook that runs before every test after it in the calling scope.
import { beforeEach } from 'vitest'

/** Setup helper `seedCart` of this case, called from its test file(s). */
export function seedCart() {
  beforeEach(() => {
    localStorage.setItem('cart', '["apple"]')
  })
}
