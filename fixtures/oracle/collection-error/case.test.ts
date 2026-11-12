import {test,expect} from 'vitest'
/** Fail before registering a test through the helper. */
function register(){const value=missingValue;test('uncollected',()=>expect(value).toBe(1))}
register()
