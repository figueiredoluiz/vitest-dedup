import { test as register } from 'vitest'

const enabled = true
export default (enabled ? register : register)
