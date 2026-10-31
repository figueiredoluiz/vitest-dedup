import * as vitest from 'vitest'
import { cases } from './support'

export const kept: any = vitest
export let api: any = vitest
api = { beforeEach: cases }
