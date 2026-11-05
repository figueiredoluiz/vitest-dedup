import { defineConfig, mergeConfig } from 'vitest/config'
import { vitestBaseConfig } from './vitest.config.base.mts'

const vitestConfig = mergeConfig(
  vitestBaseConfig,
  defineConfig({ test: { projects: ['pkg/a/vitest.config.mts'] } }),
)
export default vitestConfig
