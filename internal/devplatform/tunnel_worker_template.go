package devplatform

const cloudflareWorkerIndexMJS = `import { DurableObject } from "cloudflare:workers";

const PROTO_VERSION = 1;
const HEADER_SIZE = 9;
const MAX_CHUNK = 256 * 1024;
const WS_OPEN = WebSocket.READY_STATE_OPEN ?? WebSocket.OPEN ?? 1;

const FLAG_FIN = 0x01;
const FLAG_TEXT = 0x02;

const FRAME_REGISTER = 0x01;
const FRAME_REGISTER_ACK = 0x02;

const FRAME_HTTP_REQ_HEAD = 0x10;
const FRAME_HTTP_REQ_DATA = 0x11;
const FRAME_HTTP_RES_HEAD = 0x12;
const FRAME_HTTP_RES_DATA = 0x13;
const FRAME_STREAM_RESET = 0x14;

const FRAME_WS_OPEN = 0x20;
const FRAME_WS_OPEN_ACK = 0x21;
const FRAME_WS_DATA = 0x22;
const FRAME_WS_CLOSE = 0x23;

const CONTROL_PING = "__devbase_ping__";
const CONTROL_PONG = "__devbase_pong__";

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

function encodeFrame(type, flags, streamId, code, payload) {
  const body = payload instanceof Uint8Array
    ? payload
    : typeof payload === "string"
      ? textEncoder.encode(payload)
      : payload
        ? textEncoder.encode(JSON.stringify(payload))
        : new Uint8Array(0);
  const out = new Uint8Array(HEADER_SIZE + body.byteLength);
  const view = new DataView(out.buffer);
  out[0] = PROTO_VERSION;
  out[1] = type;
  out[2] = flags;
  view.setUint32(3, streamId, false);
  view.setUint16(7, code, false);
  if (body.byteLength > 0) {
    out.set(body, HEADER_SIZE);
  }
  return out;
}

function decodeFrame(raw) {
  const bytes = raw instanceof ArrayBuffer ? new Uint8Array(raw) : new Uint8Array(raw.buffer, raw.byteOffset, raw.byteLength);
  if (bytes.byteLength < HEADER_SIZE) {
    throw new Error("truncated tunnel frame");
  }
  if (bytes[0] !== PROTO_VERSION) {
    throw new Error("unsupported tunnel frame version");
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  return {
    type: bytes[1],
    flags: bytes[2],
    streamId: view.getUint32(3, false),
    code: view.getUint16(7, false),
    payload: bytes.subarray(HEADER_SIZE),
  };
}

function constantTimeEqualHex(a, b) {
  if (typeof a !== "string" || typeof b !== "string" || a.length !== b.length || a.length === 0) {
    return false;
  }
  let diff = 0;
  for (let i = 0; i < a.length; i++) {
    diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  }
  return diff === 0;
}

async function verifyHandshake(request, secret) {
  if (!secret) {
    return { ok: false, status: 500, reason: "TUNNEL_SECRET is not configured" };
  }
  const clientId = (request.headers.get("X-Devbase-Client") || "").trim();
  const tsRaw = (request.headers.get("X-Devbase-Timestamp") || "").trim();
  const nonce = (request.headers.get("X-Devbase-Nonce") || "").trim();
  const sig = (request.headers.get("X-Devbase-Signature") || "").trim().toLowerCase();
  if (!clientId || !tsRaw || !nonce || !sig) {
    return { ok: false, status: 401, reason: "Missing authentication headers" };
  }
  const ts = Number.parseInt(tsRaw, 10);
  const nowSec = Math.floor(Date.now() / 1000);
  if (!Number.isFinite(ts) || Math.abs(nowSec - ts) > 60) {
    return { ok: false, status: 401, reason: "Expired handshake timestamp" };
  }
  const key = await crypto.subtle.importKey(
    "raw",
    textEncoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const msg = "v1\n" + clientId + "\n" + tsRaw + "\n" + nonce;
  const mac = await crypto.subtle.sign("HMAC", key, textEncoder.encode(msg));
  const expected = Array.from(new Uint8Array(mac))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  if (!constantTimeEqualHex(expected, sig)) {
    return { ok: false, status: 401, reason: "Invalid signature" };
  }
  return { ok: true, clientId };
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/__devbase/connect") {
      if ((request.headers.get("Upgrade") || "").toLowerCase() !== "websocket") {
        return new Response("Expected WebSocket upgrade", { status: 426 });
      }
      const auth = await verifyHandshake(request, env.TUNNEL_SECRET);
      if (!auth.ok) {
        return new Response(auth.reason, { status: auth.status });
      }
      const stub = env.TUNNEL_GATEWAY.get(env.TUNNEL_GATEWAY.idFromName("gateway"));
      return stub.fetch(request);
    }
    const stub = env.TUNNEL_GATEWAY.get(env.TUNNEL_GATEWAY.idFromName("gateway"));
    return stub.fetch(request);
  },
};

export class TunnelGateway extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair(CONTROL_PING, CONTROL_PONG));
    this.agentWs = null;
    this.agentClientId = "";
    this.httpStreams = new Map();
    this.wsPending = new Map();
    this.wsVisitors = new Map();
    this.wsInboundChunks = new Map();
    this.nextStreamId = 1;

    for (const ws of this.ctx.getWebSockets("agent")) {
      const att = ws.deserializeAttachment() || {};
      if (att.active && att.clientId && ws.readyState === WS_OPEN) {
        this.agentWs = ws;
        this.agentClientId = att.clientId;
      }
    }
    for (const ws of this.ctx.getWebSockets("visitor")) {
      const att = ws.deserializeAttachment() || {};
      if (att.streamId) {
        this.wsVisitors.set(att.streamId, ws);
      }
    }
  }

  allocStreamId() {
    const id = this.nextStreamId;
    this.nextStreamId = this.nextStreamId >= 0x7fffffff ? 1 : this.nextStreamId + 1;
    return id;
  }

  cleanupAgent(keepWs) {
    if (this.agentWs && this.agentWs !== keepWs) {
      try { this.agentWs.close(1000, "superseded"); } catch {}
    }
    if (!keepWs) {
      this.agentWs = null;
      this.agentClientId = "";
    }
    for (const streamId of [...this.httpStreams.keys()]) {
      this.failHTTPStream(streamId, "Tunnel client disconnected");
    }
    for (const [streamId, pending] of this.wsPending.entries()) {
      this.wsPending.delete(streamId);
      pending.reject(new Error("Tunnel client disconnected"));
    }
    for (const [streamId, vws] of this.wsVisitors.entries()) {
      this.wsVisitors.delete(streamId);
      this.wsInboundChunks.delete(streamId);
      try { vws.close(1001, "Tunnel disconnected"); } catch {}
    }
  }

  findVisitor(streamId) {
    let ws = this.wsVisitors.get(streamId);
    if (ws) return ws;
    for (const candidate of this.ctx.getWebSockets("visitor")) {
      const att = candidate.deserializeAttachment() || {};
      if (att.streamId === streamId) {
        this.wsVisitors.set(streamId, candidate);
        return candidate;
      }
    }
    return null;
  }

  failHTTPStream(streamId, message) {
    const st = this.httpStreams.get(streamId);
    if (!st) return;
    this.httpStreams.delete(streamId);
    if (!st.responded) {
      st.responded = true;
      st.resolve(new Response(message, { status: 502 }));
    } else if (st.writer) {
      st.writer.abort(new Error(message)).catch(() => {});
    }
  }

  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/__devbase/connect") {
      const clientId = (request.headers.get("X-Devbase-Client") || "").trim();
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);
      this.ctx.acceptWebSocket(server, ["agent"]);
      server.serializeAttachment({ clientId, active: false });
      return new Response(null, { status: 101, webSocket: client });
    }

    if (!this.agentWs || this.agentWs.readyState !== WS_OPEN) {
      return new Response("Devbase tunnel offline", { status: 502 });
    }

    const host = url.hostname.toLowerCase();
    if ((request.headers.get("Upgrade") || "").toLowerCase() === "websocket") {
      return this.handleVisitorWebSocket(request, url, host, this.agentWs);
    }
    return this.handleVisitorHTTP(request, url, host, this.agentWs);
  }

  async handleVisitorHTTP(request, url, host, agentWs) {
    const streamId = this.allocStreamId();
    const headers = [];
    for (const [k, v] of request.headers.entries()) {
      headers.push([k, v]);
    }
    const bodyless = !request.body || request.method === "GET" || request.method === "HEAD";

    let resolveResp;
    const respPromise = new Promise((resolve) => {
      resolveResp = resolve;
    });
    this.httpStreams.set(streamId, {
      method: request.method,
      responded: false,
      resolve: resolveResp,
      writer: null,
      chain: Promise.resolve(),
    });

    const onAbort = () => {
      try {
        agentWs.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, streamId, 0));
      } catch {}
      this.failHTTPStream(streamId, "Client aborted request");
    };
    request.signal?.addEventListener("abort", onAbort, { once: true });

    try {
      agentWs.send(
        encodeFrame(FRAME_HTTP_REQ_HEAD, bodyless ? FLAG_FIN : 0, streamId, 0, {
          method: request.method,
          host,
          path: url.pathname + url.search,
          headers,
          remoteAddr: request.headers.get("CF-Connecting-IP") || "",
        }),
      );
      if (!bodyless && request.body) {
        const reader = request.body.getReader();
        while (true) {
          const { done, value } = await reader.read();
          if (done) {
            agentWs.send(encodeFrame(FRAME_HTTP_REQ_DATA, FLAG_FIN, streamId, 0));
            break;
          }
          if (value && value.byteLength > 0) {
            for (let off = 0; off < value.byteLength; off += MAX_CHUNK) {
              const slice = value.subarray(off, Math.min(off + MAX_CHUNK, value.byteLength));
              agentWs.send(encodeFrame(FRAME_HTTP_REQ_DATA, 0, streamId, 0, slice));
            }
          }
        }
      }
    } catch {
      this.failHTTPStream(streamId, "Failed to forward request to Devbase tunnel");
    }
    return respPromise;
  }

  async handleVisitorWebSocket(request, url, host, agentWs) {
    const streamId = this.allocStreamId();
    const protoHeader = request.headers.get("Sec-WebSocket-Protocol") || "";
    const protocols = protoHeader
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    const headers = [];
    for (const [k, v] of request.headers.entries()) {
      headers.push([k, v]);
    }

    const ackPromise = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.wsPending.delete(streamId);
        reject(new Error("WebSocket upstream timed out"));
      }, 15000);
      this.wsPending.set(streamId, {
        resolve: (val) => { clearTimeout(timer); resolve(val); },
        reject: (err) => { clearTimeout(timer); reject(err); },
      });
    });

    try {
      agentWs.send(
        encodeFrame(FRAME_WS_OPEN, FLAG_FIN, streamId, 0, {
          host,
          path: url.pathname + url.search,
          protocols,
          headers,
        }),
      );
      const ack = await ackPromise;
      if (ack.status !== 101) {
        return new Response(ack.error || "WebSocket upstream rejected connection", { status: ack.status || 502 });
      }
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);
      this.ctx.acceptWebSocket(server, ["visitor"]);
      server.serializeAttachment({ streamId });
      this.wsVisitors.set(streamId, server);
      const respHeaders = new Headers();
      if (ack.protocol) {
        respHeaders.set("Sec-WebSocket-Protocol", ack.protocol);
      }
      return new Response(null, { status: 101, webSocket: client, headers: respHeaders });
    } catch (err) {
      return new Response(err?.message || "WebSocket tunnel failed", { status: 502 });
    }
  }

  async webSocketMessage(ws, message) {
    const tags = this.ctx.getTags(ws);
    if (tags.includes("visitor")) {
      const att = ws.deserializeAttachment() || {};
      const agentWs = this.agentWs;
      if (!agentWs || agentWs.readyState !== WS_OPEN) {
        try { ws.close(1001, "Tunnel offline"); } catch {}
        return;
      }
      const isText = typeof message === "string";
      const bytes = isText ? textEncoder.encode(message) : new Uint8Array(message);
      const textFlag = isText ? FLAG_TEXT : 0;
      if (bytes.byteLength === 0) {
        agentWs.send(encodeFrame(FRAME_WS_DATA, textFlag | FLAG_FIN, att.streamId, 0));
        return;
      }
      for (let off = 0; off < bytes.byteLength; off += MAX_CHUNK) {
        const end = Math.min(off + MAX_CHUNK, bytes.byteLength);
        const fin = end === bytes.byteLength ? FLAG_FIN : 0;
        agentWs.send(encodeFrame(FRAME_WS_DATA, textFlag | fin, att.streamId, 0, bytes.subarray(off, end)));
      }
      return;
    }

    if (typeof message === "string") {
      if (message === CONTROL_PING) {
        ws.send(CONTROL_PONG);
      }
      return;
    }

    let frame;
    try {
      frame = decodeFrame(message);
    } catch {
      return;
    }

    switch (frame.type) {
      case FRAME_REGISTER: {
        const req = JSON.parse(textDecoder.decode(frame.payload));
        const att = ws.deserializeAttachment() || {};
        const clientId = req.clientId || att.clientId;
        if (!clientId) return;
        if (this.agentClientId && this.agentClientId !== clientId && this.agentWs && this.agentWs.readyState === WS_OPEN) {
          ws.send(encodeFrame(FRAME_REGISTER_ACK, FLAG_FIN, 0, 0, { ok: false, conflictOwner: this.agentClientId }));
          return;
        }
        this.cleanupAgent(ws);
        this.agentWs = ws;
        this.agentClientId = clientId;
        ws.serializeAttachment({ clientId, active: true });
        ws.send(encodeFrame(FRAME_REGISTER_ACK, FLAG_FIN, 0, 0, { ok: true }));
        break;
      }
      case FRAME_HTTP_RES_HEAD: {
        const st = this.httpStreams.get(frame.streamId);
        if (!st || st.responded) return;
        const head = JSON.parse(textDecoder.decode(frame.payload));
        const status = head.status || frame.code || 200;
        const headers = new Headers();
        for (const [k, v] of head.headers || []) {
          try { headers.append(k, v); } catch {}
        }
        const noBody = (frame.flags & FLAG_FIN) !== 0 || status === 101 || status === 204 || status === 205 || status === 304 || st.method === "HEAD";
        st.responded = true;
        if (noBody) {
          this.httpStreams.delete(frame.streamId);
          st.resolve(new Response(null, { status, headers }));
          return;
        }
        const ts = new TransformStream();
        st.writer = ts.writable.getWriter();
        st.resolve(new Response(ts.readable, { status, headers }));
        break;
      }
      case FRAME_HTTP_RES_DATA: {
        const st = this.httpStreams.get(frame.streamId);
        if (!st || !st.writer) return;
        const chunk = frame.payload.byteLength > 0 ? new Uint8Array(frame.payload) : null;
        const isFin = (frame.flags & FLAG_FIN) !== 0;
        const streamId = frame.streamId;
        st.chain = st.chain.then(async () => {
          if (!st.writer) return;
          if (chunk) {
            await st.writer.write(chunk);
          }
          if (isFin) {
            this.httpStreams.delete(streamId);
            await st.writer.close();
          }
        }).catch(() => {
          this.httpStreams.delete(streamId);
        });
        break;
      }
      case FRAME_STREAM_RESET: {
        this.failHTTPStream(frame.streamId, "Stream reset by local upstream");
        const pending = this.wsPending.get(frame.streamId);
        if (pending) {
          this.wsPending.delete(frame.streamId);
          pending.reject(new Error("WebSocket stream reset"));
        }
        const visitor = this.findVisitor(frame.streamId);
        if (visitor) {
          this.wsVisitors.delete(frame.streamId);
          this.wsInboundChunks.delete(frame.streamId);
          try { visitor.close(1001, "Stream reset"); } catch {}
        }
        break;
      }
      case FRAME_WS_OPEN_ACK: {
        const pending = this.wsPending.get(frame.streamId);
        if (!pending) return;
        this.wsPending.delete(frame.streamId);
        const body = frame.payload.byteLength > 0 ? JSON.parse(textDecoder.decode(frame.payload)) : {};
        pending.resolve({ status: frame.code, protocol: body.protocol || "", error: body.error || "" });
        break;
      }
      case FRAME_WS_DATA: {
        const visitor = this.findVisitor(frame.streamId);
        if (!visitor) return;
        const chunks = this.wsInboundChunks.get(frame.streamId) || [];
        if (frame.payload.byteLength > 0) {
          chunks.push(new Uint8Array(frame.payload));
        }
        if ((frame.flags & FLAG_FIN) === 0) {
          this.wsInboundChunks.set(frame.streamId, chunks);
          return;
        }
        this.wsInboundChunks.delete(frame.streamId);
        let total = 0;
        for (const c of chunks) total += c.byteLength;
        const combined = new Uint8Array(total);
        let offset = 0;
        for (const c of chunks) {
          combined.set(c, offset);
          offset += c.byteLength;
        }
        if ((frame.flags & FLAG_TEXT) !== 0) {
          visitor.send(textDecoder.decode(combined));
        } else {
          visitor.send(combined);
        }
        break;
      }
      case FRAME_WS_CLOSE: {
        const visitor = this.findVisitor(frame.streamId);
        if (visitor) {
          this.wsVisitors.delete(frame.streamId);
          this.wsInboundChunks.delete(frame.streamId);
          const code = frame.code >= 1000 && frame.code <= 4999 ? frame.code : 1000;
          const reason = textDecoder.decode(frame.payload);
          try { visitor.close(code, reason); } catch {}
        }
        break;
      }
    }
  }

  async webSocketClose(ws, code, reason) {
    const tags = this.ctx.getTags(ws);
    const att = ws.deserializeAttachment() || {};
    if (tags.includes("visitor")) {
      this.wsVisitors.delete(att.streamId);
      this.wsInboundChunks.delete(att.streamId);
      const agentWs = this.agentWs;
      if (agentWs && agentWs.readyState === WS_OPEN) {
        agentWs.send(encodeFrame(FRAME_WS_CLOSE, FLAG_FIN, att.streamId, code || 1000, reason || ""));
      }
      return;
    }
    if (ws === this.agentWs || (att.active && (!this.agentWs || this.agentWs === ws))) {
      this.cleanupAgent(null);
    }
  }

  async webSocketError(ws) {
    await this.webSocketClose(ws, 1011, "WebSocket error");
  }
}
`
