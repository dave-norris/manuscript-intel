// Package cdp provides Chrome DevTools Protocol connectivity for Publisher Rocket.
//
// Publisher Rocket is an Electron app. This package launches it with
// --remote-debugging-port=9222, connects via raw WebSocket, and exposes
// Eval/Click helpers that mirror the TypeScript cdp.ts from the VS Code extension.
package cdp

import (
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"time"
)

const (
	cdpHost        = "127.0.0.1"
	cdpPort        = 9222
	connectTimeout = 45 * time.Second
	pollInterval   = 500 * time.Millisecond
	rocketAppPath  = "/Applications/Publisher Rocket.app"
)

// ── Result types ─────────────────────────────────────────────────────────────

// StatusResult is returned by CheckStatus and surfaced to the frontend.
type StatusResult struct {
	Running    bool   `json:"running"`
	CDPEnabled bool   `json:"cdpEnabled"`
	PageID     string `json:"pageId"`
	Error      string `json:"error,omitempty"`
}

// LaunchResult is returned by EnsureRocket and surfaced to the frontend.
type LaunchResult struct {
	Success bool   `json:"success"`
	PageID  string `json:"pageId"`
	Error   string `json:"error,omitempty"`
}

// PageTarget mirrors the CDP /json response object.
type PageTarget struct {
	ID                   string `json:"id"`
	Title                string `json:"title"`
	WebSocketDebuggerURL string `json:"webSocketDebuggerUrl"`
}

// Session holds an open CDP WebSocket connection.
type Session struct {
	conn   net.Conn
	cmdID  int
	buf    []byte
}

// ── Binary helpers ────────────────────────────────────────────────────────────

// resolveMacBinary reads Info.plist to find the inner Mach-O binary name.
func resolveMacBinary(appPath string) (string, error) {
	plistPath := filepath.Join(appPath, "Contents", "Info.plist")
	data, err := os.ReadFile(plistPath)
	if err != nil {
		return "", fmt.Errorf("cannot read Info.plist: %w", err)
	}
	re := regexp.MustCompile(`<key>CFBundleExecutable</key>\s*<string>([^<]+)</string>`)
	m := re.FindSubmatch(data)
	if m == nil {
		return "", fmt.Errorf("CFBundleExecutable not found in Info.plist")
	}
	return filepath.Join(appPath, "Contents", "MacOS", string(m[1])), nil
}

// cleanEnv returns a copy of the current environment with Electron/VS Code
// vars scrubbed so the child process launches as a normal Electron app.
func cleanEnv() []string {
	var env []string
	for _, kv := range os.Environ() {
		key := strings.SplitN(kv, "=", 2)[0]
		switch key {
		case "ELECTRON_RUN_AS_NODE", "ELECTRON_NO_ATTACH_CONSOLE", "ELECTRON_NO_ASAR", "NODE_OPTIONS":
			continue
		}
		env = append(env, kv)
	}
	return env
}

// ── Launch ────────────────────────────────────────────────────────────────────

// LaunchRocket spawns Publisher Rocket with --remote-debugging-port=9222.
// Returns an error if the process exits immediately.
func LaunchRocket() error {
	exe, err := resolveMacBinary(rocketAppPath)
	if err != nil {
		return fmt.Errorf("cannot resolve Publisher Rocket binary: %w", err)
	}

	cmd := exec.Command(exe, fmt.Sprintf("--remote-debugging-port=%d", cdpPort))
	cmd.Env = cleanEnv()
	cmd.Stdout = io.Discard
	cmd.Stderr = io.Discard

	if err := cmd.Start(); err != nil {
		return fmt.Errorf("failed to spawn Publisher Rocket: %w", err)
	}

	// Give the process 2s to either crash or start successfully.
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()

	select {
	case err := <-done:
		return fmt.Errorf("Publisher Rocket exited immediately: %v", err)
	case <-time.After(2 * time.Second):
		// Still running — detach and return.
		return nil
	}
}

// QuitRocket quits Publisher Rocket via AppleScript.
func QuitRocket() error {
	out, err := exec.Command("osascript", "-e", `quit app "Publisher Rocket"`).CombinedOutput()
	if err != nil {
		return fmt.Errorf("failed to quit Publisher Rocket: %s (%w)", strings.TrimSpace(string(out)), err)
	}
	return nil
}

