import type
{ ViteUserConfig } from './types.mts' with { 'resolution-mode': 'import' }

export const vitestBaseConfig =
{
  test: { globals: true, include: ['**/*.test.ts'] },
} as const satisfies ViteUserConfig
