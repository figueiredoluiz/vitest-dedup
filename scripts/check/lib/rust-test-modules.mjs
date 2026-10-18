// Splits a Rust source file into its production half and its `#[cfg(test)] mod` half.
//
// This exists for one consumer -- `check-duplication.mjs` -- and for one reason: jscpd has no
// notion of `#[cfg(test)]`, so a duplication figure measured over `src/` as written is dominated
// by test setup. Roughly half of this project's duplicated Rust lines live inside inline test
// modules, where repetition is the house style (explicit fixtures over shared helpers,
// deliberately). Mixing the two halves produces a number nobody can
// act on, so the gate measures them separately and this module is how they are separated.
//
// Why a scanner and not a parser: the only question asked of the source is where a
// `#[cfg(test)] mod ... { ... }` item starts and ends, which needs brace matching that is not
// fooled by braces inside strings and comments. `masked()` answers exactly that and nothing more.
// A real Rust parser would be the right tool for a bigger question; it is not needed for this one,
// and `splitRustTestModules` asserts a partition invariant that would catch it going wrong.
//
// What it does NOT strip, deliberately: `#[cfg(test)]` on non-`mod` items. The fingerprint module has
// three -- a const, a `thread_local!`, and an `#[inline(never)]` fn -- which together are the
// recursion-depth instrumentation. They are a dozen lines, they are not test *setup*, and
// recognizing arbitrary attributed items is the point where a scanner stops being honest. They
// stay in the production half and are counted there.

/**
 * Returns `source` with every comment body and string/char literal body replaced by spaces,
 * preserving length and newlines so that every index into the result is also an index into
 * `source`.
 *
 * Handles: line comments, nested block comments (Rust nests them), `"..."` with backslash
 * escapes, raw strings `r"..."` / `r#"..."#` at any hash count, byte and C string prefixes
 * (`b`, `br`, `c`, `cr`), and char literals. Lifetimes (`'a`) are distinguished from char
 * literals by looking for the closing quote, since `'{'` is a legal char literal and a lifetime
 * is not followed by one.
 */
