// RECALL config-binding case: TypeScript-erased wrappers, a multiline type-only import, and one
// stable local config binding preserve the ambient-global project that makes these registrations
// visible.
//
// Named weakening: changing the exported `const` to `let`, adding another use, or replacing the
// base's suffix with a runtime expression must make the project unreadable, not guess globals on.
test('first', () => expect(1).toBe(1))
test('second', () => expect(2).toBe(2))
