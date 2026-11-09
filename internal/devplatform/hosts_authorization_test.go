package devplatform

import (
	"bytes"
	"crypto/sha1"
	"encoding/base64"
	"encoding/hex"
	"devbase/internal/system"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strings"
	"testing"
)

func TestAuthorizedHostsPayload(t *testing.T) {
	old := []byte("127.0.0.1 localhost\n")
	updated := []byte("127.0.0.1 localhost\n# quoted ' 字段\n127.0.0.1 app.test\n")
	script, err := hostsInstallScript(old, updated)
	if err != nil {
		t.Fatal(err)
	}
	m := regexp.MustCompile(`/usr/bin/printf '%s' '([A-Za-z0-9+/=]+)'`).FindStringSubmatch(script)
	if len(m) != 2 {
		t.Fatal("missing fixed payload")
	}
	b, e := base64.StdEncoding.DecodeString(m[1])
	if e != nil || !bytes.Equal(b, updated) {
		t.Fatal("payload changed", e)
	}
	if strings.Contains(script, "msm-hosts-") {
		t.Fatal("user staging read")
	}
	for _, want := range []string{"/etc/.msm-hosts.", "/usr/bin/shasum /etc/hosts", "/bin/mv", "trap"} {
		if !strings.Contains(script, want) {
			t.Fatal("missing protection", want)
		}
	}
	sum := sha1.Sum(old)
	hash := system.ShellQuote(hex.EncodeToString(sum[:]))
	if strings.Count(script, hash) != 2 {
		t.Fatal("old content hash not checked twice")
	}
	if strings.LastIndex(script, hash) > strings.Index(script, "/bin/mv") {
		t.Fatal("replacement precedes final conflict check")
	}
	if out, e := exec.Command("/bin/sh", "-n", "-c", script).CombinedOutput(); e != nil {
		t.Fatal(e, string(out))
	}
}

func TestAuthorizedHostsPayloadRejectsOversize(t *testing.T) {
	if _, e := hostsInstallScript(nil, []byte(strings.Repeat("x", 65<<10))); e == nil {
		t.Fatal("accepted oversized command")
	}
}
func TestAuthorizedHostsPayloadConflictAndPermissions(t *testing.T) {
	for _, conflict := range []string{"none", "before", "after-stage"} {
		t.Run(conflict, func(t *testing.T) {
			dir := t.TempDir()
			path := filepath.Join(dir, "hosts")
			old := []byte("127.0.0.1 localhost\n")
			updated := []byte("127.0.0.1 localhost\n127.0.0.1 app.test\n")
			if e := os.WriteFile(path, old, 0640); e != nil {
				t.Fatal(e)
			}
			script, e := hostsInstallScript(old, updated)
			if e != nil {
				t.Fatal(e)
			}
			// Execute only against a private fixture, with ownership/cache operations stubbed.
			script = strings.ReplaceAll(script, "/etc/hosts", system.ShellQuote(path))
			script = strings.ReplaceAll(script, "/etc/.msm-hosts.XXXXXX", system.ShellQuote(filepath.Join(dir, ".msm-hosts.XXXXXX")))
			script = strings.ReplaceAll(script, "/usr/sbin/chown", "/usr/bin/true")
			script = strings.ReplaceAll(script, "/usr/bin/dscacheutil -flushcache", "/usr/bin/true")
			foreign := []byte("# separately edited\n")
			if conflict == "before" {
				if e = os.WriteFile(path, foreign, 0640); e != nil {
					t.Fatal(e)
				}
			}
			if conflict == "after-stage" {
				at := strings.LastIndex(script, "[ -f ")
				script = script[:at] + "/usr/bin/printf '%s' '# separately edited\n' > " + system.ShellQuote(path) + " && " + script[at:]
			}
			out, e := exec.Command("/bin/sh", "-c", script).CombinedOutput()
			got, readErr := os.ReadFile(path)
			if readErr != nil {
				t.Fatal(readErr)
			}
			if conflict == "none" {
				if e != nil || !bytes.Equal(got, updated) {
					t.Fatal("fixture update failed", e, string(out))
				}
			} else if e == nil || !bytes.Equal(got, foreign) {
				t.Fatal("conflicting update overwritten", e)
			}
			st, e := os.Stat(path)
			if e != nil || st.Mode().Perm() != 0640 {
				t.Fatal("permissions changed", e)
			}
			left, e := filepath.Glob(filepath.Join(dir, ".msm-hosts.*"))
			if e != nil || len(left) != 0 {
				t.Fatal("stage leaked", e)
			}
		})
	}
}
