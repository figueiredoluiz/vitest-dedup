import { test, expect } from 'vitest';
import { value } from './dep';
import { vi, vi as tools } from 'vitest';
vi.mock('./dep', () => ({value:'mocked'}));
vi.unmock('./dep');
test('reads dependency', () => expect(value).toBe('mocked'));
