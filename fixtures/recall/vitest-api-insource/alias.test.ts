import { test, expect } from 'vitest';
const { vitest: tools } = import.meta.vitest;
tools.stubEnv('U12_API_OWNED', 'mocked');
test('reads environment', () => expect(process.env.U12_API_OWNED).toBe('mocked'));
