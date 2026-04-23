import type { Result } from './bindings'

export interface RetryOptions {
  maxAttempts: number
  delayMs: number
  /** If returns false, bail out of the retry loop with the current error. */
  shouldRetry?: (error: string, attempt: number) => boolean
  /** Called before each attempt (1-based index), useful for progress UI. */
  onAttempt?: (attempt: number) => void
}

/**
 * Re-invoke `action` until it returns `ok` or attempts run out. The final
 * error from the last failed attempt is returned unchanged so the caller
 * can surface it.
 */
export async function withRetry<T>(
  action: () => Promise<Result<T, string>>,
  opts: RetryOptions
): Promise<Result<T, string>> {
  let lastError = 'unknown error'
  for (let attempt = 1; attempt <= opts.maxAttempts; attempt++) {
    opts.onAttempt?.(attempt)
    const result = await action()
    if (result.status === 'ok') return result
    lastError = result.error
    if (opts.shouldRetry && !opts.shouldRetry(result.error, attempt)) break
    if (attempt < opts.maxAttempts) {
      await sleep(opts.delayMs)
    }
  }
  return { status: 'error', error: lastError }
}

export function sleep(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms))
}
