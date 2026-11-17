package devplatform

import (
	"bufio"
	"context"
	"crypto/rand"
	"crypto/sha1"
	"crypto/tls"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"
)

const (
	wsOpCont   byte = 0x0
	wsOpText   byte = 0x1
	wsOpBinary byte = 0x2
	wsOpClose  byte = 0x8
	wsOpPing   byte = 0x9
	wsOpPong   byte = 0xA

	wsGUID           = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"
	wsMaxMessageSize = 4 << 20 // 4 MiB
)

type wsHandshakeError struct {
	StatusCode int
	Status     string
	Body       string
}

func (e *wsHandshakeError) Error() string {
	if e.Body != "" {
		return fmt.Sprintf("websocket handshake failed: HTTP %d %s: %s", e.StatusCode, e.Status, e.Body)
	}
	return fmt.Sprintf("websocket handshake failed: HTTP %d %s", e.StatusCode, e.Status)
}

type wsCloseError struct {
	Code   uint16
	Reason string
}

func (e *wsCloseError) Error() string {
	if e.Reason != "" {
		return fmt.Sprintf("websocket closed (%d): %s", e.Code, e.Reason)
	}
	return fmt.Sprintf("websocket closed (%d)", e.Code)
}

type wsConn struct {
	conn       net.Conn
	br         *bufio.Reader
	isClient   bool
	writeMu    sync.Mutex
	closeOnce  sync.Once
	closed     bool
	onActivity func()
	onPing     func()
}

func wsAcceptKey(key string) string {
	sum := sha1.Sum([]byte(key + wsGUID))
	return base64.StdEncoding.EncodeToString(sum[:])
}

func dialWebSocket(ctx context.Context, rawURL string, headers http.Header, protocols []string, tlsConfig *tls.Config) (*wsConn, *http.Response, error) {
	return dialWebSocketVia(ctx, rawURL, headers, protocols, tlsConfig, nil)
}

