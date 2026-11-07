package main

import (
	"context"
	"macos-service-manager/internal/devplatform"
	"os"
	"os/signal"
	"syscall"
	"testing"
)

func TestMain(m *testing.M) {
	if len(os.Args) == 5 && os.Args[1] == "--dev" && (os.Args[2] == "database-worker" || os.Args[2] == "command-worker") {
		ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		defer cancel()
		worker := devplatform.New(os.Args[3])
		var e error
		if os.Args[2] == "command-worker" {
			e = worker.RunProjectCommandWorker(ctx, os.Args[4])
		} else {
			e = worker.RunDatabaseWorker(ctx, os.Args[4])
		}
		if e != nil {
			os.Exit(1)
		}
		os.Exit(0)
	}
	os.Exit(m.Run())
}
