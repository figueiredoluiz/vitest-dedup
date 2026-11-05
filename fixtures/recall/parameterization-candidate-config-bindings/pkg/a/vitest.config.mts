import { defineProject, mergeConfig } from 'vitest/config'
import { vitestBaseConfig } from '../../vitest.config.base.mts'
import type
{ UserConfig } from 'vitest/config' with { 'resolution-mode': 'require' }

const vitestConfig =
mergeConfig(
  vitestBaseConfig,
  defineProject({ test: { name: 'a' } }),
)
export default vitestConfig
