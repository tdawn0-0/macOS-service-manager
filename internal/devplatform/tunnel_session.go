package devplatform

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"html/template"
	"io"
	"net"
	"net/http"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

type tunnelSession struct {
	runner      *tunnelWorkerRunner
	ws          *wsConn
	sem         chan struct{}
	streamsMu   sync.Mutex
	httpStreams map[uint32]*tunnelHTTPStream
	wsStreams   map[uint32]*tunnelWSStream
}

func (s *tunnelSession) sendRegister(clientID string) error {
	frame, err := encodeTunnelJSONFrame(frameRegister, tunnelFlagFIN, 0, 0, tunnelRegisterPayload{
		ClientID: clientID,
	})
	if err != nil {
		return err
	}
	return s.ws.WriteBinary(frame)
}

func (s *tunnelSession) closeAll() {
	s.streamsMu.Lock()
	httpList := make([]*tunnelHTTPStream, 0, len(s.httpStreams))
	for id, st := range s.httpStreams {
		httpList = append(httpList, st)
		delete(s.httpStreams, id)
	}
	wsList := make([]*tunnelWSStream, 0, len(s.wsStreams))
	for id, st := range s.wsStreams {
		wsList = append(wsList, st)
		delete(s.wsStreams, id)
	}
	s.streamsMu.Unlock()

	for _, st := range httpList {
		st.cancel()
		st.body.CloseWithError(io.ErrClosedPipe)
	}
	for _, st := range wsList {
		st.close(1001, "tunnel closed")
	}
}

func (s *tunnelSession) readLoop(ctx context.Context) error {
	for {
		op, data, err := s.ws.ReadMessage()
		if err != nil {
			return err
		}
		if op == wsOpText {
			// Tunnel frames are binary; the gateway's text control replies
			// answer text pings, which this client never sends.
			continue
		}
		frame, err := decodeTunnelFrame(data)
		if err != nil {
			return err
		}
		switch frame.Type {
		case frameRegisterAck:
			var ack tunnelRegisterAckPayload
			if err := json.Unmarshal(frame.Payload, &ack); err == nil {
				var conflicts []TunnelConflict
				if !ack.OK && ack.ConflictOwner != "" {
					conflicts = []TunnelConflict{{Host: s.runner.route.PublicHost(), Owner: ack.ConflictOwner}}
				}
				s.runner.sup.updateStatus(s.runner.route.WorkerName, tunnelWorkerStatus{
					phase:     "Connected",
					conflicts: conflicts,
				})
			}
		case frameHTTPReqHead:
			s.handleHTTPHead(ctx, frame)
		case frameHTTPReqData:
			s.handleHTTPData(frame)
		case frameStreamReset:
			s.handleStreamReset(frame.StreamID)
		case frameWSOpen:
			s.handleWSOpen(ctx, frame)
		case frameWSData:
			s.handleWSData(frame)
		case frameWSClose:
			s.handleWSClose(frame)
		}
	}
}

func (s *tunnelSession) handleHTTPHead(ctx context.Context, frame tunnelFrame) {
	var head tunnelHTTPReqHead
	if err := json.Unmarshal(frame.Payload, &head); err != nil {
		s.sendReset(frame.StreamID)
		return
	}
	route := s.runner.route
	select {
	case s.sem <- struct{}{}:
	default:
		s.sendSimpleHTTPResponse(frame.StreamID, http.StatusServiceUnavailable, "text/plain; charset=utf-8", []byte("Tunnel concurrency limit reached"))
		return
	}

	streamCtx, cancel := context.WithCancel(ctx)
	var bodyReader io.ReadCloser = http.NoBody
	var bodyBuf *streamBodyBuffer
	if frame.Flags&tunnelFlagFIN == 0 {
		bodyBuf = newStreamBodyBuffer(tunnelMaxStreamBuffer)
		bodyReader = bodyBuf
		st := &tunnelHTTPStream{body: bodyBuf, cancel: cancel}
		s.streamsMu.Lock()
		s.httpStreams[frame.StreamID] = st
		s.streamsMu.Unlock()
	}

	go func() {
		defer func() {
			cancel()
			if bodyBuf != nil {
				bodyBuf.CloseWithError(io.ErrClosedPipe)
				s.streamsMu.Lock()
				delete(s.httpStreams, frame.StreamID)
				s.streamsMu.Unlock()
			}
			<-s.sem
		}()
		s.forwardHTTP(streamCtx, frame.StreamID, route, head, bodyReader)
	}()
}

