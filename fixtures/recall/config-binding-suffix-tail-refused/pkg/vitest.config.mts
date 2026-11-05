import { defineProject, mergeConfig } from 'vitest/config'
import { base } from './base.mts'

export default mergeConfig(base, defineProject({ test: { name: 'suffix' } }))
