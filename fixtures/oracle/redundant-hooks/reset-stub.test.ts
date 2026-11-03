import { beforeEach, expect, test, vi } from 'vitest'

beforeEach(() => { vi.unstubAllGlobals() })
beforeEach(() => { vi.unstubAllGlobals() })
test('stubs a global', () => { vi.stubGlobal('rc12Marker', 1); expect((globalThis as any).rc12Marker).toBe(1) })
test('starts without the stub', () => { expect((globalThis as any).rc12Marker).toBeUndefined() })