func (s *tunnelSession) handleHTTPData(frame tunnelFrame) {
	s.streamsMu.Lock()
	st := s.httpStreams[frame.StreamID]
	s.streamsMu.Unlock()
	if st == nil {
		return
	}
	if len(frame.Payload) > 0 {
		if !st.body.Push(frame.Payload) {
			st.cancel()
			s.sendReset(frame.StreamID)
			return
		}
	}
	if frame.Flags&tunnelFlagFIN != 0 {
		st.body.Finish()
	}
}

func (s *tunnelSession) handleStreamReset(streamID uint32) {
	s.streamsMu.Lock()
	hs := s.httpStreams[streamID]
	delete(s.httpStreams, streamID)
	ws := s.wsStreams[streamID]
	delete(s.wsStreams, streamID)
	s.streamsMu.Unlock()
	if hs != nil {
		hs.cancel()
		hs.body.CloseWithError(errors.New("stream reset by peer"))
	}
	if ws != nil {
		ws.close(1001, "stream reset")
	}
}

func (s *tunnelSession) forwardHTTP(ctx context.Context, streamID uint32, route TunnelRoute, head tunnelHTTPReqHead, body io.ReadCloser) {
	path := head.Path
	if path == "" || !strings.HasPrefix(path, "/") {
		path = "/" + strings.TrimPrefix(path, "/")
	}
	targetURL := "http://" + upstreamHostPort(route.UpstreamHost, route.LocalPort) + path
	req, err := http.NewRequestWithContext(ctx, head.Method, targetURL, body)
	if err != nil {
		s.sendSimpleHTTPResponse(streamID, http.StatusBadRequest, "text/plain; charset=utf-8", []byte("Invalid tunneled request"))
		return
	}
	localHost := route.localHostHeader()
	req.Host = localHost
	applyTunnelForwardedHeaders(req.Header, head.Headers, route.PublicHost(), localHost, head.RemoteAddr)

	resp, err := s.runner.transport.RoundTrip(req)
	if err != nil {
		if ctx.Err() != nil {
			s.sendReset(streamID)
			return
		}
		status, bodyBytes := renderTunnelFailure(route, err)
		s.sendSimpleHTTPResponse(streamID, status, "text/html; charset=utf-8", bodyBytes)
		return
	}
	defer resp.Body.Close()

	respHeaders := filterResponseHeaders(resp.Header)
	bodyless := strings.EqualFold(head.Method, http.MethodHead) || resp.StatusCode == http.StatusNoContent || resp.StatusCode == http.StatusNotModified || resp.StatusCode == http.StatusSwitchingProtocols || resp.ContentLength == 0
	var headFlags byte
	if bodyless {
		headFlags = tunnelFlagFIN
	}
	headFrame, err := encodeTunnelJSONFrame(frameHTTPResHead, headFlags, streamID, uint16(resp.StatusCode), tunnelHTTPResHead{
		Status:  resp.StatusCode,
		Headers: respHeaders,
	})
	if err != nil {
		s.sendReset(streamID)
		return
	}
	if err = s.ws.WriteBinary(headFrame); err != nil {
		return
	}
	if bodyless {
		return
	}

	buf := make([]byte, tunnelMaxChunk)
	var bytesRead int64
	for {
		n, readErr := resp.Body.Read(buf)
		bytesRead += int64(n)
		if readErr == nil && resp.ContentLength > 0 && bytesRead >= resp.ContentLength {
			var probe [1]byte
			pn, pErr := resp.Body.Read(probe[:])
			if pn > 0 {
				buf = append(buf[:n], probe[:pn]...)
				n += pn
				bytesRead += int64(pn)
			}
			readErr = pErr
		}
		if n > 0 {
			for off := 0; off < n; off += tunnelMaxChunk {
				end := off + tunnelMaxChunk
				if end > n {
					end = n
				}
				var flags byte
				if readErr == io.EOF && end == n {
					flags = tunnelFlagFIN
				}
				frame := encodeTunnelFrame(tunnelFrame{
					Type:     frameHTTPResData,
					Flags:    flags,
					StreamID: streamID,
					Payload:  buf[off:end],
				})
				if writeErr := s.ws.WriteBinary(frame); writeErr != nil {
					return
				}
			}
			if readErr == io.EOF {
				return
			}
		}
		if readErr == io.EOF {
			finFrame := encodeTunnelFrame(tunnelFrame{
				Type:     frameHTTPResData,
				Flags:    tunnelFlagFIN,
				StreamID: streamID,
			})
			_ = s.ws.WriteBinary(finFrame)
			return
		}
		if readErr != nil {
			s.sendReset(streamID)
			return
		}
	}
}