// dialWebSocketVia is dialWebSocket with an optional dial function that
// replaces the plain TCP dialer for ws URLs.
func dialWebSocketVia(ctx context.Context, rawURL string, headers http.Header, protocols []string, tlsConfig *tls.Config, dial func(ctx context.Context, network, addr string) (net.Conn, error)) (*wsConn, *http.Response, error) {
	u, err := url.Parse(rawURL)
	if err != nil {
		return nil, nil, err
	}
	host := u.Host
	if _, _, splitErr := net.SplitHostPort(host); splitErr != nil {
		if u.Scheme == "wss" {
			host = net.JoinHostPort(host, "443")
		} else {
			host = net.JoinHostPort(host, "80")
		}
	}
	var conn net.Conn
	dialer := &net.Dialer{Timeout: 15 * time.Second, KeepAlive: 30 * time.Second}
	switch u.Scheme {
	case "ws":
		if dial != nil {
			conn, err = dial(ctx, "tcp", host)
		} else {
			conn, err = dialer.DialContext(ctx, "tcp", host)
		}
	case "wss":
		cfg := &tls.Config{MinVersion: tls.VersionTLS12}
		if tlsConfig != nil {
			cfg = tlsConfig.Clone()
		}
		if len(cfg.NextProtos) == 0 {
			cfg.NextProtos = []string{"http/1.1"}
		}
		if cfg.ServerName == "" {
			cfg.ServerName = u.Hostname()
		}
		conn, err = (&tls.Dialer{NetDialer: dialer, Config: cfg}).DialContext(ctx, "tcp", host)
	default:
		return nil, nil, fmt.Errorf("unsupported websocket scheme %q", u.Scheme)
	}
	if err != nil {
		return nil, nil, err
	}

	var nonce [16]byte
	if _, err = rand.Read(nonce[:]); err != nil {
		conn.Close()
		return nil, nil, err
	}
	secKey := base64.StdEncoding.EncodeToString(nonce[:])

	httpScheme := "http"
	if u.Scheme == "wss" {
		httpScheme = "https"
	}
	reqURL := &url.URL{Scheme: httpScheme, Host: u.Host, Path: u.Path, RawPath: u.RawPath, RawQuery: u.RawQuery}
	if reqURL.Path == "" {
		reqURL.Path = "/"
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, reqURL.String(), nil)
	if err != nil {
		conn.Close()
		return nil, nil, err
	}
	for k, vals := range headers {
		lk := strings.ToLower(k)
		if lk == "host" || lk == "upgrade" || lk == "connection" || lk == "sec-websocket-key" || lk == "sec-websocket-version" || lk == "sec-websocket-accept" {
			continue
		}
		for _, v := range vals {
			req.Header.Add(k, v)
		}
	}
	if hostOverride := headers.Get("Host"); hostOverride != "" {
		req.Host = hostOverride
	}
	if req.Header.Get("User-Agent") == "" {
		req.Header.Set("User-Agent", "Devbase-Tunnel/1.0")
	}
	req.Header.Set("Upgrade", "websocket")
	req.Header.Set("Connection", "Upgrade")
	req.Header.Set("Sec-WebSocket-Key", secKey)
	req.Header.Set("Sec-WebSocket-Version", "13")
	if len(protocols) > 0 {
		req.Header.Set("Sec-WebSocket-Protocol", strings.Join(protocols, ", "))
	}

	if deadline, ok := ctx.Deadline(); ok {
		_ = conn.SetDeadline(deadline)
	} else {
		_ = conn.SetDeadline(time.Now().Add(15 * time.Second))
	}
	if err = req.Write(conn); err != nil {
		conn.Close()
		return nil, nil, err
	}
	br := bufio.NewReaderSize(conn, 64<<10)
	resp, err := http.ReadResponse(br, req)
	if err != nil {
		conn.Close()
		return nil, nil, err
	}
	_ = conn.SetDeadline(time.Time{})

	if resp.StatusCode != http.StatusSwitchingProtocols {
		body, _ := io.ReadAll(io.LimitReader(resp.Body, 256))
		resp.Body.Close()
		conn.Close()
		return nil, resp, &wsHandshakeError{StatusCode: resp.StatusCode, Status: resp.Status, Body: strings.TrimSpace(string(body))}
	}
	resp.Body.Close()

	if !strings.EqualFold(resp.Header.Get("Upgrade"), "websocket") || !headerContainsToken(resp.Header.Get("Connection"), "upgrade") {
		conn.Close()
		return nil, resp, errors.New("websocket server did not complete upgrade")
	}
	if resp.Header.Get("Sec-WebSocket-Accept") != wsAcceptKey(secKey) {
		conn.Close()
		return nil, resp, errors.New("invalid Sec-WebSocket-Accept header")
	}

	return &wsConn{conn: conn, br: br, isClient: true}, resp, nil
}

func headerContainsToken(headerVal, token string) bool {
	for _, part := range strings.Split(headerVal, ",") {
		if strings.EqualFold(strings.TrimSpace(part), token) {
			return true
		}
	}
	return false
}

func (c *wsConn) SetReadDeadline(t time.Time) error {
	return c.conn.SetReadDeadline(t)
}

func (c *wsConn) Close() error {
	var err error
	c.closeOnce.Do(func() {
		c.writeMu.Lock()
		c.closed = true
		c.writeMu.Unlock()
		err = c.conn.Close()
	})
	return err
}

func (c *wsConn) WriteBinary(data []byte) error {
	return c.WriteMessage(wsOpBinary, data)
}

func (c *wsConn) WritePing(data []byte) error {
	if len(data) > 125 {
		data = data[:125]
	}
	return c.writeFrame(wsOpPing, data)
}

func (c *wsConn) WriteClose(code uint16, reason string) error {
	if code == 0 {
		return c.writeFrame(wsOpClose, nil)
	}
	if len(reason) > 123 {
		reason = reason[:123]
	}
	payload := make([]byte, 2+len(reason))
	binary.BigEndian.PutUint16(payload[:2], code)
	copy(payload[2:], reason)
	return c.writeFrame(wsOpClose, payload)
}

func (c *wsConn) WriteMessage(opcode byte, data []byte) error {
	return c.writeFrame(opcode, data)
}

