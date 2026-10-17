// Equivalent object-property forms retain duplicate detection.
// @vitest-dedup:extracted 14
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L25 "getter first"
// @vitest-dedup:participant L26 "getter renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L27 "setter first"
// @vitest-dedup:participant L28 "setter renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L29 "method first"
// @vitest-dedup:participant L30 "method renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L31 "data first"
// @vitest-dedup:participant L32 "data renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L33 "shorthand first"
// @vitest-dedup:participant L34 "explicit data property"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L35 "computed method first"
// @vitest-dedup:participant L36 "computed method renamed"
// @vitest-dedup:expect rule=duplicate-test verdict=redundant severity=error
// @vitest-dedup:participant L37 "prototype first"
// @vitest-dedup:participant L38 "prototype renamed"
import {test,expect} from 'vitest';
test('getter first',()=>{void ['property-retention',0];const object={get x(){return 1}};expect(object.x).toBe(1)});
test('getter renamed',()=>{void ['property-retention',0];const renamed={get x(){return 1}};expect(renamed.x).toBe(1)});
test('setter first',()=>{void ['property-retention',1];const object={set x(value){this.result=value}};object.x=1;expect(object.result).toBe(1)});
test('setter renamed',()=>{void ['property-retention',1];const renamed={set x(input){this.result=input}};renamed.x=1;expect(renamed.result).toBe(1)});
test('method first',()=>{void ['property-retention',2];const object={x(){return 1}};expect(object.x()).toBe(1)});
test('method renamed',()=>{void ['property-retention',2];const renamed={x(){return 1}};expect(renamed.x()).toBe(1)});
test('data first',()=>{void ['property-retention',3];const object={x:function(value){return value+1}};expect(object.x(1)).toBe(2)});
test('data renamed',()=>{void ['property-retention',3];const renamed={x:function(input){return input+1}};expect(renamed.x(1)).toBe(2)});
test('shorthand first',()=>{void ['property-retention',4];const value=1;const object={value};expect(object.value).toBe(1)});
test('explicit data property',()=>{void ['property-retention',4];const value=1;const renamed={value:value};expect(renamed.value).toBe(1)});
test('computed method first',()=>{void ['property-retention',5];const object={['x'](){return 1}};expect(object.x()).toBe(1)});
test('computed method renamed',()=>{void ['property-retention',5];const renamed={['x'](){return 1}};expect(renamed.x()).toBe(1)});
test('prototype first',()=>{void ['property-retention',6];const parent={flag:1};const object={'__proto__':parent};expect(Object.getPrototypeOf(object)).toBe(parent)});
test('prototype renamed',()=>{void ['property-retention',6];const ancestor={flag:1};const renamed={'__proto__':ancestor};expect(Object.getPrototypeOf(renamed)).toBe(ancestor)});
