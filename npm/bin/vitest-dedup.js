#!/usr/bin/env node
// Thin entry point. Every decision lives in `../lib/launcher.mjs` so it can be unit-tested without
// spawning a process; this file exists only because `bin` needs a real path with a shebang.
import { run } from '../lib/launcher.mjs'

process.exitCode = await run(process.argv.slice(2))