func (c *wsConn) writeFrame(opcode byte, data []byte) error {
	c.writeMu.Lock()
	defer c.writeMu.Unlock()
	if c.closed {
		return net.ErrClosed
	}
	_ = c.conn.SetWriteDeadline(time.Now().Add(30 * time.Second))
	var header [14]byte
	header[0] = 0x80 | (opcode & 0x0F)
	n := len(data)
	hlen := 2
	var maskBit byte
	if c.isClient {
		maskBit = 0x80
	}
	switch {
	case n <= 125:
		header[1] = maskBit | byte(n)
	case n <= 65535:
		header[1] = maskBit | 126
		binary.BigEndian.PutUint16(header[2:4], uint16(n))
		hlen = 4
	default:
		header[1] = maskBit | 127
		binary.BigEndian.PutUint64(header[2:10], uint64(n))
		hlen = 10
	}
	var mask [4]byte
	if c.isClient {
		if _, err := rand.Read(mask[:]); err != nil {
			return err
		}
		copy(header[hlen:hlen+4], mask[:])
		hlen += 4
	}
	frame := make([]byte, hlen+n)
	copy(frame[:hlen], header[:hlen])
	if n > 0 {
		copy(frame[hlen:], data)
		if c.isClient {
			for i := 0; i < n; i++ {
				frame[hlen+i] ^= mask[i%4]
			}
		}
	}
	_, err := c.conn.Write(frame)
	return err
}

func (c *wsConn) readFrame() (fin bool, opcode byte, payload []byte, err error) {
	var hdr [2]byte
	if _, err = io.ReadFull(c.br, hdr[:]); err != nil {
		return false, 0, nil, err
	}
	fin = hdr[0]&0x80 != 0
	opcode = hdr[0] & 0x0F
	masked := hdr[1]&0x80 != 0
	length := uint64(hdr[1] & 0x7F)
	switch length {
	case 126:
		var ext [2]byte
		if _, err = io.ReadFull(c.br, ext[:]); err != nil {
			return false, 0, nil, err
		}
		length = uint64(binary.BigEndian.Uint16(ext[:]))
	case 127:
		var ext [8]byte
		if _, err = io.ReadFull(c.br, ext[:]); err != nil {
			return false, 0, nil, err
		}
		length = binary.BigEndian.Uint64(ext[:])
	}
	if length > wsMaxMessageSize {
		return false, 0, nil, errors.New("websocket frame exceeds maximum size")
	}
	var mask [4]byte
	if masked {
		if _, err = io.ReadFull(c.br, mask[:]); err != nil {
			return false, 0, nil, err
		}
	}
	if length > 0 {
		payload = make([]byte, length)
		if _, err = io.ReadFull(c.br, payload); err != nil {
			return false, 0, nil, err
		}
		if masked {
			for i := range payload {
				payload[i] ^= mask[i%4]
			}
		}
	}
	return fin, opcode, payload, nil
}

func (c *wsConn) ReadMessage() (byte, []byte, error) {
	var msgOp byte
	var buf []byte
	inFrag := false
	for {
		fin, op, payload, err := c.readFrame()
		if err != nil {
			return 0, nil, err
		}
		if c.onActivity != nil {
			c.onActivity()
		}
		switch op {
		case wsOpPing:
			if c.onPing != nil {
				c.onPing()
			}
			_ = c.writeFrame(wsOpPong, payload)
			continue
		case wsOpPong:
			continue
		case wsOpClose:
			code := uint16(1005)
			reason := ""
			if len(payload) >= 2 {
				code = binary.BigEndian.Uint16(payload[:2])
				reason = string(payload[2:])
			}
			_ = c.WriteClose(code, "")
			return 0, nil, &wsCloseError{Code: code, Reason: reason}
		case wsOpText, wsOpBinary:
			if inFrag {
				return 0, nil, errors.New("unexpected new websocket frame inside fragmented message")
			}
			if fin {
				return op, payload, nil
			}
			msgOp = op
			inFrag = true
			buf = append(buf[:0], payload...)
		case wsOpCont:
			if !inFrag {
				return 0, nil, errors.New("unexpected websocket continuation frame")
			}
			if len(buf)+len(payload) > wsMaxMessageSize {
				return 0, nil, errors.New("fragmented websocket message exceeds maximum size")
			}
			buf = append(buf, payload...)
			if fin {
				return msgOp, buf, nil
			}
		default:
			return 0, nil, fmt.Errorf("unsupported websocket opcode %d", op)
		}
	}
}
