import { test, expect } from 'vitest';
import * as framework from 'vitest';
framework.vitest.mock('./dep', () => ({value:'mocked'}));
test('reads dependency', async () => { const { value } = await import('./dep'); expect(value).toBe('mocked') });
