import { test, expect } from 'vitest';
import { vi } from 'vitest';
vi.mock('./dep', () => ({value:'mocked'}));
test('reads dependency', async () => { const { value } = await import('./dep'); expect(value).toBe('mocked') });
