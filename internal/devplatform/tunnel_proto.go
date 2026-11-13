package devplatform

import (
	"crypto/hmac"
	"crypto/sha256"
	"crypto/subtle"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"time"
)

const (
	tunnelProtoVersion byte = 1
	tunnelHeaderSize        = 9
	tunnelMaxChunk          = 256 << 10 // 256 KiB

	tunnelFlagFIN  byte = 0x01
	tunnelFlagText byte = 0x02

	frameRegister    byte = 0x01
	frameRegisterAck byte = 0x02

	frameHTTPReqHead byte = 0x10
	frameHTTPReqData byte = 0x11
	frameHTTPResHead byte = 0x12
	frameHTTPResData byte = 0x13
	frameStreamReset byte = 0x14

	frameWSOpen    byte = 0x20
	frameWSOpenAck byte = 0x21
	frameWSData    byte = 0x22
	frameWSClose   byte = 0x23

	tunnelControlPing = "__devbase_ping__"
	tunnelControlPong = "__devbase_pong__"
)

type tunnelFrame struct {
	Type     byte
	Flags    byte
	StreamID uint32
	Code     uint16
	Payload  []byte
}

func encodeTunnelFrame(f tunnelFrame) []byte {
	out := make([]byte, tunnelHeaderSize+len(f.Payload))
	out[0] = tunnelProtoVersion
	out[1] = f.Type
	out[2] = f.Flags
	binary.BigEndian.PutUint32(out[3:7], f.StreamID)
	binary.BigEndian.PutUint16(out[7:9], f.Code)
	copy(out[tunnelHeaderSize:], f.Payload)
	return out
}

func decodeTunnelFrame(raw []byte) (tunnelFrame, error) {
	if len(raw) < tunnelHeaderSize {
		return tunnelFrame{}, errors.New("truncated tunnel frame")
	}
	if raw[0] != tunnelProtoVersion {
		return tunnelFrame{}, fmt.Errorf("unsupported tunnel frame version %d", raw[0])
	}
	return tunnelFrame{
		Type:     raw[1],
		Flags:    raw[2],
		StreamID: binary.BigEndian.Uint32(raw[3:7]),
		Code:     binary.BigEndian.Uint16(raw[7:9]),
		Payload:  raw[tunnelHeaderSize:],
	}, nil
}

type tunnelRegisterPayload struct {
	ClientID string `json:"clientId"`
}

type tunnelRegisterAckPayload struct {
	OK            bool   `json:"ok"`
	ConflictOwner string `json:"conflictOwner,omitempty"`
}

type tunnelHTTPReqHead struct {
	Method     string      `json:"method"`
	Host       string      `json:"host"`
	Path       string      `json:"path"`
	Headers    [][2]string `json:"headers"`
	RemoteAddr string      `json:"remoteAddr,omitempty"`
}

type tunnelHTTPResHead struct {
	Status  int         `json:"status"`
	Headers [][2]string `json:"headers"`
}

type tunnelWSOpenHead struct {
	Host      string      `json:"host"`
	Path      string      `json:"path"`
	Protocols []string    `json:"protocols,omitempty"`
	Headers   [][2]string `json:"headers,omitempty"`
}

type tunnelWSOpenAckHead struct {
	Protocol string `json:"protocol,omitempty"`
	Error    string `json:"error,omitempty"`
}

func encodeTunnelJSONFrame(frameType, flags byte, streamID uint32, code uint16, payload any) ([]byte, error) {
	b, err := json.Marshal(payload)
	if err != nil {
		return nil, err
	}
	return encodeTunnelFrame(tunnelFrame{
		Type:     frameType,
		Flags:    flags,
		StreamID: streamID,
		Code:     code,
		Payload:  b,
	}), nil
}

func signTunnelHandshake(secret, clientID, timestamp, nonce string) string {
	mac := hmac.New(sha256.New, []byte(secret))
	mac.Write([]byte("v1\n" + clientID + "\n" + timestamp + "\n" + nonce))
	return hex.EncodeToString(mac.Sum(nil))
}

func verifyTunnelHandshake(secret, clientID, timestamp, nonce, signature string, now time.Time) bool {
	if secret == "" || clientID == "" || nonce == "" || signature == "" {
		return false
	}
	ts, err := strconv.ParseInt(timestamp, 10, 64)
	if err != nil {
		return false
	}
	diff := now.Unix() - ts
	if diff < 0 {
		diff = -diff
	}
	if diff > 60 {
		return false
	}
	expected := signTunnelHandshake(secret, clientID, timestamp, nonce)
	if len(expected) != len(signature) {
		return false
	}
	return subtle.ConstantTimeCompare([]byte(expected), []byte(signature)) == 1
}