// ── CDP /json polling ─────────────────────────────────────────────────────────

// GetPageTarget queries http://127.0.0.1:9222/json and returns the Publisher
// Rocket page target, or an error if none is found.
func GetPageTarget() (*PageTarget, error) {
	client := &http.Client{Timeout: 3 * time.Second}
	resp, err := client.Get(fmt.Sprintf("http://%s:%d/json", cdpHost, cdpPort))
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var pages []PageTarget
	if err := json.NewDecoder(resp.Body).Decode(&pages); err != nil {
		return nil, err
	}

	for i, p := range pages {
		title := strings.ToLower(p.Title)
		if p.Title == "Publisher Rocket" || p.Title == "KDP Rocket" || strings.Contains(title, "rocket") {
			// Rewrite localhost → 127.0.0.1 to avoid Node 17+ IPv6 trap.
			pages[i].WebSocketDebuggerURL = strings.ReplaceAll(
				p.WebSocketDebuggerURL, "ws://localhost:", fmt.Sprintf("ws://%s:", cdpHost),
			)
			return &pages[i], nil
		}
	}
	return nil, fmt.Errorf("no Publisher Rocket page target found on port %d", cdpPort)
}

// IsPortOpen returns true if port 9222 is accepting TCP connections.
func IsPortOpen() bool {
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("%s:%d", cdpHost, cdpPort), 2*time.Second)
	if err != nil {
		return false
	}
	conn.Close()
	return true
}

// PollForTarget polls /json until a Rocket target appears or the timeout expires.
func PollForTarget() (*PageTarget, error) {
	deadline := time.Now().Add(connectTimeout)
	for time.Now().Before(deadline) {
		if t, err := GetPageTarget(); err == nil {
			return t, nil
		}
		time.Sleep(pollInterval)
	}
	return nil, fmt.Errorf("CDP connection timed out after %s — Publisher Rocket did not expose a page target on port %d", connectTimeout, cdpPort)
}

// EnsureRocket handles all three cases:
//  1. Port not open → launch, poll.
//  2. Port open, no Rocket target → quit, relaunch, poll.
//  3. Port open, Rocket target found → return it.
func EnsureRocket() (*PageTarget, error) {
	target, err := GetPageTarget()
	if err == nil {
		return target, nil
	}

	if !IsPortOpen() {
		// Case 1: launch fresh.
		if err := LaunchRocket(); err != nil {
			return nil, err
		}
		time.Sleep(5 * time.Second)
		return PollForTarget()
	}

	// Case 2: port open but no Rocket target — quit and relaunch.
	_ = QuitRocket()
	time.Sleep(2 * time.Second)
	if err := LaunchRocket(); err != nil {
		return nil, err
	}
	time.Sleep(5 * time.Second)
	return PollForTarget()
}

// ── WebSocket session ─────────────────────────────────────────────────────────

// Connect opens a raw WebSocket connection to the CDP page endpoint.
func Connect(target *PageTarget) (*Session, error) {
	conn, err := net.DialTimeout("tcp", fmt.Sprintf("%s:%d", cdpHost, cdpPort), 5*time.Second)
	if err != nil {
		return nil, fmt.Errorf("CDP TCP connect failed: %w", err)
	}

	// WebSocket handshake.
	handshake := fmt.Sprintf(
		"GET /devtools/page/%s HTTP/1.1\r\nHost: %s:%d\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: YXV0aG9ydG9vbHMxMjM0PT0=\r\nSec-WebSocket-Version: 13\r\n\r\n",
		target.ID, cdpHost, cdpPort,
	)
	if _, err := conn.Write([]byte(handshake)); err != nil {
		conn.Close()
		return nil, fmt.Errorf("CDP handshake write failed: %w", err)
	}

	// Read past the HTTP 101 response.
	tmp := make([]byte, 4096)
	conn.SetReadDeadline(time.Now().Add(5 * time.Second))
	if _, err := conn.Read(tmp); err != nil {
		conn.Close()
		return nil, fmt.Errorf("CDP handshake read failed: %w", err)
	}
	conn.SetReadDeadline(time.Time{})

	return &Session{conn: conn, cmdID: 1}, nil
}

// Close closes the CDP session.
func (s *Session) Close() {
	if s.conn != nil {
		s.conn.Close()
	}
}

