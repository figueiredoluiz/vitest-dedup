import { defineProject, mergeConfig } from 'vitest/config'
import base from './base.mjs'
import type from './effect.mjs'

const config = mergeConfig(base, defineProject({ test: { name: 'runtime-type' } }))
export default config