func (s *tunnelSession) sendSimpleHTTPResponse(streamID uint32, status int, contentType string, body []byte) {
	headers := [][2]string{
		{"Content-Type", contentType},
		{"Cache-Control", "no-store"},
		{"Content-Length", strconv.Itoa(len(body))},
	}
	var headFlags byte
	if len(body) == 0 {
		headFlags = tunnelFlagFIN
	}
	headFrame, err := encodeTunnelJSONFrame(frameHTTPResHead, headFlags, streamID, uint16(status), tunnelHTTPResHead{
		Status:  status,
		Headers: headers,
	})
	if err != nil {
		return
	}
	if err = s.ws.WriteBinary(headFrame); err != nil || len(body) == 0 {
		return
	}
	for off := 0; off < len(body); off += tunnelMaxChunk {
		end := off + tunnelMaxChunk
		var flags byte
		if end >= len(body) {
			end = len(body)
			flags = tunnelFlagFIN
		}
		_ = s.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
			Type:     frameHTTPResData,
			Flags:    flags,
			StreamID: streamID,
			Payload:  body[off:end],
		}))
	}
}

func (s *tunnelSession) sendReset(streamID uint32) {
	_ = s.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
		Type:     frameStreamReset,
		Flags:    tunnelFlagFIN,
		StreamID: streamID,
	}))
}

func (s *tunnelSession) handleWSOpen(ctx context.Context, frame tunnelFrame) {
	var head tunnelWSOpenHead
	if err := json.Unmarshal(frame.Payload, &head); err != nil {
		s.sendWSOpenError(frame.StreamID, http.StatusBadRequest, "invalid websocket open payload")
		return
	}
	route := s.runner.route
	select {
	case s.sem <- struct{}{}:
	default:
		s.sendWSOpenError(frame.StreamID, http.StatusServiceUnavailable, "tunnel concurrency limit reached")
		return
	}

	path := head.Path
	if path == "" || !strings.HasPrefix(path, "/") {
		path = "/" + strings.TrimPrefix(path, "/")
	}
	localHost := route.localHostHeader()
	reqHeaders := http.Header{}
	reqHeaders.Set("Host", localHost)
	applyTunnelForwardedHeaders(reqHeaders, head.Headers, route.PublicHost(), localHost, "")

	dialCtx, dialCancel := context.WithTimeout(ctx, 10*time.Second)
	localWS, resp, err := dialWebSocketVia(dialCtx, "ws://"+upstreamHostPort(route.UpstreamHost, route.LocalPort)+path, reqHeaders, head.Protocols, nil, upstreamDialer)
	dialCancel()
	if err != nil {
		<-s.sem
		status := http.StatusBadGateway
		var hsErr *wsHandshakeError
		if errors.As(err, &hsErr) {
			status = hsErr.StatusCode
		}
		s.sendWSOpenError(frame.StreamID, status, "local websocket upstream unavailable")
		return
	}

	negotiatedProto := ""
	if resp != nil {
		negotiatedProto = resp.Header.Get("Sec-WebSocket-Protocol")
	}
	ackFrame, err := encodeTunnelJSONFrame(frameWSOpenAck, tunnelFlagFIN, frame.StreamID, http.StatusSwitchingProtocols, tunnelWSOpenAckHead{
		Protocol: negotiatedProto,
	})
	if err != nil {
		localWS.Close()
		<-s.sem
		return
	}

	wsStream := &tunnelWSStream{
		session:  s,
		streamID: frame.StreamID,
		local:    localWS,
		sendCh:   make(chan wsOutboundMsg, 64),
	}
	s.streamsMu.Lock()
	s.wsStreams[frame.StreamID] = wsStream
	s.streamsMu.Unlock()

	if err = s.ws.WriteBinary(ackFrame); err != nil {
		s.streamsMu.Lock()
		delete(s.wsStreams, frame.StreamID)
		s.streamsMu.Unlock()
		wsStream.close(1001, "tunnel write error")
		<-s.sem
		return
	}

	go func() {
		defer func() {
			s.streamsMu.Lock()
			delete(s.wsStreams, frame.StreamID)
			s.streamsMu.Unlock()
			wsStream.close(1000, "")
			<-s.sem
		}()
		wsStream.pump(ctx)
	}()
}

