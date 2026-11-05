import { defineProject, mergeConfig } from 'vitest/config'
import base from './base.mts'

import.meta.url && (base.test.globals = false)
const config = mergeConfig(base, defineProject({ test: { name: 'import-meta' } }))
export default config
