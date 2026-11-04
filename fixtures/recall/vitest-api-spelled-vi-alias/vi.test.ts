import { test, expect } from 'vitest';
import { value } from './dep';
import { vi } from 'vitest';
vi.mock('./dep', () => ({value:'mocked'}));
test('reads dependency', () => expect(value).toBe('mocked'));
