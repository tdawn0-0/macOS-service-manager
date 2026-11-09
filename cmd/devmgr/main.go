package main

import (
	"context"
	"fmt"
	"devbase/internal/devcli"
	"os"
	"os/signal"
	"syscall"
)

// Command devmgr is the test and verification harness of the development
// platform: the tests and scripts/ drive the platform through it. It is not
// shipped, documented for users or maintained as an interface; the app has
// no command line.
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
