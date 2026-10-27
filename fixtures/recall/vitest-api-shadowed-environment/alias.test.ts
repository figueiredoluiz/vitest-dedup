import { test, expect } from 'vitest';
const vitest = { stubEnv: () => {} };
vitest.stubEnv('U12_SPIKE_OWNED', 'mocked');
test('reads environment', () => expect(process.env.U12_SPIKE_OWNED).toBe('mocked'));
