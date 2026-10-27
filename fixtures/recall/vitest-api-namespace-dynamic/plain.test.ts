import { test, expect } from 'vitest';
test('reads dependency', async () => { const { value } = await import('./dep'); expect(value).toBe('mocked') });
