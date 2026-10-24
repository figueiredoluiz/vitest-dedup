// Imported table cells whose runtime value is not the literal beside them.
export const FAST = 'fast'
export const SLOW = 'slow'

// A `let` export can be reassigned by its own module after the importer reads its initializer.
export let LET = 'before'
export function setLet() {
  LET = 'after'
}
