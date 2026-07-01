// pqkeygen generates the strict-PQ staking keypairs (ML-DSA-65 + ML-KEM-768)
// that luxfi/node's strict-PQ profile requires at boot. Uses the node's own
// staking.InitNodePQKeyPair (ML-DSA) and crypto/mlkem (handshake KEM) so the
// PEM encoding and key bytes match config.go's loaders exactly.
package main

import (
	"bytes"
	"crypto/rand"
	"encoding/pem"
	"fmt"
	"os"
	"path/filepath"

	mlkemcrypto "github.com/luxfi/crypto/mlkem"
	"github.com/luxfi/node/staking"
)

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: pqkeygen <staking-dir>")
		os.Exit(2)
	}
	dir := os.Args[1]
	if err := os.MkdirAll(dir, 0o700); err != nil {
		fmt.Fprintln(os.Stderr, "mkdir:", err)
		os.Exit(1)
	}

	// ML-DSA-65 staking identity (mandatory under strict-PQ). InitNodePQKeyPair
	// is a no-op if the key already exists and writes FIPS 204 PEM.
	mldsaKey := filepath.Join(dir, "mldsa.key")
	mldsaPub := filepath.Join(dir, "mldsa.pub")
	if err := staking.InitNodePQKeyPair(mldsaKey, mldsaPub); err != nil {
		fmt.Fprintln(os.Stderr, "mldsa:", err)
		os.Exit(1)
	}

	// ML-KEM-768 handshake key (optional but matches the working node). Encode
	// with the exact PEM types config.loadHandshakeMLKEM expects.
	mlkemKey := filepath.Join(dir, "mlkem.key")
	mlkemPub := filepath.Join(dir, "mlkem.pub")
	if _, err := os.Stat(mlkemKey); os.IsNotExist(err) {
		pub, priv, err := mlkemcrypto.GenerateKeyPair(rand.Reader, mlkemcrypto.MLKEM768)
		if err != nil {
			fmt.Fprintln(os.Stderr, "mlkem gen:", err)
			os.Exit(1)
		}
		if err := writePEM(mlkemKey, "ML-KEM-768 PRIVATE KEY", priv.Bytes()); err != nil {
			fmt.Fprintln(os.Stderr, "mlkem key:", err)
			os.Exit(1)
		}
		if err := writePEM(mlkemPub, "ML-KEM-768 PUBLIC KEY", pub.Bytes()); err != nil {
			fmt.Fprintln(os.Stderr, "mlkem pub:", err)
			os.Exit(1)
		}
	}

	fmt.Println("OK: PQ staking keys ready in", dir)
}

func writePEM(path, typ string, der []byte) error {
	var buf bytes.Buffer
	if err := pem.Encode(&buf, &pem.Block{Type: typ, Bytes: der}); err != nil {
		return err
	}
	return os.WriteFile(path, buf.Bytes(), 0o600)
}