export function masked(source) {
  // `split('')`, not `Array.from`: the latter iterates by code point, so a single astral
  // character (there is a literal emoji in `reporters/jsonl.rs`'s adversarial tests) would make
  // every subsequent array index disagree with the string index the scanner computes, and the
  // mask would slide. That bug shipped once in this function and the symptom was a
  // string-literal brace surviving 800 lines later.
  const out = source.split('')
  const blank = (start, end) => {
    for (let i = start; i < end && i < out.length; i += 1) {
      if (out[i] !== '\n') out[i] = ' '
    }
  }
  let i = 0
  while (i < source.length) {
    const two = source.slice(i, i + 2)
    if (two === '//') {
      let end = source.indexOf('\n', i)
      if (end === -1) end = source.length
      blank(i, end)
      i = end
      continue
    }
    if (two === '/*') {
      let depth = 1
      let j = i + 2
      while (j < source.length && depth > 0) {
        if (source.slice(j, j + 2) === '/*') {
          depth += 1
          j += 2
        } else if (source.slice(j, j + 2) === '*/') {
          depth -= 1
          j += 2
        } else {
          j += 1
        }
      }
      blank(i, j)
      i = j
      continue
    }
    // Raw / byte / C string prefixes, longest first so `br#` is not read as `b` then `r#`.
    const raw = /^(?:b|c)?r(#*)"/.exec(source.slice(i, i + 12))
    if (raw && (i === 0 || !/[\w]/.test(source[i - 1]))) {
      const terminator = `"${raw[1]}`
      const bodyStart = i + raw[0].length
      let end = source.indexOf(terminator, bodyStart)
      end = end === -1 ? source.length : end + terminator.length
      blank(i, end)
      i = end
      continue
    }
    // Non-raw strings, including the `b"..."` / `c"..."` prefixed forms: the prefix character is
    // an ordinary identifier char to the scanner, and the quote that follows starts the literal.
    if (source[i] === '"') {
      let j = i + 1
      while (j < source.length) {
        if (source[j] === '\\') {
          j += 2
          continue
        }
        if (source[j] === '"') {
          j += 1
          break
        }
        j += 1
      }
      blank(i, j)
      i = j
      continue
    }
    if (source[i] === "'") {
      // `'a'`, `'\n'`, `'\u{1f}'` are char literals; `'a` followed by anything else is a lifetime.
      const rest = source.slice(i, i + 12)
      const char = /^'(?:\\(?:u\{[0-9a-fA-F]{1,6}\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'/.exec(rest)
      if (char) {
        blank(i, i + char[0].length)
        i += char[0].length
        continue
      }
      i += 1
      continue
    }
    i += 1
  }
  return out.join('')
}

/**
 * Finds the `[start, end)` character ranges of every `#[cfg(test)] mod ...` item in `source`,
 * each already widened to whole lines.
 */
function findTestModuleRanges(source) {
  const view = masked(source)
  const ranges = []
  const attribute = /#\[\s*cfg\s*\(\s*test\s*\)\s*\]/g
  let match
  while ((match = attribute.exec(view)) !== null) {
    if (ranges.some((range) => match.index >= range.start && match.index < range.end)) continue
    // Skip any further attributes stacked between `#[cfg(test)]` and the item it applies to.
    let cursor = attribute.lastIndex
    for (;;) {
      const next = /^(\s*#\[[^\]]*\])/.exec(view.slice(cursor))
      if (!next) break
      cursor += next[0].length
    }
    // `r#` because `mod r#tests { … }` is a legal item and `\w` does not match `#`: without the
    // prefix the item regex declines, `findTestModuleRanges` returns nothing, and every line of
    // the module lands in the production half -- the gate then measures the wrong scope and
    // reports a number for it rather than failing.
    const item = /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:r#)?\w+\s*([;{])/.exec(view.slice(cursor))
    if (!item) continue // `#[cfg(test)]` on a const, fn, thread_local!, ... -- left in production.
    const opener = cursor + item[0].length - 1
    let end
    if (item[1] === ';') {
      end = opener + 1
    } else {
      let depth = 0
      let j = opener
      for (; j < view.length; j += 1) {
        if (view[j] === '{') depth += 1
        else if (view[j] === '}') {
          depth -= 1
          if (depth === 0) break
        }
      }
      if (depth !== 0) {
        throw new Error(
          `unbalanced braces after a #[cfg(test)] mod at offset ${match.index}; ` +
            'the brace scanner cannot see the end of the module, so the split would be wrong',
        )
      }
      end = j + 1
    }
    // Widen to whole lines: the mirrors are line-oriented, and jscpd reports line ranges.
    const lineStart = source.lastIndexOf('\n', match.index) + 1
    let lineEnd = source.indexOf('\n', end)
    lineEnd = lineEnd === -1 ? source.length : lineEnd + 1
    ranges.push({ start: lineStart, end: lineEnd })
  }
  return ranges
}

/**
 * Splits a Rust source into a production half and a test half.
 *
 * Both halves are compacted, not blanked. Blanking would keep line numbers aligned but would
 * inflate jscpd's denominator with thousands of empty lines and understate the percentage, which
 * is the one number this gate exists to report. The line numbers are recovered instead:
 * `lineMap[i]` is the original 1-based line of mirror line `i`, so a clone reported against the
 * mirror can be printed at its real location.
 *
 * @param {string} source
 * @returns {{
 *   production: { text: string, lineMap: number[] },
 *   tests: { text: string, lineMap: number[] },
 *   adjacencyJoins: number,
 * }}
 */
export function splitRustTestModules(source) {
  const ranges = findTestModuleRanges(source).sort((a, b) => a.start - b.start)
  const lines = source.split('\n')
  // A trailing newline yields a final empty element; it is not a line.
  if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()

  const lineOffsets = []
  let offset = 0
  for (const line of lines) {
    lineOffsets.push(offset)
    offset += line.length + 1
  }

  const isTest = lines.map((_, index) => {
    const start = lineOffsets[index]
    return ranges.some((range) => start >= range.start && start < range.end)
  })

  const half = (wanted) => {
    const text = []
    const lineMap = []
    for (let index = 0; index < lines.length; index += 1) {
      if (isTest[index] !== wanted) continue
      text.push(lines[index])
      lineMap.push(index + 1)
    }
    return { text: text.length > 0 ? `${text.join('\n')}\n` : '', lineMap }
  }

  const production = half(false)
  const tests = half(true)

  // The partition invariant. If the scanner mismatched a brace or a range overlapped, this is
  // where it shows up -- before a wrong number is reported as a measurement.
  if (production.lineMap.length + tests.lineMap.length !== lines.length) {
    throw new Error(
      `split lost lines: ${production.lineMap.length} + ${tests.lineMap.length} != ${lines.length}`,
    )
  }

  // How often removing a test module made two pieces of production code adjacent that were not
  // adjacent in the source. Every such join is a chance for jscpd to see a clone that does not
  // exist in the real file, so the gate reports the count rather than assuming it is zero.
  let adjacencyJoins = 0
  for (const range of ranges) {
    const after = source.slice(range.end)
    if (after.trim().length > 0) adjacencyJoins += 1
  }

  return { production, tests, adjacencyJoins }
}