func (s *tunnelSession) sendWSOpenError(streamID uint32, status int, msg string) {
	b, err := encodeTunnelJSONFrame(frameWSOpenAck, tunnelFlagFIN, streamID, uint16(status), tunnelWSOpenAckHead{Error: msg})
	if err == nil {
		_ = s.ws.WriteBinary(b)
	}
}

func (s *tunnelSession) handleWSData(frame tunnelFrame) {
	s.streamsMu.Lock()
	st := s.wsStreams[frame.StreamID]
	s.streamsMu.Unlock()
	if st == nil {
		return
	}
	st.mu.Lock()
	if len(st.fragBuf)+len(frame.Payload) > wsMaxMessageSize {
		st.mu.Unlock()
		st.close(1009, "message too big")
		s.sendReset(frame.StreamID)
		return
	}
	st.fragBuf = append(st.fragBuf, frame.Payload...)
	if frame.Flags&tunnelFlagFIN == 0 {
		st.mu.Unlock()
		return
	}
	msg := st.fragBuf
	st.fragBuf = nil
	st.mu.Unlock()

	opcode := wsOpBinary
	if frame.Flags&tunnelFlagText != 0 {
		opcode = wsOpText
	}
	if !st.enqueue(opcode, msg) {
		st.close(1001, "local websocket backpressure")
		s.sendReset(frame.StreamID)
	}
}

func (s *tunnelSession) handleWSClose(frame tunnelFrame) {
	s.streamsMu.Lock()
	st := s.wsStreams[frame.StreamID]
	delete(s.wsStreams, frame.StreamID)
	s.streamsMu.Unlock()
	if st != nil {
		code := frame.Code
		if code == 0 {
			code = 1000
		}
		st.close(code, string(frame.Payload))
	}
}

type wsOutboundMsg struct {
	opcode byte
	data   []byte
}

type tunnelWSStream struct {
	session  *tunnelSession
	streamID uint32
	local    *wsConn

	mu        sync.Mutex
	fragBuf   []byte
	closed    bool
	closeOnce sync.Once
	sendCh    chan wsOutboundMsg
}

func (st *tunnelWSStream) enqueue(opcode byte, data []byte) bool {
	st.mu.Lock()
	closed := st.closed
	st.mu.Unlock()
	if closed {
		return false
	}
	select {
	case st.sendCh <- wsOutboundMsg{opcode: opcode, data: data}:
		return true
	default:
		return false
	}
}

func (st *tunnelWSStream) close(code uint16, reason string) {
	st.closeOnce.Do(func() {
		st.mu.Lock()
		st.closed = true
		st.mu.Unlock()
		if code != 0 {
			_ = st.local.WriteClose(code, reason)
		}
		_ = st.local.Close()
		close(st.sendCh)
	})
}

