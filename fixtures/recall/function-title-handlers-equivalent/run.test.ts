import { test, expect } from 'vitest';
test(/** Supplies only the test title; its body must never execute. */ function alpha() { throw Error('title must never execute') }, () => expect(1).toBe(1));
test(/** Supplies only the test title; its body must never execute. */ function beta() { return 99 }, () => expect(1).toBe(1));
