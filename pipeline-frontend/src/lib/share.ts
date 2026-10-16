// Client-side permalink state: {code, stage} encoded into the URL fragment.
// Hand-rolled (no dependency): JSON -> UTF-8 bytes -> optional deflate -> base64url.

export interface ShareState {
  code: string;
  stage: string;
}

const VERSION = '1';

function bytesToBase64Url(bytes: Uint8Array): string {
  let binary = '';
  for (let i = 0; i < bytes.length; i++) {
    binary += String.fromCharCode(bytes[i]);
  }
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function base64UrlToBytes(value: string): Uint8Array {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

type CompressionCtor = new (format: string) => { readable: ReadableStream; writable: WritableStream };

function compressionStream(): CompressionCtor | undefined {
  return (globalThis as { CompressionStream?: CompressionCtor }).CompressionStream;
}

function decompressionStream(): CompressionCtor | undefined {
  return (globalThis as { DecompressionStream?: CompressionCtor }).DecompressionStream;
}

async function deflate(bytes: Uint8Array): Promise<Uint8Array | null> {
  const Ctor = compressionStream();
  if (!Ctor) return null;
  try {
    const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new Ctor('deflate-raw'));
    return new Uint8Array(await new Response(stream).arrayBuffer());
  } catch {
    return null;
  }
}

async function inflate(bytes: Uint8Array): Promise<Uint8Array> {
  const Ctor = decompressionStream();
  if (!Ctor) throw new Error('DecompressionStream unavailable');
  const stream = new Blob([bytes as BlobPart]).stream().pipeThrough(new Ctor('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/** Encode share state into a compact, URL-safe token. */
export async function encodeShareState(state: ShareState): Promise<string> {
  const bytes = new TextEncoder().encode(JSON.stringify(state));
  const compressed = await deflate(bytes);
  if (compressed) {
    return `z${VERSION}${bytesToBase64Url(compressed)}`;
  }
  return `p${VERSION}${bytesToBase64Url(bytes)}`;
}

/** Decode a token produced by {@link encodeShareState}; returns null if invalid. */
export async function decodeShareState(token: string): Promise<ShareState | null> {
  try {
    const marker = token.charAt(0);
    const bytes = base64UrlToBytes(token.slice(2));
    const raw = marker === 'z' ? await inflate(bytes) : bytes;
    const parsed = JSON.parse(new TextDecoder().decode(raw)) as Partial<ShareState>;
    if (typeof parsed.code !== 'string') return null;
    return {
      code: parsed.code,
      stage: typeof parsed.stage === 'string' ? parsed.stage : 'tokens',
    };
  } catch {
    return null;
  }
}

/** Build a full shareable URL for the given state. */
export async function buildShareUrl(state: ShareState): Promise<string> {
  const token = await encodeShareState(state);
  const { origin, pathname } = window.location;
  return `${origin}${pathname}#s=${token}`;
}

/** Read and decode share state from the current URL fragment, if present. */
export async function readShareStateFromUrl(): Promise<ShareState | null> {
  const hash = window.location.hash;
  const match = hash.match(/[#&]s=([^&]+)/);
  if (!match) return null;
  return decodeShareState(match[1]);
}