func (st *tunnelWSStream) pump(ctx context.Context) {
	writerDone := make(chan struct{})
	go func() {
		defer close(writerDone)
		for msg := range st.sendCh {
			if err := st.local.WriteMessage(msg.opcode, msg.data); err != nil {
				st.close(1011, "local write error")
				return
			}
		}
	}()

	for {
		op, data, err := st.local.ReadMessage()
		if err != nil {
			var closeErr *wsCloseError
			code := uint16(1000)
			reason := ""
			if errors.As(err, &closeErr) {
				code = closeErr.Code
				reason = closeErr.Reason
			}
			if ctx.Err() == nil {
				_ = st.session.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
					Type:     frameWSClose,
					Flags:    tunnelFlagFIN,
					StreamID: st.streamID,
					Code:     code,
					Payload:  []byte(reason),
				}))
			}
			st.close(0, "")
			<-writerDone
			return
		}
		var textFlag byte
		if op == wsOpText {
			textFlag = tunnelFlagText
		}
		if len(data) == 0 {
			_ = st.session.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
				Type:     frameWSData,
				Flags:    textFlag | tunnelFlagFIN,
				StreamID: st.streamID,
			}))
			continue
		}
		for off := 0; off < len(data); off += tunnelMaxChunk {
			end := off + tunnelMaxChunk
			flags := textFlag
			if end >= len(data) {
				end = len(data)
				flags |= tunnelFlagFIN
			}
			if writeErr := st.session.ws.WriteBinary(encodeTunnelFrame(tunnelFrame{
				Type:     frameWSData,
				Flags:    flags,
				StreamID: st.streamID,
				Payload:  data[off:end],
			})); writeErr != nil {
				st.close(1001, "tunnel write error")
				<-writerDone
				return
			}
		}
	}
}

type tunnelHTTPStream struct {
	body   *streamBodyBuffer
	cancel context.CancelFunc
}

type streamBodyBuffer struct {
	mu      sync.Mutex
	chunks  [][]byte
	queued  int
	maxSize int
	eof     bool
	err     error
	notify  chan struct{}
}

func newStreamBodyBuffer(maxSize int) *streamBodyBuffer {
	return &streamBodyBuffer{
		maxSize: maxSize,
		notify:  make(chan struct{}, 1),
	}
}

func (b *streamBodyBuffer) signal() {
	select {
	case b.notify <- struct{}{}:
	default:
	}
}

func (b *streamBodyBuffer) Push(p []byte) bool {
	if len(p) == 0 {
		return true
	}
	cp := append([]byte(nil), p...)
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.eof || b.err != nil || b.queued+len(cp) > b.maxSize {
		return false
	}
	b.chunks = append(b.chunks, cp)
	b.queued += len(cp)
	b.signal()
	return true
}

func (b *streamBodyBuffer) Finish() {
	b.mu.Lock()
	b.eof = true
	b.signal()
	b.mu.Unlock()
}

func (b *streamBodyBuffer) CloseWithError(err error) {
	b.mu.Lock()
	if b.err == nil {
		b.err = err
	}
	b.signal()
	b.mu.Unlock()
}

func (b *streamBodyBuffer) Close() error {
	b.CloseWithError(io.ErrClosedPipe)
	return nil
}

func (b *streamBodyBuffer) Read(p []byte) (int, error) {
	for {
		b.mu.Lock()
		if len(b.chunks) > 0 {
			head := b.chunks[0]
			n := copy(p, head)
			if n < len(head) {
				b.chunks[0] = head[n:]
			} else {
				b.chunks = b.chunks[1:]
			}
			b.queued -= n
			b.mu.Unlock()
			return n, nil
		}
		if b.err != nil {
			err := b.err
			b.mu.Unlock()
			return 0, err
		}
		if b.eof {
			b.mu.Unlock()
			return 0, io.EOF
		}
		b.mu.Unlock()
		<-b.notify
	}
}

