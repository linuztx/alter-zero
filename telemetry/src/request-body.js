/** Read at most maxBytes of a UTF-8 body, including requests without a length. */
export async function readBoundedText(request, maxBytes) {
  let reader;
  try {
    if (request.bodyUsed) return { ok: false, reason: 'unreadable' };
    if (!request.body) return { ok: true, text: '' };
    reader = request.body.getReader();
    const decoder = new TextDecoder();
    let bytes = 0;
    let text = '';
    while (true) {
      const { done, value } = await reader.read();
      if (done) return { ok: true, text: text + decoder.decode() };
      if (!(value instanceof Uint8Array)) throw new TypeError('Expected a byte stream');
      bytes += value.byteLength;
      if (bytes > maxBytes) {
        // A hostile stream must not delay rejection by leaving cancel pending.
        reader.cancel().catch(() => {});
        return { ok: false, reason: 'too-large' };
      }
      text += decoder.decode(value, { stream: true });
    }
  } catch {
    reader?.cancel().catch(() => {});
    return { ok: false, reason: 'unreadable' };
  } finally {
    reader?.releaseLock();
  }
}
