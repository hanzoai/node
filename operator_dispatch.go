// hanzod operator / hanzod install — supervised handoff to the canonical
// Kubernetes operator.
//
// The operator is the canonical Rust reconcile loop with exactly ONE home:
// hanzoai/operator (image ghcr.io/hanzoai/operator). node vendors zero operator
// source. node's production image COPYs the operator binary from that image into
// /usr/local/bin/operator (docker/Dockerfile), and `hanzod operator …` /
// `hanzod install …` exec into it. One image runs both the node and the operator
// without fusing their runtimes: the Go node (luxfi/node) and the Rust operator
// do not share an address space — they compose over the Kubernetes API. This is
// the decomplected boundary: supervised multi-binary in one image, not FFI, and
// not a source fork.
package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"syscall"
)

// operatorBinary resolves the canonical operator executable, in order:
//  1. $HANZO_OPERATOR_BIN — explicit override,
//  2. `operator` on $PATH,
//  3. an `operator` sitting next to the hanzod executable (the image layout,
//     where both land in /usr/local/bin).
func operatorBinary() (string, error) {
	if p := os.Getenv("HANZO_OPERATOR_BIN"); p != "" {
		return p, nil
	}
	if p, err := exec.LookPath("operator"); err == nil {
		return p, nil
	}
	if self, err := os.Executable(); err == nil {
		cand := filepath.Join(filepath.Dir(self), "operator")
		if _, statErr := os.Stat(cand); statErr == nil {
			return cand, nil
		}
	}
	return "", fmt.Errorf(
		"operator binary not found: set HANZO_OPERATOR_BIN, put `operator` on PATH, " +
			"or co-locate it next to hanzod")
}

// execOperator replaces this process with `operator <args>` via execve — a
// supervised handoff, so signals and the exit status flow straight to the
// operator (it becomes PID 1's process in the container). It returns only if
// the exec itself fails.
func execOperator(args []string) error {
	bin, err := operatorBinary()
	if err != nil {
		return err
	}
	argv := append([]string{bin}, args...)
	return syscall.Exec(bin, argv, os.Environ())
}

// dispatchOperator handles `hanzod operator …` — the supervised reconcile loop.
// Args after `operator` pass through verbatim to the operator's own dispatcher
// (so `hanzod operator`, `hanzod operator --namespace x`, and
// `hanzod operator install …` all work); hanzod does not duplicate the
// operator's subcommand surface.
func dispatchOperator(args []string) {
	if err := execOperator(args); err != nil {
		fmt.Fprintf(os.Stderr, "hanzod operator: %v\n", err)
		os.Exit(1)
	}
}

// dispatchInstall handles `hanzod install …` — self-install as the in-cluster
// operator (applies the CRDs + the operator's own Deployment/RBAC into the
// current-context cluster). It is a named convenience over `operator install`;
// the container image it renders runs `hanzod operator`, closing the loop.
func dispatchInstall(args []string) {
	if err := execOperator(append([]string{"install"}, args...)); err != nil {
		fmt.Fprintf(os.Stderr, "hanzod install: %v\n", err)
		os.Exit(1)
	}
}
