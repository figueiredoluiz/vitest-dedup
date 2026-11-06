import { expect, test } from 'vitest'

const key = '\uD83D' + '\uDE00'
test.each([key, '😀'])('joined value', value => { expect(value).toBe('😀') })
const values = { '😀': '\uD800' }
test.each([values[key], '\uD800'])('unproved selector', value => { expect(value).toBe('\uD800') })
