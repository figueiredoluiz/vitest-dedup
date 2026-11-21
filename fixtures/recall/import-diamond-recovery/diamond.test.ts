import { check } from './shared'
import { expect } from 'vitest'
import './left'
import './right'

check('first', () => expect(42).toBe(42))
check('second', () => expect(42).toBe(42))
check('distinct', () => expect(42).not.toBe(0))
