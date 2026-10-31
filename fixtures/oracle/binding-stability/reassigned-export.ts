import { beforeEach } from 'vitest'
import { cases } from './support'

export const closed = beforeEach

let moved = beforeEach
moved = cases
export { moved as open }
