import { expect, test } from 'vitest'
import * as helper from './project-reexport'

helper.other.extend = () => () => {}
const run = (0, test.extend({}))
run('replaced builder', () => expect(false).toBe(true))

test('project namespace left', () => expect(7).toBe(7))
test('project namespace right', () => expect(7).toBe(7))
