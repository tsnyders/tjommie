// What the pairing QR holds is `tjommie://pair#<code>`. This only takes the code out of what the scanner read, or from a pasted code (which may be
// the whole link or the code alone); what the code means is the pairing protocol's business, not this package's.
const PREFIX = "tjommie://pair#";
const CODE = /^[A-Za-z0-9_-]{16,8192}$/; // base64url, within what a pairing code can be

export function codeFromScan(text: string | null): string | null {
  if (text === null) return null;
  const trimmed = text.trim();
  const code = trimmed.startsWith(PREFIX) ? trimmed.slice(PREFIX.length) : trimmed;
  return CODE.test(code) ? code : null;
}
