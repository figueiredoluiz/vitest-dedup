// Owned subprocesses for revert proofs: operational failures are never assertion evidence.
import { spawn } from 'node:child_process'
import { closeSync, mkdtempSync, openSync, writeSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

/** Refuse unsupported execution before opening logs, spawning commands or changing sources. */
export function requireMacOS() {
  if (process.platform !== 'darwin') throw new Error('revert-proof execution is supported only on macOS')
}

/** Stop the command's owned process tree before its caller restores a mutated source file. */
export async function stopTree(child, { kill = process.kill } = {}) {
  if (child.pid === undefined) return
  try { kill(-child.pid, 'SIGKILL') }
  catch (error) { if (error.code !== 'ESRCH') throw error }
}

/**
 * Run a command to completion, retaining bounded output in a temporary log. Only completed exit
 * codes become booleans; spawn errors, signals, deadlines and cancellation reject the proof.
 * @param {string[]} command Executable and literal arguments.
 * @param {object} options Abort signal, time/output limits, and diagnostic sink.
 * @returns {Promise<boolean>} Whether the completed command exited zero.
 */
export async function runCommand(command, {
  signal, timeoutMs = 15 * 60 * 1000, maxOutputBytes = 8 * 1024 * 1024,
  print = console.error,
} = {}) {
  requireMacOS()
  signal?.throwIfAborted()
  const log = join(mkdtempSync(join(tmpdir(), 'revert-proof-command-')), 'output.log')
  const fd = openSync(log, 'w')
  try {
    return await new Promise((resolve, reject) => {
      const child = spawn(command[0], command.slice(1), {
        stdio: ['ignore', 'pipe', 'pipe'], detached: true,
      })
      let failure = null
      let stopping = null
      let bytes = 0
      /** Begin termination exactly once, retaining a cleanup error as an operational failure. */
      const cancel = (error) => {
        failure ??= error
        stopping ??= stopTree(child).catch((error) => { failure = error })
      }
      /** Keep the log bounded even if the child writes faster than it can be terminated. */
      const output = (data) => {
        const keep = Math.min(data.length, Math.max(0, maxOutputBytes - bytes))
        try { if (keep > 0) writeSync(fd, data.subarray(0, keep)) }
        catch (error) { cancel(error) }
        bytes += data.length
        if (bytes > maxOutputBytes) cancel(new Error(`output exceeded ${maxOutputBytes} bytes`))
      }
      child.stdout.on('data', output)
      child.stderr.on('data', output)
      const abort = () => cancel(signal.reason)
      signal?.addEventListener('abort', abort, { once: true })
      const timer = setTimeout(() => cancel(new Error(`command timed out after ${timeoutMs} ms`)), timeoutMs)
      child.once('error', cancel)
      // Descendants may inherit the output pipes, so close can wait for them indefinitely.
      // Start cleanup at exit, while close remains the boundary for draining captured output.
      child.once('exit', () => {
        stopping ??= stopTree(child).catch((error) => { failure = error })
      })
      child.once('close', async (code, endedBy) => {
        clearTimeout(timer)
        signal?.removeEventListener('abort', abort)
        try {
          // A POSIX descendant can outlive a normally exiting parent. Its process group remains
          // ours until cleanup, including when that parent was killed by an external signal.
          await (stopping ?? stopTree(child))
          if (endedBy !== null) failure ??= new Error(`command terminated by ${endedBy}`)
          if (failure !== null || code === null) {
            reject(new Error(`${failure?.message ?? failure ?? 'command did not complete'}; log: ${log}`))
          } else {
            if (code !== 0) print(`Command exited ${code}: ${command.join(' ')}; log: ${log}`)
            resolve(code === 0)
          }
        } catch (error) { reject(error) }
      })
    })
  } finally { closeSync(fd) }
}
