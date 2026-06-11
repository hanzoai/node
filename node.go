// hanzod node lifecycle.
//
// run constructs the luxfi/node application from the resolved node config,
// starts it, and drives a signal-driven graceful shutdown. The App wrapper
// (github.com/luxfi/node/app) gives us the same lifecycle luxd uses: Start
// returns immediately, Stop signals shutdown, and ExitCode blocks until the
// node has fully finished so the process exits only once teardown completes.
package main

import (
	"fmt"
	"os"
	"os/signal"
	"syscall"

	"github.com/luxfi/node/app"
	nodeconfig "github.com/luxfi/node/config/node"
)

func run(nodeConfig nodeconfig.Config) error {
	nodeApp, err := app.New(nodeConfig)
	if err != nil {
		return fmt.Errorf("init: %w", err)
	}

	nodeApp.Start()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	<-sigCh

	fmt.Fprintln(os.Stderr, "\n[hanzod] shutting down")
	nodeApp.Stop()

	if code := nodeApp.ExitCode(); code != 0 {
		return fmt.Errorf("node exited with code %d", code)
	}
	return nil
}
