import { test, expect } from 'vitest';
import { value } from './dep';
import { vitest as tools } from 'vitest';
tools.mock('./dep', () => ({value:'mocked'}));
test('reads dependency', () => expect(value).toBe('mocked'));
