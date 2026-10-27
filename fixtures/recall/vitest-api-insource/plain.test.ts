import { test, expect } from 'vitest';
test('reads environment', () => expect(process.env.U12_API_OWNED).toBe('mocked'));
