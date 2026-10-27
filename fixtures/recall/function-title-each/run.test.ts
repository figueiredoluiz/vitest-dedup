import { test, expect } from 'vitest';
test.each([1,2])(/** Supplies only the test title; its body must never execute. */ function row() {}, n => expect(n).toBe(1));
