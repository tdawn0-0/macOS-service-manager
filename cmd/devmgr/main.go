package main

import (
	"context"
	"fmt"
	"macos-service-manager/internal/devcli"
	"os"
	"os/signal"
	"syscall"
)

func main() {
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	args := os.Args[1:]
	if len(args) > 0 && args[0] == "--dev" {
		args = args[1:]
	}
	if e := devcli.Run(ctx, args, os.Stdout); e != nil {
		fmt.Fprintln(os.Stderr, e)
		os.Exit(1)
	}
}
