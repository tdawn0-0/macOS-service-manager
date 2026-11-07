package devplatform

import (
	"context"
	"errors"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"testing"
	"time"
)

func crashDatabaseCaller(t *testing.T, m *Manager) DatabaseWorkInfo {
	t.Helper()
	c := exec.Command(os.Args[0], "-test.run=^TestOperationCrashHelper$")
	c.Env = append(os.Environ(), "MSM_OPERATION_CRASH_ROOT="+m.Root, "MSM_OPERATION_BACKUP_CRASH=1", "MSM_OPERATION_BACKUP_ID=first")
	if e := c.Start(); e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() {
		_ = c.Process.Kill()
		_ = c.Wait()
		records, _ := m.PendingDatabaseWork(context.Background())
		for _, r := range records {
			_ = m.ResolveDatabaseWork(context.Background(), r.ID, r.Revision)
		}
	})
	waitDatabaseTool(t, m, "source")
	if e := c.Process.Kill(); e != nil {
		t.Fatal(e)
	}
	_ = c.Wait()
	rows, e := m.PendingDatabaseWork(context.Background())
	deadline := time.Now().Add(3 * time.Second)
	for e == nil && len(rows) == 1 && rows[0].CallerActive && time.Now().Before(deadline) {
		time.Sleep(20 * time.Millisecond)
		rows, e = m.PendingDatabaseWork(context.Background())
	}
	if e != nil || len(rows) != 1 {
		t.Fatal(rows, e)
	}
	r := rows[0]
	if r.CallerActive || !r.Running || !r.CanResolve {
		t.Fatal(r)
	}
	records, e := m.Operations(context.Background())
	if e != nil || len(records) != 1 || records[0].Status != "Interrupted" {
		t.Fatal(records, e)
	}
	return r
}
func TestDatabaseWorkCallerDeathKeepsReservationAndCompletedBackup(t *testing.T) {
	m, p := databaseToolFixture(t)
	r := crashDatabaseCaller(t, m)
	ctx := context.Background()
	if e := m.ReplaceProxyRules(ctx, 0, 8443, []ProxyRule{{Domain: "during-orphan.test", LocalPort: 3000}}); e != nil {
		t.Fatal(e)
	}
	for _, action := range []string{"start", "stop", "restart", "remove"} {
		var busy *ResourceBusyError
		if e := m.InstanceAction(ctx, "first", action); !errors.As(e, &busy) {
			t.Fatal(action, e)
		}
	}
	if e := m.RepairPackage(ctx, p.ID()); e == nil {
		t.Fatal("repaired a worker's package")
	}
	if e := os.WriteFile(filepath.Join(m.Root, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	deadline := time.Now().Add(8 * time.Second)
	for time.Now().Before(deadline) {
		rows, e := m.PendingDatabaseWork(ctx)
		if e != nil || len(rows) != 1 {
			t.Fatal(rows, e)
		}
		r = rows[0]
		if !r.Running && r.Status == "Succeeded" {
			break
		}
		time.Sleep(25 * time.Millisecond)
	}
	if r.Status != "Succeeded" || r.Running || !strings.HasSuffix(r.Output, ".dump") {
		t.Fatal(r)
	}
	if e := m.RunDatabaseWorker(ctx, r.ID); e == nil {
		t.Fatal("replayed completed worker")
	}
	var busy *DatabaseWorkBusyError
	if e := m.InstanceAction(ctx, "first", "remove"); !errors.As(e, &busy) {
		t.Fatal("unreviewed result released instance", e)
	}
	b, e := os.ReadFile(r.Output)
	if e != nil || string(b) != "completed dump" {
		t.Fatal(string(b), e)
	}
	if e = m.ResolveDatabaseWork(ctx, r.ID, r.Revision); e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(r.Output); e != nil {
		t.Fatal("review removed backup", e)
	}
	rows, e := m.PendingDatabaseWork(ctx)
	if e != nil || len(rows) != 0 {
		t.Fatal(rows, e)
	}
	if e = m.CreateDatabase(ctx, "first", "after-review"); e != nil {
		t.Fatal(e)
	}
	records, _ := m.Operations(ctx)
	for _, record := range records {
		if record.Request.Kind == "backup" && record.Status != "Interrupted" {
			t.Fatal("invented caller success", record)
		}
	}
}
func TestDatabaseWorkReviewRefusesChangedOwnershipThenStopsOnlyWorker(t *testing.T) {
	m, _ := databaseToolFixture(t)
	r := crashDatabaseCaller(t, m)
	ctx := context.Background()
	path := filepath.Join(m.Root, "jobs", m.label("dbwork-"+r.ID)+".plist")
	original, e := os.ReadFile(path)
	if e != nil {
		t.Fatal(e)
	}
	t.Cleanup(func() { _ = os.WriteFile(path, original, 0600) })
	if e = os.WriteFile(path, append(append([]byte{}, original...), []byte("\n<!--changed-->\n")...), 0600); e != nil {
		t.Fatal(e)
	}
	rows, e := m.PendingDatabaseWork(ctx)
	if e != nil || len(rows) != 1 || rows[0].CanResolve {
		t.Fatal(rows, e)
	}
	if e = m.ResolveDatabaseWork(ctx, r.ID, rows[0].Revision); e == nil {
		t.Fatal("stopped changed job")
	}
	job, e := m.readJobState(ctx, "dbwork-"+r.ID)
	if e != nil || !job.Running {
		t.Fatal("ownership refusal stopped worker", job, e)
	}
	if e = os.WriteFile(path, original, 0600); e != nil {
		t.Fatal(e)
	}
	rows, e = m.PendingDatabaseWork(ctx)
	if e != nil || len(rows) != 1 {
		t.Fatal(rows, e)
	}
	if e = m.ResolveDatabaseWork(ctx, r.ID, "stale"); e == nil {
		t.Fatal("accepted stale review")
	}
	if e = m.ResolveDatabaseWork(ctx, r.ID, rows[0].Revision); e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(r.Output); e != nil {
		t.Fatal("orphan review removed diagnostic output", e)
	}
	if e = m.Restore(ctx, "first", "another", r.Output); e == nil {
		t.Fatal("restored partial output")
	}

	if e = m.verifyShutdownTarget(ctx, domain()+"/"+m.label("dbwork-"+r.ID)); e != nil {
		t.Fatal(e)
	}
	pidBytes, e := os.ReadFile(filepath.Join(m.Root, "pid-source"))
	if e != nil {
		t.Fatal(e)
	}
	childPID, e := strconv.Atoi(string(pidBytes))
	if e != nil || childPID <= 0 {
		t.Fatal("invalid live tool PID", e)
	}
	deadline := time.Now().Add(3 * time.Second)
	for time.Now().Before(deadline) && syscall.Kill(childPID, 0) != syscall.ESRCH {
		time.Sleep(20 * time.Millisecond)
	}
	if syscall.Kill(childPID, 0) != syscall.ESRCH {
		t.Fatal("stopping owned worker left its tool process alive", childPID)
	}

	s, e := m.Read()
	if e != nil || len(s.Instances) != 2 {
		t.Fatal("review removed database", s, e)
	}
	if _, e = os.Stat(filepath.Join(m.Root, "instances", "first", "password")); e != nil {
		t.Fatal("review removed credentials", e)
	}
	if e = os.WriteFile(filepath.Join(m.Root, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	if e = m.CreateDatabase(ctx, "first", "after-stop"); e != nil {
		t.Fatal("review retained lease", e)
	}
}

func TestDatabaseWorkCredentialsStayInPrivateInputAndDisappearAfterCompletion(t *testing.T) {
	m, _ := databaseToolFixture(t)
	ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
	defer cancel()
	secret := "private-" + randomID() + "-'quoted-密钥-value"
	done := make(chan error, 1)
	go func() { done <- m.CreateUser(ctx, "first", "app_user", secret, "first") }()
	defer func() {
		cancel()
		select {
		case <-done:
		case <-time.After(2 * time.Second):
		}
	}()
	waitDatabaseTool(t, m, "ON_ERROR_STOP=1")
	rows, e := m.PendingDatabaseWork(ctx)
	if e != nil || len(rows) != 1 || !rows[0].CallerActive || rows[0].CanResolve {
		t.Fatal(rows, e)
	}
	input := filepath.Join(m.Root, "database-work", rows[0].ID, "input.json")
	if info, e := os.Stat(input); e != nil || info.Mode().Perm() != 0600 {
		t.Fatal("credential input permissions", info, e)
	}
	for _, name := range []string{"record.json", "result.json"} {
		b, e := os.ReadFile(filepath.Join(filepath.Dir(input), name))
		if e == nil && strings.Contains(string(b), "private-") {
			t.Fatal("secret in task record", name)
		}
	}
	path := filepath.Join(m.Root, "jobs", m.label("dbwork-"+rows[0].ID)+".plist")
	if b, e := os.ReadFile(path); e != nil || strings.Contains(string(b), "private-") || strings.Contains(string(b), "app_user") {
		t.Fatal("credentials in worker arguments", e)
	}
	if e = os.WriteFile(filepath.Join(m.Root, "release"), nil, 0600); e != nil {
		t.Fatal(e)
	}
	e = <-done
	done <- e
	if e != nil {
		t.Fatal(e)
	}
	if _, e = os.Stat(input); !errors.Is(e, os.ErrNotExist) {
		t.Fatal("completed task retained credential input", e)
	}
	if e = filepath.Walk(m.Root, func(path string, info os.FileInfo, e error) error {
		if e != nil {
			return e
		}
		if info.Mode().IsRegular() {
			b, e := os.ReadFile(path)
			if e != nil {
				return e
			}
			if strings.Contains(string(b), secret) || strings.Contains(string(b), strings.ReplaceAll(secret, "'", "''")) {
				t.Errorf("secret retained in %s", path)
			}
		}
		return nil
	}); e != nil {
		t.Fatal(e)
	}
}
