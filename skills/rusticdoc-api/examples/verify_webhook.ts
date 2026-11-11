// A rusticdoc webhook receiver that checks each delivery's signature.
//
//   RUSTICDOC_SIGNING_KEY=<hex> PORT=8000 node verify_webhook.ts
//
// Node 23.6 or newer runs TypeScript directly; no dependencies. RUSTICDOC_SIGNING_KEY
// is the hex that `rusticdoc webhook-secret` prints for the calling key, run where
// RUSTICDOC_WEBHOOK_SECRET is set. A verified result is saved in ./received.
import { createServer, type IncomingHttpHeaders } from 'node:http';
import { createHmac, timingSafeEqual } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

const key = Buffer.from(process.env.RUSTICDOC_SIGNING_KEY ?? '', 'hex');
const MAX_AGE_SECONDS = 300;
const seen = new Set<string>(); // request ids already handled; deliveries can repeat

function header(headers: IncomingHttpHeaders, name: string): string {
  const value = headers[name];
  return Array.isArray(value) ? (value[0] ?? '') : (value ?? '');
}

export function verified(headers: IncomingHttpHeaders, body: Buffer): boolean {
  const timestamp = header(headers, 'x-rusticdoc-timestamp');
  const event = header(headers, 'x-rusticdoc-event');
  const requestId = header(headers, 'x-request-id');
  const signature = header(headers, 'x-rusticdoc-signature');
  if (!/^\d+$/.test(timestamp) || !event || !requestId || !signature.startsWith('v1=')) return false;
  if (Math.abs(Date.now() / 1000 - Number(timestamp)) > MAX_AGE_SECONDS) return false;
  // Over the raw body bytes: a parsed or re-encoded body won't verify.
  const expected = createHmac('sha256', key).update(`${timestamp}.${event}.${requestId}.`).update(body).digest('hex');
  const a = Buffer.from(signature.slice(3));
  const b = Buffer.from(expected);
  return a.length === b.length && timingSafeEqual(a, b);
}

createServer((req, res) => {
  const chunks: Buffer[] = [];
  req.on('data', (chunk: Buffer) => chunks.push(chunk));
  req.on('end', () => {
    const body = Buffer.concat(chunks);
    const ok = verified(req.headers, body);
    const event = header(req.headers, 'x-rusticdoc-event');
    const requestId = header(req.headers, 'x-request-id');
    console.log(ok ? 'verified' : 'REJECTED', event, requestId, body.length);
    if (ok && !seen.has(requestId)) {
      seen.add(requestId);
      if (event === 'render.completed') {
        const name = /filename="([^"/\\]+)"/.exec(header(req.headers, 'content-disposition'))?.[1] ?? requestId;
        mkdirSync('received', { recursive: true });
        writeFileSync(join('received', name), body);
      } else {
        console.log('failure:', body.toString('utf8'));
      }
    }
    // Any 2xx stops the retries; anything else asks for another attempt.
    res.statusCode = ok ? 204 : 401;
    res.end();
  });
}).listen(Number(process.env.PORT ?? 8000));
