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
	return nil
}
