import { defineConfig, mergeConfig } from 'vitest/config'
import base from './base.mts'

export default mergeConfig(base, defineConfig({ test: { projects: ['./pkg/vitest.config.mts'] } }));
export type ConfigLabel = string
