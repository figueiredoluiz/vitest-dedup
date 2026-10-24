// A Vite plugin that lets a fixture importing a module that does not exist still be collected.
//
// Fixtures are written for static analysis: `./cart`, `./math` and the like were never written.
// An import Vite cannot resolve becomes a stub module whose every export is `undefined`, and every
// read of an export reports itself through `globalThis.__vitestDedupStubRead`. The probe runner
// counts reads that happen while a file is being collected, and the gate does not compare such a
// file: its collection may depend on a value the stub made up. A file whose collection read no
// stub collected exactly what it would with the real module, because nothing it collected came
// from that module.
//
// A read is only observable as a named property access. A file that takes the module whole
// (`import * as`, `export * from`, `import()`) can inspect it without one (`'x' in ns`,
// `Object.keys(ns)`), so a stub such a file imports reports a read as soon as it loads. Which
// imports those are is read from the module's AST after the TypeScript transform, so it covers every
// spelling the parser accepts; a module that does not parse takes every stub whole.
//
// `then` is the one name the runtime reads too: awaiting an import reads the exports' `then`. A read
// of `then` reports only when code under `root` made it directly, which is how a fixture's read,
// directly or through a re-export, differs from the runtime's.
const PREFIX = '\0vitest-dedup-stub:'
const WHOLE = 'whole:'
const NAMED = 'named:'

/** `path` with forward slashes. */
const slashed = (path) => path.replaceAll('\\', '/')

/** Every node in an ESTree `ast`, depth first. */
function* nodes(node) {
  if (Array.isArray(node)) {
    for (const child of node) yield* nodes(child)
  } else if (node && typeof node === 'object') {
    if (typeof node.type === 'string') yield node
    for (const value of Object.values(node)) if (value && typeof value === 'object') yield* nodes(value)
  }
}

/**
 * The specifiers a module takes whole, from its ESTree `ast`: namespace imports, imports with no
 * specifiers (for their side effects), `export * from` and literal `import()`. `true` when any stub may be taken whole: a computed `import()`, or no
 * AST.
 */
export function wholeModuleImports(ast) {
  if (!ast) return true
  const specifiers = new Set()
  for (const node of nodes(ast)) {
    if (node.type === 'ImportDeclaration') {
      // No specifiers (`import './x'`, `import {} from './x'`) imports the module for what it does
      // when it loads, which a stub cannot stand in for.
      if (node.specifiers.length === 0 || node.specifiers.some((s) => s.type === 'ImportNamespaceSpecifier')) {
        specifiers.add(node.source.value)
      }
    } else if (node.type === 'ExportAllDeclaration' || (node.type === 'ExportNamedDeclaration' && node.source && node.specifiers.length === 0)) {
      specifiers.add(node.source.value)
    } else if (node.type === 'ImportExpression') {
      if (node.source.type !== 'Literal' || typeof node.source.value !== 'string') return true
      specifiers.add(node.source.value)
    }
  }
  return specifiers
}

/**
 * The stub module for `specifier`: a null-prototype export object whose missing keys report. A
 * `then` read reports only when the frame that called the stub's `get` is code under `root`: a
 * fixture or a helper re-exporting the stub. The runtime awaiting the import calls it from Vite's
 * module runner or the microtask queue.
 */
function stubSource(specifier, whole, root) {
  const name = JSON.stringify(specifier)
  return `${whole ? `globalThis.__vitestDedupStubRead?.(${name}, '*')\n` : ''}const calledFromCodeUnderTest = () => {
  const limit = Error.stackTraceLimit
  Error.stackTraceLimit = 64
  const frames = String(new Error().stack).split('\\n')
  Error.stackTraceLimit = limit
  // This function and the trap are both stub frames; the caller is the first frame after them.
  const own = frames.findIndex((frame) => frame.includes(${JSON.stringify(PREFIX.slice(1))}))
  const caller = own < 0 ? undefined : frames.slice(own).find((frame) => !frame.includes(${JSON.stringify(PREFIX.slice(1))}))
  // Windows frames print backslashes; \`root\` is compared in forward slashes, as Vite writes ids.
  const location = caller?.replaceAll('\\\\', '/')
  return location !== undefined && location.includes(${JSON.stringify(`${root}/`)}) && !/\\/node_modules\\//.test(location)
}
Object.setPrototypeOf(__vite_ssr_exports__, new Proxy(Object.create(null), {
  get(_, key) {
    if (typeof key !== 'symbol' && (key !== 'then' || calledFromCodeUnderTest())) globalThis.__vitestDedupStubRead?.(${name}, key)
    return undefined
  },
}))
`
}

/**
 * Two plugins: the resolver runs before Vite's own (`enforce: 'pre'`) so an unresolvable import
 * becomes a stub instead of an error; the inspector runs after the TypeScript transform, where
 * `this.parse` reads the module as it will run.
 *
 * @param {string} directory - only imports from files under this directory are stubbed.
 * @returns {import('vite').Plugin[]}
 */
export function missingModuleStubs(directory) {
  // Vite ids use forward slashes on every platform; \`path.resolve\` gives backslashes on Windows.
  const root = slashed(directory).replace(/\/+$/, '')
  // Under the root: the root itself or below it, never a sibling that shares its name as a prefix.
  const inside = (path) => slashed(path) === root || slashed(path).startsWith(`${root}/`)
  const whole = new Map()
  const resolver = {
    name: 'vitest-dedup-missing-module-stubs',
    enforce: 'pre',
    async resolveId(id, importer, options) {
      if (id.startsWith(PREFIX) || !importer || !inside(importer)) return null
      const resolved = await this.resolve(id, importer, { ...options, skipSelf: true })
      if (resolved) return null
      // An importer never inspected (or whose inspection failed) takes every stub whole.
      const taken = whole.get(importer) ?? true
      return PREFIX + (taken === true || taken.has(id) ? WHOLE : NAMED) + id
    },
    load(id) {
      if (!id.startsWith(PREFIX)) return null
      const rest = id.slice(PREFIX.length)
      return rest.startsWith(WHOLE) ? stubSource(rest.slice(WHOLE.length), true, root) : stubSource(rest.slice(NAMED.length), false, root)
    },
  }
  const inspector = {
    name: 'vitest-dedup-whole-module-imports',
    transform(code, id) {
      if (!inside(id)) return null
      let ast = null
      try {
        ast = this.parse(code)
      } catch {
        // Recorded as `true` below: every stub this module imports is taken whole.
      }
      whole.set(id, wholeModuleImports(ast))
      return null
    },
  }
  return [resolver, inspector]
}
