import {test,expect} from 'vitest';
import {run as left} from './left';
import {run as right} from './right';
test('left',()=>expect(left()).toBe(1));
test('right',()=>expect(right()).toBe(1));
test('plain one',()=>expect(9).toBe(9));
test('plain two',()=>expect(9).toBe(9));
