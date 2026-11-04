import { test, expect } from 'vitest';
import { vi } from 'vitest';
test('reads dependency', async () => { vi.mock('./dep', () => ({value:'mocked'})); const { value } = await import('./dep'); expect(value).toBe('mocked') });
