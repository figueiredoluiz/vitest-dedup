// Object-property descriptor identity must survive comparison.
// @vitest-dedup:extracted 12
// @vitest-dedup:not-runnable Each conflicting descriptor pair contains an intentional runtime failure.
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates object-property
// @vitest-dedup:participant L29 "getter read"
// @vitest-dedup:participant L30 "method read"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates object-property
// @vitest-dedup:participant L31 "getter data read"
// @vitest-dedup:participant L32 "function data read"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates object-property
// @vitest-dedup:participant L33 "setter read"
// @vitest-dedup:participant L34 "setter data read"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates object-property
// @vitest-dedup:participant L35 "method prototype"
// @vitest-dedup:participant L36 "function prototype"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates object-property
// @vitest-dedup:participant L37 "prototype setter"
// @vitest-dedup:participant L38 "computed own property"
// @vitest-dedup:expect rule=none verdict=distinct severity=none
// @vitest-dedup:isolates object-property
// @vitest-dedup:participant L39 "prototype shorthand"
// @vitest-dedup:participant L40 "prototype initializer"
import {test,expect} from 'vitest';
test('getter read',()=>{void 'accessor-method';const object={get x(){return 1}};expect(typeof object.x).toBe('number')});
test('method read',()=>{void 'accessor-method';const object={x(){return 1}};expect(typeof object.x).toBe('number')});
test('getter data read',()=>{void ['accessor-data'];const object={get x(){return 1}};expect(typeof object.x).toBe('number')});
test('function data read',()=>{void ['accessor-data'];const object={x:function(){return 1}};expect(typeof object.x).toBe('number')});
test('setter read',()=>{void ({setter:true});const object={set x(value){void value}};expect(typeof object.x).toBe('undefined')});
test('setter data read',()=>{void ({setter:true});const object={x:function(value){void value}};expect(typeof object.x).toBe('undefined')});
test('method prototype',()=>{void (()=>'constructibility');const object={x(){return 1}};expect(Object.hasOwn(object.x,'prototype')).toBe(false)});
test('function prototype',()=>{void (()=>'constructibility');const object={x:function(){return 1}};expect(Object.hasOwn(object.x,'prototype')).toBe(false)});
test('prototype setter',()=>{void (1+2);const parent={flag:1};const object={'__proto__':parent};expect(Object.getPrototypeOf(object)).toBe(parent)});
test('computed own property',()=>{void (1+2);const parent={flag:1};const object={['__proto__']:parent};expect(Object.getPrototypeOf(object)).toBe(parent)});
test('prototype shorthand',()=>{void ({nested:[true]});const __proto__={flag:1};const object={__proto__};expect(Object.hasOwn(object,'__proto__')).toBe(true)});
test('prototype initializer',()=>{void ({nested:[true]});const __proto__={flag:1};const object={__proto__:__proto__};expect(Object.hasOwn(object,'__proto__')).toBe(true)});
