import { test as other } from './conflicting';
import { test, expect } from 'vitest';
other('not a framework registration', () => {});
test('retained direct test', () => { expect(typeof other).toBe('function'); });
