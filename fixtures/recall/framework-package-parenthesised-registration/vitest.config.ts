// Ambient globals: the installed package may call `test` without importing it.
import { defineConfig } from 'vitest/config'

export default defineConfig({ test: { globals: true } })
