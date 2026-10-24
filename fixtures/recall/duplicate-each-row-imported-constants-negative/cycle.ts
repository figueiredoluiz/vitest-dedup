// `X` reads a member of an exported object that an importer writes before `X` is initialized.
import { overwrite } from './cycle-writer'

export const LEVELS = { a: 'x' }
overwrite()
export const X = LEVELS.a
