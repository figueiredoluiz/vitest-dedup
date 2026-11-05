import { expect, test } from 'vitest'

Object.defineProperty(Object.prototype, 'rowProofProbe', {
  configurable: true,
  get() { this.n = 2; return 0 },
})
const box = { n: 1 }
void box.rowProofProbe
delete Object.prototype.rowProofProbe
test.each([[box.n], [1]])('own input', n => expect(n).toBe(1))
