import { expect, test } from 'vitest'

;({ run: test.extend({}), other: 0 }).run('dot field', () => expect(12).toBe(12))
;({ other: 1, ['run']: test.extend({}) })['run']('bracket field', () => expect(13).toBe(13))
;({ run: test.extend({}), ...{ run: (_name: string, _fn: () => void) => {} } }).run('overwritten', () => {})
;({ get run() { return (_name: string, _fn: () => void) => {} } }).run('inert accessor', () => {})
