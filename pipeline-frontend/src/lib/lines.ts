// Client-side mirrors of the backend's source limits, kept in one place so the
// guards in the editor / compile / run paths can't drift from the server.

// Line counting matches Rust `str::lines().count()`: split on '\n' and ignore a
// single trailing newline, so "a\nb\n" is 2 lines.
export const MAX_LINE_LIMIT = 100;

// Byte counting matches Rust `String::len()` (UTF-8 byte length, not code
// points). Mirrors the backend `MAX_SOURCE_BYTES` default.
export const MAX_SOURCE_BYTES = 16384;

export const LINE_LIMIT_MESSAGE = `Code exceeds the ${MAX_LINE_LIMIT}-line limit`;
export const BYTE_LIMIT_MESSAGE = `Code exceeds the ${MAX_SOURCE_BYTES}-byte (16 KB) limit`;

export function countLines(source: string): number {
  if (source.length === 0) return 0;
  const lines = source.split('\n');
  if (lines[lines.length - 1] === '') {
    lines.pop();
  }
  return lines.length;
}

export function exceedsLineLimit(source: string): boolean {
  return countLines(source) > MAX_LINE_LIMIT;
}

/** UTF-8 byte length, matching the backend's `String::len()`. */
export function countBytes(source: string): number {
  return new TextEncoder().encode(source).length;
}

export function exceedsByteLimit(source: string): boolean {
  return countBytes(source) > MAX_SOURCE_BYTES;
}
