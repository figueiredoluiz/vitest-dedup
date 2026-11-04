import { test, expect } from 'vitest';
import { vi as tools } from 'vitest';
tools.mock('./dep', () => ({value:'mocked'}));
test('reads dependency', async () => { const { value } = await import('./dep'); expect(value).toBe('mocked') });
