import { test } from 'node:test';
import assert from 'node:assert/strict';

import { readBoundedText } from '../src/request-body.js';

const encode = (text) => new TextEncoder().encode(text);
const requestWith = (body) => new Request('https://c.example/v1/ping', {
  method: 'POST',
  body,
  duplex: 'half',
});

function chunkedRequest(chunks, cancel = () => {}) {
  let index = 0;
  return requestWith(new ReadableStream({
    pull(controller) {
      if (index === chunks.length) controller.close();
      else controller.enqueue(chunks[index++]);
    },
    cancel,
  }, { highWaterMark: 0 }));
}

test('a missing body and an empty stream read as empty text', async () => {
  for (const request of [requestWith(null), chunkedRequest([])]) {
    assert.deepEqual(await readBoundedText(request, 0), { ok: true, text: '' });
  }
});

test('the byte limit is inclusive, for a single chunk and multiple chunks', async () => {
  for (const chunks of [[encode('abcd')], [encode('a'), encode('bc'), encode('d')]]) {
    assert.deepEqual(await readBoundedText(chunkedRequest(chunks), 4), {
      ok: true, text: 'abcd',
    });
  }
});

test('a body one byte over the limit is rejected even without content-length', async () => {
  for (const chunks of [[encode('abcde')], [encode('ab'), encode('cd'), encode('e')]]) {
    let cancelled = false;
    const request = chunkedRequest(chunks, () => { cancelled = true; });
    assert.equal(request.headers.get('content-length'), null);
    assert.deepEqual(await readBoundedText(request, 4), { ok: false, reason: 'too-large' });
    assert.equal(cancelled, true);
    assert.equal(request.body.locked, false);
  }
});

test('UTF-8 sequences split between chunks decode correctly and count raw bytes', async () => {
  const bytes = encode('a☕🚀z');
  const chunks = Array.from(bytes, (byte) => Uint8Array.of(byte));
  assert.deepEqual(await readBoundedText(chunkedRequest(chunks), bytes.length), {
    ok: true, text: 'a☕🚀z',
  });
  assert.deepEqual(await readBoundedText(chunkedRequest(chunks), bytes.length - 1), {
    ok: false, reason: 'too-large',
  });
});

test('an incomplete UTF-8 sequence flushes using the same replacement behavior as Request.text', async () => {
  const bytes = Uint8Array.of(0x61, 0xf0, 0x9f);
  assert.deepEqual(await readBoundedText(chunkedRequest([bytes]), bytes.length), {
    ok: true, text: await requestWith(bytes).text(),
  });
});

test('oversized streams stop pulling and cancel without waiting for the source', async () => {
  let pulls = 0;
  let cancelled = false;
  const request = requestWith(new ReadableStream({
    pull(controller) {
      pulls += 1;
      controller.enqueue(encode('too large'));
    },
    cancel() {
      cancelled = true;
      return new Promise(() => {});
    },
  }, { highWaterMark: 0 }));
  let timer;
  try {
    const result = await Promise.race([
      readBoundedText(request, 4),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error('body reader waited for cancellation')), 1000);
      }),
    ]);
    assert.deepEqual(result, { ok: false, reason: 'too-large' });
    assert.equal(pulls, 1);
    assert.equal(cancelled, true);
    assert.equal(request.body.locked, false);
  } finally {
    clearTimeout(timer);
  }
});

test('a rejecting cancellation does not turn an oversized body into a read error', async () => {
  const request = chunkedRequest([encode('12345')], () => Promise.reject(new Error('cancel failed')));
  assert.deepEqual(await readBoundedText(request, 4), { ok: false, reason: 'too-large' });
});

test('stream errors return unreadable and release the reader', async () => {
  let pulls = 0;
  const request = requestWith(new ReadableStream({
    pull(controller) {
      if (pulls++ === 0) controller.enqueue(encode('partial'));
      else controller.error(new Error('connection interrupted'));
    },
  }, { highWaterMark: 0 }));
  assert.deepEqual(await readBoundedText(request, 1024), { ok: false, reason: 'unreadable' });
  assert.equal(request.body.locked, false);
});

test('a locked request body returns unreadable without stealing its reader', async () => {
  const request = requestWith('body');
  const reader = request.body.getReader();
  assert.deepEqual(await readBoundedText(request, 1024), { ok: false, reason: 'unreadable' });
  assert.equal(request.body.locked, true);
  reader.releaseLock();
});

test('a previously consumed body returns unreadable even after its reader is released', async () => {
  const request = requestWith('body');
  const reader = request.body.getReader();
  while (!(await reader.read()).done) { /* Consume and then unlock the stream. */ }
  reader.releaseLock();
  assert.deepEqual(await readBoundedText(request, 1024), { ok: false, reason: 'unreadable' });
});

test('non-byte chunks return unreadable and cancel the source', async () => {
  let cancelled = false;
  const request = chunkedRequest([undefined, encode('12345')], () => { cancelled = true; });
  assert.deepEqual(await readBoundedText(request, 4), { ok: false, reason: 'unreadable' });
  assert.equal(cancelled, true);
});