func applyTunnelForwardedHeaders(dst http.Header, incoming [][2]string, publicHost, localHost, remoteAddr string) {
	hopByHop := map[string]bool{
		"connection":               true,
		"keep-alive":               true,
		"proxy-authenticate":       true,
		"proxy-authorization":      true,
		"te":                       true,
		"trailer":                  true,
		"transfer-encoding":        true,
		"upgrade":                  true,
		"host":                     true,
		"forwarded":                true,
		"x-forwarded-for":          true,
		"x-forwarded-host":         true,
		"x-forwarded-proto":        true,
		"x-real-ip":                true,
		"sec-websocket-key":        true,
		"sec-websocket-version":    true,
		"sec-websocket-extensions": true,
		"sec-websocket-protocol":   true,
		"cf-connecting-ip":         false,
	}
	var connTokens []string
	for _, kv := range incoming {
		if strings.EqualFold(kv[0], "connection") {
			for _, tok := range strings.Split(kv[1], ",") {
				if t := strings.ToLower(strings.TrimSpace(tok)); t != "" {
					connTokens = append(connTokens, t)
				}
			}
		}
	}
	for _, t := range connTokens {
		hopByHop[t] = true
	}
	for _, kv := range incoming {
		lk := strings.ToLower(kv[0])
		if hopByHop[lk] {
			continue
		}
		if lk == "origin" && publicHost != "" {
			origin := strings.TrimSpace(kv[1])
			if strings.EqualFold(origin, "https://"+publicHost) || strings.EqualFold(origin, "https://"+publicHost+":443") {
				dst.Set("X-Forwarded-Origin", origin)
				dst.Set("Origin", "http://"+localHost)
				continue
			}
		}
		dst.Add(kv[0], kv[1])
	}
	if publicHost != "" {
		dst.Set("X-Forwarded-Host", publicHost)
	}
	dst.Set("X-Forwarded-Proto", "https")
	if remoteAddr != "" {
		dst.Set("X-Forwarded-For", remoteAddr)
		dst.Set("X-Real-IP", remoteAddr)
	}
}

func filterResponseHeaders(h http.Header) [][2]string {
	hopByHop := map[string]bool{
		"connection":          true,
		"keep-alive":          true,
		"proxy-authenticate":  true,
		"proxy-authorization": true,
		"te":                  true,
		"trailer":             true,
		"transfer-encoding":   true,
		"upgrade":             true,
	}
	for _, v := range h.Values("Connection") {
		for _, tok := range strings.Split(v, ",") {
			if t := strings.ToLower(strings.TrimSpace(tok)); t != "" {
				hopByHop[t] = true
			}
		}
	}
	var out [][2]string
	keys := make([]string, 0, len(h))
	for k := range h {
		if !hopByHop[strings.ToLower(k)] {
			keys = append(keys, k)
		}
	}
	sort.Strings(keys)
	for _, k := range keys {
		for _, v := range h[k] {
			out = append(out, [2]string{k, v})
		}
	}
	return out
}

var tunnelFailurePage = template.Must(template.New("tunnel-failure").Parse(`<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>{{.Title}}</title><style>body{font:16px/1.6 system-ui;margin:12vh auto;padding:24px;max-width:640px;color:#202124;background:#f5f6f8}main{background:white;padding:32px;border-radius:16px}h1{line-height:1.2;font-size:28px}code{word-break:break-all}p{color:#555}</style></head><body><main><h1>{{.Title}}</h1><p>{{.Message}}</p><p><strong>{{.PublicHost}}</strong> → <code>http://{{.Upstream}}</code></p><p>Open Devbase to check the local service and tunnel status.</p></main></body></html>`))

func renderTunnelFailure(route TunnelRoute, err error) (int, []byte) {
	status := http.StatusBadGateway
	title := "Local service is unavailable"
	message := "The local server did not accept this tunneled request. Start the local application or check its port."
	var netErr net.Error
	if errors.As(err, &netErr) && netErr.Timeout() {
		status = http.StatusGatewayTimeout
		title = "Local service timed out"
		message = "The local server did not respond within the tunnel timeout."
	}
	var buf bytes.Buffer
	_ = tunnelFailurePage.Execute(&buf, struct {
		Title, Message, PublicHost string
		Upstream                   string
	}{title, message, route.PublicHost(), upstreamHostPort(route.UpstreamHost, route.LocalPort)})
	return status, buf.Bytes()
}

// localHostHeader is the Host the upstream sees for tunneled requests:
// localhost for this Mac, otherwise the upstream's own address.
func (r TunnelRoute) localHostHeader() string {
	if r.UpstreamHost == "" {
		return fmt.Sprintf("localhost:%d", r.LocalPort)
	}
	return upstreamHostPort(r.UpstreamHost, r.LocalPort)
}
