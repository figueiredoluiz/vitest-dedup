import { test, expect } from 'vitest';
import { vi as tools } from 'vitest';
test('reads dependency', async () => { tools.mock('./dep', () => ({value:'mocked'})); const { value } = await import('./dep'); expect(value).toBe('mocked') });
