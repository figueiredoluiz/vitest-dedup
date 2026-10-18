# The module-scope ambient-state family

Mocking is not the only way a file establishes state its tests run under. A second family of
module-scope calls swaps the *conditions* rather than the *dependencies*: the timer mode, a
stubbed environment key or global, a spied return value, a redefined assertion predicate. None of
them is a registration the mock fold could hold, and none is awaited (so `module_setup`, whose
admission rule is `await`, never saw them), yet two files differing only in one of them are as
non-duplicate as two files mocking different modules.

| Member | What it establishes | Modelled |
| --- | --- | --- |
| `vi.useFakeTimers()` / `vi.useRealTimers()` | The timer mode every test in the file runs under | **Yes**, `Context::ambient_state` (`src/model.rs`), as raw module-top-level source text in source order |
| `vi.stubEnv(key, value)` | A `process.env`/`import.meta.env` key's value | **Yes**, same terms |
| `vi.stubGlobal(name, value)` | A global binding's value, the same call shape as `stubEnv` | **Yes**, same terms (no dedicated corpus case; recorded here so the choice is visible) |
| `vi.spyOn(obj, prop).mockReturnValue(v)` (and the rest of the chain) | A spied binding's return value, the ordinary way to stub one export without `vi.mock` | **Yes**, the whole chained statement is captured, chain and arguments together |
| `expect.extend({ name(…) {…} })` | What an assertion *means*, same matcher name, different predicate | **Yes**, same terms. Sharpest of the family: the body and the dependencies can be identical while the two tests assert opposite things |

Two boundary rules, both inherited from `module_setup`'s design. **Module top level only**: the
same calls inside a hook body are that hook's body, inside a test body are that body, and inside a
`describe` are not facts every test in the file inherits. **A known family, not every `vi.*` /
`expect.*` call**: `vi.mock`/`vi.unmock` stay the mock fold's territory (never also recorded here),
and ordinary program text at module scope, a helper call, a plain `const`, is not ambient state.
The payload is raw source text, deliberately: collapsing whitespace would merge values that differ
inside a string. A formatting difference therefore separates two files, which can only hide a
duplicate, while a genuinely different value never matches. An unreadable payload suppresses a
comparison rather than inventing one.

Back to the [Vitest surface index](../vitest-surface.md).
