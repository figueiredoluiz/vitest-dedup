import { test, expect } from 'vitest';
test(/** Supplies only the test title; its body must never execute. */ function alpha() { throw Error('title') }, {skip: true}, () => expect(1).toBe(1));
test(/** Supplies only the test title; its body must never execute. */ function beta() { throw Error('title') }, {todo: true}, () => expect(2).toBe(1));
