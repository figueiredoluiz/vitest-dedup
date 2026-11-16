import { expect, test } from 'vitest';

test('count then entry', () => { expect(1).toBe(1); expect({ n: 1 }).toEqual({ n: 1 }); });
test('another count then entry', () => { expect(2).toBe(2); expect({ n: 2 }).toEqual({ n: 2 }); });
test('entry then count', () => { expect({ n: 1 }).toEqual({ n: 1 }); expect(1).toBe(1); });
test('another entry then count', () => { expect({ n: 2 }).toEqual({ n: 2 }); expect(2).toBe(2); });
