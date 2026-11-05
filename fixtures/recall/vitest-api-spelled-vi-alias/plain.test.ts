import { test, expect } from 'vitest';
import { value } from './dep';
test('reads dependency', () => expect(value).toBe('mocked'));
