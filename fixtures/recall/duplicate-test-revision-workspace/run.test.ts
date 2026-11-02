import {test,expect} from 'vitest';
import {run} from './packages/runner';
test('one',()=>expect(run()).toBe(1));
test('two',()=>expect(run()).toBe(1));
