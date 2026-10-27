import { test, expect } from 'vitest';
import { value } from './dep';
import { vitest } from 'vitest';
vitest.mock('./dep', () => ({value:'mocked'}));
test('reads dependency', () => expect(value).toBe('mocked'));
