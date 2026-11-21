import { expect, test as api } from 'vitest'
const enabled = true
const chosen = enabled ? api : later
const later = api
chosen('ordered left', () => expect(4).toBe(4))
chosen('ordered right', () => expect(4).toBe(4))