// ── Send / Receive ────────────────────────────────────────────────────────────

// send writes a masked WebSocket text frame containing a CDP JSON message.
func (s *Session) send(method string, params map[string]interface{}) (int, error) {
	id := s.cmdID
	s.cmdID++

	payload, err := json.Marshal(map[string]interface{}{"id": id, "method": method, "params": params})
	if err != nil {
		return 0, err
	}

	mask := [4]byte{0xfe, 0xed, 0xbe, 0xef}
	masked := make([]byte, len(payload))
	for i, b := range payload {
		masked[i] = b ^ mask[i%4]
	}

	var header []byte
	if len(payload) <= 125 {
		header = []byte{0x81, byte(0x80 | len(payload))}
	} else {
		header = []byte{0x81, 0x80 | 126, byte(len(payload) >> 8), byte(len(payload))}
	}

	frame := append(header, mask[:]...)
	frame = append(frame, masked...)
	_, err = s.conn.Write(frame)
	return id, err
}

// recv reads WebSocket frames until it finds the response for targetID.
func (s *Session) recv(targetID int, timeout time.Duration) (interface{}, error) {
	s.conn.SetReadDeadline(time.Now().Add(timeout))
	defer s.conn.SetReadDeadline(time.Time{})

	for {
		header := make([]byte, 2)
		if _, err := io.ReadFull(s.conn, header); err != nil {
			return nil, fmt.Errorf("CDP recv header: %w", err)
		}

		isMasked := (header[1] & 0x80) != 0
		plen := int(header[1] & 0x7f)

		if plen == 126 {
			ext := make([]byte, 2)
			if _, err := io.ReadFull(s.conn, ext); err != nil {
				return nil, err
			}
			plen = int(ext[0])<<8 | int(ext[1])
		}

		if isMasked {
			if _, err := io.ReadFull(s.conn, make([]byte, 4)); err != nil {
				return nil, err
			}
		}

		data := make([]byte, plen)
		if _, err := io.ReadFull(s.conn, data); err != nil {
			return nil, err
		}

		var msg map[string]interface{}
		if err := json.Unmarshal(data, &msg); err != nil {
			continue
		}

		if id, ok := msg["id"].(float64); ok && int(id) == targetID {
			if result, ok := msg["result"].(map[string]interface{}); ok {
				if r, ok := result["result"].(map[string]interface{}); ok {
					return r["value"], nil
				}
			}
			return msg, nil
		}
	}
}

// Eval executes JavaScript in Publisher Rocket's renderer and returns the result as a string.
func (s *Session) Eval(expression string, timeout time.Duration) (string, error) {
	wrapped := fmt.Sprintf("(function(){ %s })()", expression)
	id, err := s.send("Runtime.evaluate", map[string]interface{}{
		"expression":    wrapped,
		"returnByValue": true,
	})
	if err != nil {
		return "", err
	}

	result, err := s.recv(id, timeout)
	if err != nil {
		return "", err
	}
	if result == nil {
		return "", nil
	}
	if s, ok := result.(string); ok {
		return s, nil
	}
	b, _ := json.Marshal(result)
	return string(b), nil
}

// Click dispatches a mouse press+release at (x, y) in the Rocket renderer.
func (s *Session) Click(x, y float64) error {
	params := map[string]interface{}{
		"type": "mousePressed", "x": x, "y": y,
		"button": "left", "clickCount": 1,
	}
	if _, err := s.send("Input.dispatchMouseEvent", params); err != nil {
		return err
	}
	time.Sleep(80 * time.Millisecond)

	params["type"] = "mouseReleased"
	if _, err := s.send("Input.dispatchMouseEvent", params); err != nil {
		return err
	}
	time.Sleep(80 * time.Millisecond)
	return nil
}

// KeyEscape dispatches an Escape keydown+keyup into the Rocket renderer.
func (s *Session) KeyEscape() {
	params := map[string]interface{}{"type": "keyDown", "key": "Escape", "code": "Escape"}
	s.send("Input.dispatchKeyEvent", params)
	time.Sleep(80 * time.Millisecond)
	params["type"] = "keyUp"
	s.send("Input.dispatchKeyEvent", params)
	time.Sleep(200 * time.Millisecond)
}
