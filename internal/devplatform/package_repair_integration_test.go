package devplatform

import (
	"context"
	"io"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"testing"
)

func TestRedisPackageRepairPreservesInstanceData(t *testing.T) {
	sourceRoot := os.Getenv("MSM_SIX_INTEGRATION_ROOT")
	if sourceRoot == "" {
		t.Skip("set MSM_SIX_INTEGRATION_ROOT to verified Redis storage")
	}
	source := New(sourceRoot)
	state, err := source.Read()
	if err != nil {
		t.Fatal(err)
	}
	p, ok := state.Packages["redis@8.2.11-"+runtime.GOARCH]
	if !ok {
		t.Fatal("verified Redis package is required")
	}
	m := testManager(t)
	ctx := context.Background()
	in, err := os.Open(filepath.Join(sourceRoot, "downloads", p.SHA256))
	if err != nil {
		t.Fatal(err)
	}
	defer in.Close()
	out, err := os.Create(filepath.Join(m.Root, "downloads", p.SHA256))
	if err != nil {
		t.Fatal(err)
	}
	_, err = io.Copy(out, in)
	closeErr := out.Close()
	if err != nil || closeErr != nil {
		t.Fatal(err, closeErr)
	}
	if err = m.Install(ctx, p); err != nil {
		t.Fatal(err)
	}
	instance := Instance{ID: "repair-data", Kind: "redis", Package: p.ID(), Port: integrationPort(t), Persistent: true}
	if err = m.CreateInstance(ctx, instance); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := m.InstanceAction(context.Background(), instance.ID, "remove"); err != nil {
			t.Error(err)
		}
	})
	if err = m.InstanceAction(ctx, instance.ID, "start"); err != nil {
		t.Fatal(err)
	}
	if _, err = m.redisCommand(ctx, instance, 0, []byte("SET"), []byte("repair-value"), []byte("42")); err != nil {
		t.Fatal(err)
	}
	if err = m.RepairPackage(ctx, p.ID()); err == nil || !strings.Contains(err.Error(), "stop database") {
		t.Fatal("replaced database software while it was running", err)
	}
	if err = m.InstanceAction(ctx, instance.ID, "stop"); err != nil {
		t.Fatal(err)
	}
	binary := filepath.Join(m.packageDir(p), p.Binaries["redis-cli"])
	if err = os.Remove(binary); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Needs repair")
	if err = m.RepairPackage(ctx, p.ID()); err != nil {
		t.Fatal(err)
	}
	assertIntegrity(t, m, p, "Verified")
	if err = m.InstanceAction(ctx, instance.ID, "start"); err != nil {
		t.Fatal(err)
	}
	value, err := m.redisCommand(ctx, instance, 0, []byte("GET"), []byte("repair-value"))
	if err != nil {
		t.Fatal(err)
	}
	data, ok := value.([]byte)
	if !ok || string(data) != "42" {
		t.Fatal("database contents changed after software repair", value)
	}
}
