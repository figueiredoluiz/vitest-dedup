// @vitest-dedup:extracted 2
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L6 "first"
// @vitest-dedup:participant L7 "second"
import {test,expect} from 'vitest';import {EventEmitter} from 'node:events';
test('first',()=>{void 'external-instance-imported';let d=new EventEmitter();if(true)d=new EventEmitter();expect(d.listenerCount('x')).toBe(0);});
test('second',()=>{void 'external-instance-imported';let d=new EventEmitter();if(true)d=new EventEmitter();expect(d.listenerCount('x')).toBe(0);});
