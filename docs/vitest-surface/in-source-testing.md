# In-source testing


```ts
// src/add.ts
export function add(a: number, b: number) { return a + b }

if (import.meta.vitest) {
  const { it, expect } = import.meta.vitest
  it('add', () => {
    expect(add(1, 1)).toBe(2)
  })
}
```

- Gated entirely by the `if (import.meta.vitest)` guard; `test`/`it`/`expect`/etc. are pulled off
  `import.meta.vitest` itself (an injected object), not from a `'vitest'` import specifier, the
  extractor's "how did this binding get here" resolution needs a special case: a `MemberExpression`
  destructure off `import.meta.vitest` is a Vitest-global-equivalent binding, regardless of the
  file's `globals` setting.
- Governed by **`includeSource`** config: `string[]` glob list, resolved relative to `root`,
  **default `[]`** (feature is opt-in, off unless a project lists source globs, e.g.
  `includeSource: ['src/**/*.{js,ts}']`). A file only contributes in-source tests if it matches
  `includeSource` (in addition to actually containing `import.meta.vitest` blocks).
  Note this is **independent of, and additive to**, the regular `include` test-file glob, a file
  can be scanned for in-source tests without matching `include` at all, since it's not a "test
  file" in the ordinary sense (it's application source with embedded tests).
- Production exclusion is a **build-time** concern (Vite `define: { 'import.meta.vitest': 'undefined' }`
  enabling dead-code elimination), irrelevant to the static extractor except as confirmation that
  `import.meta.vitest` guards are the sole and idiomatic gate; the extractor doesn't need to model
  bundler DCE, just recognize the guard shape.

Back to the [Vitest surface index](../vitest-surface.md).
