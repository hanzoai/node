// hanzod is the Hanzo Network node — an L1 on the Lux Network.
//
// Same consensus (Quasar), same transport (ZAP), same stack as luxd.
//
// Usage:
//
//	hanzod                   Run the node
//	hanzod version           Print version
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"

	"github.com/spf13/pflag"

	"github.com/luxfi/node/config"
	nodeversion "github.com/luxfi/node/version"
)

func main() {
	if len(os.Args) > 1 {
		switch os.Args[1] {
		case "version", "--version", "-v":
			fmt.Printf("hanzod %s (luxd %s)\n", "0.1.0", nodeversion.Current)
			return
		}
	}

	runNode()
}

func runNode() {
	fs := config.BuildFlagSet()
	v, err := config.BuildViper(fs, os.Args[1:])

	if errors.Is(err, pflag.ErrHelp) {
		os.Exit(0)
	}
	if err != nil {
		fmt.Fprintf(os.Stderr, "config: %v\n", err)
		os.Exit(1)
	}

	if v.GetBool(config.VersionKey) {
		fmt.Println(nodeversion.GetVersions().String())
		os.Exit(0)
	}
	if v.GetBool(config.VersionJSONKey) {
		b, _ := json.MarshalIndent(nodeversion.GetVersions(), "", "  ")
		fmt.Println(string(b))
		os.Exit(0)
	}

	nc, err := config.GetNodeConfig(v)
	if err != nil {
		fmt.Fprintf(os.Stderr, "node config: %v\n", err)
		os.Exit(1)
	}

	if err := run(nc); err != nil {
		fmt.Fprintf(os.Stderr, "node: %v\n", err)
		os.Exit(1)
	}
}
