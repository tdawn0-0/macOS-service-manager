package devplatform

const cloudflareWorkerIndexMJS = `import { DurableObject } from "cloudflare:workers";

const PROTO_VERSION = 1;
const HEADER_SIZE = 9;
const MAX_CHUNK = 256 * 1024;
const MAX_WS_MESSAGE = 4 * 1024 * 1024;
const HTTP_HEAD_TIMEOUT_MS = 65000;
const WS_OPEN_TIMEOUT_MS = 15000;
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

function truncateUTF8(str, maxBytes) {
  if (!str) return "";
  if (textEncoder.encode(str).byteLength <= maxBytes) return str;
  let out = "";
  let used = 0;
  for (const ch of str) {
    const b = textEncoder.encode(ch).byteLength;
    if (used + b > maxBytes) break;
    out += ch;
    used += b;
  }
  return out;
}

function safeClose(ws, code, reason) {
  if (!ws) return;
  const validCode = Number.isInteger(code) && ((code >= 1000 && code <= 1014 && code !== 1004 && code !== 1005 && code !== 1006) || (code >= 3000 && code <= 4999))
    ? code
    : 1000;
  const safeReason = truncateUTF8(typeof reason === "string" ? reason : String(reason || ""), 123);
  try {
    ws.close(validCode, safeReason);
  } catch {
    try { ws.close(1000); } catch {}
  }
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/__devbase/connect") {
      if (request.method !== "GET") {
        return new Response("Method Not Allowed", { status: 405 });
      }
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
    if (url.pathname.startsWith("/__devbase/")) {
      return new Response("Not Found", { status: 404 });
    }
    const stub = env.TUNNEL_GATEWAY.get(env.TUNNEL_GATEWAY.idFromName("gateway"));
    return stub.fetch(request);
  },
};

export class TunnelGateway extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    if (typeof this.ctx.getWebSocketAutoResponse !== "function" || !this.ctx.getWebSocketAutoResponse()) {
      this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair(CONTROL_PING, CONTROL_PONG));
    }
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
        if (this.agentWs && this.agentWs !== ws) {
          const prev = this.agentWs;
          const prevAtt = prev.deserializeAttachment() || {};
          try { prev.serializeAttachment({ clientId: prevAtt.clientId || "", active: false }); } catch {}
          safeClose(prev, 1000, "superseded");
        }
        this.agentWs = ws;
        this.agentClientId = att.clientId;
        if (Number.isInteger(att.nextStreamId) && att.nextStreamId >= 1 && att.nextStreamId <= 0x7fffffff) {
          this.nextStreamId = att.nextStreamId;
        }
      }
    }
    for (const ws of this.ctx.getWebSockets("visitor")) {
      if (!this.agentWs) {
        safeClose(ws, 1001, "Tunnel offline");
        continue;
      }
      const att = ws.deserializeAttachment() || {};
      if (ws.readyState === WS_OPEN && Number.isInteger(att.streamId) && att.streamId > 0) {
        this.wsVisitors.set(att.streamId, ws);
        if (att.streamId >= this.nextStreamId) {
          this.nextStreamId = att.streamId >= 0x7fffffff ? 1 : att.streamId + 1;
        }
      }
    }
  }

  getActiveAgent() {
    if (this.agentWs && this.agentWs.readyState === WS_OPEN) {
      return this.agentWs;
    }
    if (this.agentWs) {
      this.agentWs = null;
      this.agentClientId = "";
    }
    return null;
  }

  allocStreamId() {
    for (let i = 0; i < 0x7fffffff; i++) {
      const id = this.nextStreamId;
      this.nextStreamId = id >= 0x7fffffff ? 1 : id + 1;
      if (!this.wsVisitors.has(id) && !this.httpStreams.has(id) && !this.wsPending.has(id)) {
        if (this.agentWs && this.agentClientId) {
          try {
            this.agentWs.serializeAttachment({
              clientId: this.agentClientId,
              active: true,
              nextStreamId: this.nextStreamId,
            });
          } catch {}
        }
        return id;
      }
    }
    throw new Error("No available tunnel stream IDs");
  }

  cleanupAgent(keepWs, keepClientId = "") {
    if (this.agentWs && this.agentWs !== keepWs) {
      const prev = this.agentWs;
      const prevAtt = prev.deserializeAttachment() || {};
      try { prev.serializeAttachment({ clientId: prevAtt.clientId || "", active: false }); } catch {}
      safeClose(prev, 1000, "superseded");
    }
    for (const ws of this.ctx.getWebSockets("agent")) {
      if (ws === keepWs || ws === this.agentWs) continue;
      const att = ws.deserializeAttachment() || {};
      if (att.active || (keepClientId && att.clientId === keepClientId)) {
        try { ws.serializeAttachment({ clientId: att.clientId || "", active: false }); } catch {}
        safeClose(ws, 1000, "superseded");
      }
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
      safeClose(vws, 1001, "Tunnel disconnected");
    }
  }

  findVisitor(streamId) {
    let ws = this.wsVisitors.get(streamId);
    if (ws) {
      if (ws.readyState === WS_OPEN) return ws;
      this.wsVisitors.delete(streamId);
      return null;
    }
    for (const candidate of this.ctx.getWebSockets("visitor")) {
      const att = candidate.deserializeAttachment() || {};
      if (att.streamId === streamId && candidate.readyState === WS_OPEN) {
        this.wsVisitors.set(streamId, candidate);
        return candidate;
      }
    }
    return null;
  }

  failHTTPStream(streamId, message, status = 502) {
    const st = this.httpStreams.get(streamId);
    if (!st) return;
    this.httpStreams.delete(streamId);
    if (st.timer) {
      clearTimeout(st.timer);
      st.timer = null;
    }
    if (st.cleanupAbort) {
      st.cleanupAbort();
      st.cleanupAbort = null;
    }
    if (!st.responded) {
      st.responded = true;
      st.resolve(new Response(message, { status }));
    } else if (st.controller) {
      try { st.controller.error(new Error(message)); } catch {}
      st.controller = null;
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

    const agentWs = this.getActiveAgent();
    if (!agentWs) {
      return new Response("Devbase tunnel offline", { status: 502 });
    }

    const host = url.hostname.toLowerCase();
    if ((request.headers.get("Upgrade") || "").toLowerCase() === "websocket") {
      return this.handleVisitorWebSocket(request, url, host, agentWs);
    }
    return this.handleVisitorHTTP(request, url, host, agentWs);
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
    const st = {
      method: request.method,
      responded: false,
      resolve: resolveResp,
      controller: null,
      timer: null,
      cleanupAbort: null,
    };
    this.httpStreams.set(streamId, st);

    st.timer = setTimeout(() => {
      const cur = this.getActiveAgent();
      if (cur) {
        try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, streamId, 0)); } catch {}
      }
      this.failHTTPStream(streamId, "Gateway timeout waiting for local upstream", 504);
    }, HTTP_HEAD_TIMEOUT_MS);

    const onAbort = () => {
      const cur = this.getActiveAgent();
      if (cur) {
        try {
          cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, streamId, 0));
        } catch {}
      }
      this.failHTTPStream(streamId, "Client aborted request");
    };
    if (request.signal) {
      if (request.signal.aborted) {
        onAbort();
        return respPromise;
      }
      request.signal.addEventListener("abort", onAbort, { once: true });
      st.cleanupAbort = () => {
        try { request.signal.removeEventListener("abort", onAbort); } catch {}
      };
    }

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
          if (!this.httpStreams.has(streamId)) {
            try { await reader.cancel(); } catch {}
            break;
          }
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
    if (request.signal?.aborted) {
      return new Response("Client aborted WebSocket upgrade", { status: 499 });
    }
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

    let timer = null;
    const ackPromise = new Promise((resolve, reject) => {
      timer = setTimeout(() => {
        this.wsPending.delete(streamId);
        reject(new Error("WebSocket upstream timed out"));
      }, WS_OPEN_TIMEOUT_MS);
      this.wsPending.set(streamId, {
        resolve: (val) => { clearTimeout(timer); resolve(val); },
        reject: (err) => { clearTimeout(timer); reject(err); },
      });
    });

    const onAbort = () => {
      const pending = this.wsPending.get(streamId);
      if (pending) {
        this.wsPending.delete(streamId);
        pending.reject(new Error("Client aborted WebSocket upgrade"));
      }
      const cur = this.getActiveAgent();
      if (cur) {
        try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, streamId, 0)); } catch {}
      }
    };
    request.signal?.addEventListener("abort", onAbort, { once: true });

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
    } finally {
      clearTimeout(timer);
      this.wsPending.delete(streamId);
      try { request.signal?.removeEventListener("abort", onAbort); } catch {}
    }
  }

  async webSocketMessage(ws, message) {
    const tags = this.ctx.getTags(ws);
    if (tags.includes("visitor")) {
      const att = ws.deserializeAttachment() || {};
      const agentWs = this.getActiveAgent();
      if (!agentWs) {
        safeClose(ws, 1001, "Tunnel offline");
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
        let req;
        try {
          req = JSON.parse(textDecoder.decode(frame.payload));
        } catch {
          return;
        }
        const att = ws.deserializeAttachment() || {};
        const clientId = req.clientId || att.clientId;
        if (!clientId) return;
        if (this.agentClientId && this.agentClientId !== clientId && this.getActiveAgent()) {
          ws.send(encodeFrame(FRAME_REGISTER_ACK, FLAG_FIN, 0, 0, { ok: false, conflictOwner: this.agentClientId }));
          return;
        }
        this.cleanupAgent(ws, clientId);
        this.agentWs = ws;
        this.agentClientId = clientId;
        ws.serializeAttachment({ clientId, active: true, nextStreamId: this.nextStreamId });
        ws.send(encodeFrame(FRAME_REGISTER_ACK, FLAG_FIN, 0, 0, { ok: true }));
        break;
      }
      case FRAME_HTTP_RES_HEAD: {
        const st = this.httpStreams.get(frame.streamId);
        if (!st || st.responded) return;
        if (st.timer) {
          clearTimeout(st.timer);
          st.timer = null;
        }
        let head;
        try {
          head = JSON.parse(textDecoder.decode(frame.payload));
        } catch {
          this.failHTTPStream(frame.streamId, "Invalid tunnel response header");
          return;
        }
        const status = head.status || frame.code || 200;
        const headers = new Headers();
        for (const [k, v] of head.headers || []) {
          try { headers.append(k, v); } catch {}
        }
        const noBody = (frame.flags & FLAG_FIN) !== 0 || status === 101 || status === 204 || status === 205 || status === 304 || st.method === "HEAD";
        st.responded = true;
        if (noBody) {
          this.httpStreams.delete(frame.streamId);
          if (st.cleanupAbort) {
            st.cleanupAbort();
            st.cleanupAbort = null;
          }
          st.resolve(new Response(null, { status, headers }));
          return;
        }
        const streamId = frame.streamId;
        const readable = new ReadableStream({
          start: (controller) => {
            st.controller = controller;
          },
          cancel: () => {
            if (this.httpStreams.delete(streamId)) {
              st.controller = null;
              if (st.cleanupAbort) {
                st.cleanupAbort();
                st.cleanupAbort = null;
              }
              const cur = this.getActiveAgent();
              if (cur) {
                try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, streamId, 0)); } catch {}
              }
            }
          },
        });
        st.resolve(new Response(readable, { status, headers }));
        break;
      }
      case FRAME_HTTP_RES_DATA: {
        const st = this.httpStreams.get(frame.streamId);
        if (!st || !st.controller) return;
        const isFin = (frame.flags & FLAG_FIN) !== 0;
        try {
          if (frame.payload.byteLength > 0) {
            st.controller.enqueue(frame.payload.slice());
          }
          if (isFin) {
            this.httpStreams.delete(frame.streamId);
            if (st.cleanupAbort) {
              st.cleanupAbort();
              st.cleanupAbort = null;
            }
            const controller = st.controller;
            st.controller = null;
            controller.close();
          }
        } catch {
          this.httpStreams.delete(frame.streamId);
          st.controller = null;
          if (st.cleanupAbort) {
            st.cleanupAbort();
            st.cleanupAbort = null;
          }
          const cur = this.getActiveAgent();
          if (cur) {
            try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, frame.streamId, 0)); } catch {}
          }
        }
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
          safeClose(visitor, 1001, "Stream reset");
        }
        break;
      }
      case FRAME_WS_OPEN_ACK: {
        const pending = this.wsPending.get(frame.streamId);
        if (!pending) return;
        this.wsPending.delete(frame.streamId);
        let body = {};
        if (frame.payload.byteLength > 0) {
          try {
            body = JSON.parse(textDecoder.decode(frame.payload));
          } catch {}
        }
        pending.resolve({ status: frame.code, protocol: body.protocol || "", error: body.error || "" });
        break;
      }
      case FRAME_WS_DATA: {
        const visitor = this.findVisitor(frame.streamId);
        if (!visitor) return;
        const isFin = (frame.flags & FLAG_FIN) !== 0;
        const isText = (frame.flags & FLAG_TEXT) !== 0;
        let entry = this.wsInboundChunks.get(frame.streamId);
        if (!entry && isFin) {
          if (frame.payload.byteLength > MAX_WS_MESSAGE) {
            this.wsVisitors.delete(frame.streamId);
            safeClose(visitor, 1009, "Message too big");
            const cur = this.getActiveAgent();
            if (cur) {
              try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, frame.streamId, 0)); } catch {}
            }
            return;
          }
          try {
            if (isText) {
              visitor.send(textDecoder.decode(frame.payload));
            } else {
              visitor.send(frame.payload);
            }
          } catch {
            this.wsVisitors.delete(frame.streamId);
            safeClose(visitor, 1001, "Visitor send failed");
            const cur = this.getActiveAgent();
            if (cur) {
              try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, frame.streamId, 0)); } catch {}
            }
          }
          return;
        }
        if (!entry) {
          entry = { chunks: [], total: 0 };
          this.wsInboundChunks.set(frame.streamId, entry);
        }
        if (frame.payload.byteLength > 0) {
          if (entry.total + frame.payload.byteLength > MAX_WS_MESSAGE) {
            this.wsInboundChunks.delete(frame.streamId);
            this.wsVisitors.delete(frame.streamId);
            safeClose(visitor, 1009, "Message too big");
            const cur = this.getActiveAgent();
            if (cur) {
              try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, frame.streamId, 0)); } catch {}
            }
            return;
          }
          entry.chunks.push(frame.payload.slice());
          entry.total += frame.payload.byteLength;
        }
        if (!isFin) {
          return;
        }
        this.wsInboundChunks.delete(frame.streamId);
        const combined = new Uint8Array(entry.total);
        let offset = 0;
        for (const c of entry.chunks) {
          combined.set(c, offset);
          offset += c.byteLength;
        }
        try {
          if (isText) {
            visitor.send(textDecoder.decode(combined));
          } else {
            visitor.send(combined);
          }
        } catch {
          this.wsVisitors.delete(frame.streamId);
          safeClose(visitor, 1001, "Visitor send failed");
          const cur = this.getActiveAgent();
          if (cur) {
            try { cur.send(encodeFrame(FRAME_STREAM_RESET, FLAG_FIN, frame.streamId, 0)); } catch {}
          }
        }
        break;
      }
      case FRAME_WS_CLOSE: {
        const visitor = this.findVisitor(frame.streamId);
        if (visitor) {
          this.wsVisitors.delete(frame.streamId);
          this.wsInboundChunks.delete(frame.streamId);
          const reason = textDecoder.decode(frame.payload);
          safeClose(visitor, frame.code, reason);
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
      const agentWs = this.getActiveAgent();
      if (agentWs && att.streamId) {
        try {
          agentWs.send(encodeFrame(FRAME_WS_CLOSE, FLAG_FIN, att.streamId, code || 1000, reason || ""));
        } catch {}
      }
      safeClose(ws, code, reason);
      return;
    }
    const isCurrentAgent = ws === this.agentWs;
    const activeAgent = this.getActiveAgent();
    if (isCurrentAgent || (att.active && (!activeAgent || activeAgent === ws))) {
      try { ws.serializeAttachment({ clientId: att.clientId || "", active: false }); } catch {}
      if (this.agentWs === ws) {
        this.agentWs = null;
        this.agentClientId = "";
      }
      this.cleanupAgent(null);
    }
    safeClose(ws, code, reason);
  }

  async webSocketError(ws) {
    await this.webSocketClose(ws, 1011, "WebSocket error");
  }
}
`
