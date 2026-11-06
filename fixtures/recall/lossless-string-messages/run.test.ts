import { expect, test } from 'vitest'

test('\uD800', () => { expect(7).toBe(7) })
test.skip('\uD800', () => { expect(7).toBe(7) })
test('\uFFFDd800', () => { expect(7).toBe(7) })
test.skip('\uFFFDd800', () => { expect(7).toBe(7) })
