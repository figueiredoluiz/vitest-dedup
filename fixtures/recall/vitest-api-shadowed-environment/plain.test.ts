import { test, expect } from 'vitest';
test('reads environment', () => expect(process.env.U12_SPIKE_OWNED).toBe('mocked'));
