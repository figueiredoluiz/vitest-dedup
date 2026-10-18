#!/usr/bin/env node
// Local named-function documentation pre-check. CodeRabbit owns its separate review metric.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { scopeDiff } from '../dev/verify.mjs'
export { scopeDiff }
import { masked } from './lib/rust-test-modules.mjs'
import { parse } from '@babel/parser'

export const MINIMUM = 80

/** Read added/edited line ranges from Git's zero-context, non-renaming diff. */
export function changedLines(diff) {
  if (diff.trim() && !diff.startsWith('diff --git ')) throw new Error('Malformed Git diff')
  const files = new Map()
  let path, deleted = false
  for (const line of diff.split('\n')) {
    if (line.startsWith('diff --git ')) { path = undefined; deleted = false }
    if (line.startsWith('+++ ')) {
      path = line.slice(4).replace(/^b\//, '')
      // A deleted file adds no lines; its hunk is skipped rather than rejected as malformed.
      if (path === '/dev/null') { path = undefined; deleted = true; continue }
      if (path.startsWith('"') || path.startsWith('/') || path.split('/').includes('..')) throw new Error('Unsupported diff path')
      files.set(path, [])
    }
    if (line.startsWith('@@ ')) {
      const match = /^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@/.exec(line)
      if (deleted && match) continue
      if (!path || !match) throw new Error('Malformed diff hunk')
      const start = Number(match[1]), count = Number(match[2] ?? 1)
      // A deletion touches the boundary on both sides, including removed documentation.
      files.get(path).push([Math.max(1, start), Math.max(1, count ? start + count - 1 : start + 1)])
    }
  }
  return files
}

/** Locate attached nonempty documentation and its attribute/comment preamble. */
function documentation(source, line, rust, offset) {
  let prefix = (offset === undefined ? source.split('\n').slice(0, line - 1).join('\n') : source.slice(0, offset)).trimEnd()
  if (rust) prefix = prefix.replace(/(?:\s*#\[[^\]]*\])+\s*$/, '').trimEnd()
  else prefix = prefix.replace(/\bexport(?:\s+default)?\s*$/, '').trimEnd()
  const blockStart = prefix.lastIndexOf('/**')
  const block = /\/\*\*(?!\*)([\s\S]*?)\*\/$/.exec(prefix.slice(blockStart))
  const lineDoc = rust && /(?:^|\n)\s*\/\/\/(?!\/)[^\n]*\S[^\n]*$/.test(prefix)
  const documented = Boolean(lineDoc || block?.[1].replace(/[\s*]/g, ''))
  let start = Math.min(line, prefix ? prefix.split('\n').length + (lineDoc ? 0 : 1) : 1)
  if (block) start = Math.min(start, prefix.slice(0, blockStart).split('\n').length)
  const lines = source.split('\n')
  while (start > 1 && /^\s*\/\/\//.test(lines[start - 2])) start--
  return { documented, start }
}

/** Parse named JS/TS functions without mistaking regexes, JSX or type bodies for code blocks. */
function javascriptFunctions(path, source) {
  const ast = parse(source, { sourceType: 'unambiguous', plugins: [...(/\.[cm]?tsx?$/.test(path) ? ['typescript'] : []), ...(/\.[jt]sx$/.test(path) ? ['jsx'] : [])] })
  const stack = [{ node: ast }], result = []
  while (stack.length) {
    const { node, parent, variable } = stack.pop()
    if (['FunctionDeclaration', 'FunctionExpression', 'TSDeclareFunction'].includes(node.type) && node.id) {
      const line = node.loc.start.line
      let doc = documentation(source, line, false, node.start)
      if (!doc.documented && node.type === 'FunctionExpression' && parent?.type === 'VariableDeclarator' && parent.init === node && variable) {
        doc = documentation(source, variable.loc.start.line, false, variable.start)
      }
      result.push({ name: node.id.name, line, end: node.loc.end.line, ...doc })
    }
    for (const value of Object.values(node)) {
      for (const child of Array.isArray(value) ? value : [value]) if (child?.type) stack.push({ node: child, parent: node, variable: node.type === 'VariableDeclaration' ? node : variable })
    }
  }
  return result.sort((a, b) => a.line - b.line)
}

/** Match a balanced parameter/body delimiter in tokens that already exclude comments/strings. */
function closing(tokens, start, opener, closer) {
  let depth = 0
  for (let i = start; i < tokens.length; i++) {
    if (tokens[i].value === opener) depth++
    if (tokens[i].value === closer && --depth === 0) return i
  }
  throw new Error('Unbalanced function delimiters')
}

/** Enumerate named Rust functions/methods and JS/TS function declarations, including nested ones. */
export function functionsIn(path, source) {
  if (!path.endsWith('.rs')) return javascriptFunctions(path, source)
  let previous = 0, lineNumber = 1
  const tokens = [...masked(source).matchAll(/[A-Za-z_$][\w$]*|[^\s]/g)].map(match => {
    lineNumber += source.slice(previous, match.index).split('\n').length - 1
    previous = match.index
    return { value: match[0], line: lineNumber }
  })
  const result = []
  for (let i = 0; i < tokens.length; i++) {
    if (tokens[i].value !== 'fn') continue
    const name = tokens[i + 1]?.value === '*' ? i + 2 : i + 1
    if (!/^[A-Za-z_$][\w$]*$/.test(tokens[name]?.value ?? '')) continue
    let params = name + 1
    while (params < tokens.length && tokens[params].value !== '(') params++
    let end = closing(tokens, params, '(', ')') + 1
    while (end < tokens.length && !['{', ';'].includes(tokens[end].value)) end++
    if (end === tokens.length) throw new Error(`Cannot locate body of ${tokens[name].value}`)
    if (tokens[end].value === '{') end = closing(tokens, end, '{', '}')
    const line = tokens[i].line
    result.push({ name: tokens[name].value, line, end: tokens[end].line, ...documentation(source, line, true) })
  }
  return result
}

/** Measure only supported named functions intersecting the supplied immutable diff scope. */
export function coverage(root, diff) {
  const functions = []
  for (const [path, ranges] of changedLines(diff)) {
    if (!/\.(?:rs|[cm]?[jt]sx?)$/.test(path)) continue
    const source = readFileSync(resolve(root, path), 'utf8')
    for (const fn of functionsIn(path, source)) {
      if (ranges.some(([start, end]) => start <= fn.end && end >= fn.start)) functions.push({ ...fn, path })
    }
  }
  const documented = functions.filter(fn => fn.documented).length
  const percent = functions.length ? documented * 100 / functions.length : 100
  return { functions, documented, percent, passed: percent >= MINIMUM }
}

/** Run the local guard; a supplied diff carries original Git scope into exported hook snapshots. */
export function main(args = process.argv.slice(2), root = process.cwd()) {
  if (args.length && (args.length !== 2 || args[0] !== '--diff-file')) throw new Error('Usage: check-docstrings.mjs [--diff-file PATH]')
  const diff = args.length ? readFileSync(args[1], 'utf8') : scopeDiff(root)
  const result = coverage(root, diff)
  console.log(`Named-function docstrings: ${result.documented}/${result.functions.length} (${result.percent.toFixed(2)}%; minimum ${MINIMUM}%)${result.functions.length ? '' : ' — no touched named functions'}`)
  for (const fn of result.functions.filter(fn => !fn.documented)) console.log(`Missing: ${fn.path}:${fn.line} ${fn.name}`)
  if (!result.passed) throw new Error('Touched named-function documentation is below the required floor')
}

/** Convert validation errors into a failing CLI status; directly testable without a child process. */
export function cli(args, root) {
  try { main(args, root); return 0 } catch (error) { console.error(error.message); return 1 }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) process.exitCode = cli()
