import { test, expect } from 'vitest';
import { vitest } from 'vitest';
vitest.stubEnv('U12_SPIKE_OWNED', 'mocked');
test('reads environment', () => expect(process.env.U12_SPIKE_OWNED).toBe('mocked'));
