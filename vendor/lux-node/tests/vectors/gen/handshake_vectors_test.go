// Copyright (C) 2019-2026, Lux Industries Inc. All rights reserved.
// SPDX-License-Identifier: BSD-3-Clause-Eco

// Conformance vectors for the peer post-quantum handshake, produced BY the
// Go node's own handshake code.
//
// The file is `package peer` because the encodings it has to pin —
// transcriptPrefix, canonicalBytes, bindAEADTranscript — are unexported. It
// is not kept in the Go tree: it is injected into `mesh/peer` at build time
// through a go build overlay, so the Go working copy is read and never
// written, and every byte below comes out of handshake.go as it stands.
//
//	cd <the Go node checkout>
//	cat > /tmp/pqvec/overlay.json <<'EOF'
//	{"Replace":{"<checkout>/mesh/peer/handshake_vectors_test.go":"<this file>"}}
//	EOF
//	PQ_VECTORS_OUT=<rust>/tests/vectors/pq_handshake.json \
//	  go test -overlay /tmp/pqvec/overlay.json -run TestWritePQHandshakeVectors ./mesh/peer/
//
// Determinism comes from seeds, not from a fixed RNG: ML-DSA-65 keys are
// derived with NewKeyFromSeed (FIPS 204 ξ), ML-KEM-768/1024 keys with
// NewKeyFromSeed (FIPS 203 d‖z), the encapsulation with EncapsulateTo under an
// explicit m, and every signature in deterministic mode. The same seeds fed to
// any conforming implementation have to produce the same bytes, which is the
// whole point: a reader in another language reproduces these without ever
// touching Go.
package peer

import (
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"

	"github.com/cloudflare/circl/kem/mlkem/mlkem1024"
	"github.com/cloudflare/circl/kem/mlkem/mlkem768"
	"github.com/cloudflare/circl/sign/mldsa/mldsa65"

	"github.com/luxfi/ids"
	"github.com/luxfi/node/mesh/kem"
)

// vectorSide is one peer's half of a handshake: the seeds it was built from,
// the public values those seeds produce, and the two byte strings it signs
// over and puts on the wire.
type vectorSide struct {
	MLDSASeed string `json:"mldsaSeed"`
	MLDSAPub  string `json:"mldsaPub"`
	NodeID    string `json:"nodeId"`
	KEMSeed   string `json:"kemSeed,omitempty"`   // initiator: FIPS 203 d‖z
	KEMPub    string `json:"kemPub,omitempty"`    // initiator
	EncapSeed string `json:"encapSeed,omitempty"` // responder: FIPS 203 m
	KEMCt     string `json:"kemCiphertext,omitempty"`
	Prefix    string `json:"transcriptPrefix"`
	Sig       string `json:"sig"`
	Canonical string `json:"canonical"`
}

// vector is one complete handshake, from seeds to session key.
type vector struct {
	Name            string     `json:"name"`
	ProtocolVersion uint8      `json:"protocolVersion"`
	Profile         uint8      `json:"profile"`
	ChainID         string     `json:"chainId"`
	KEMScheme       uint8      `json:"kemScheme"`
	Init            vectorSide `json:"init"`
	Resp            vectorSide `json:"resp"`
	SharedSecret    string     `json:"sharedSecret"`
	TranscriptHash  string     `json:"transcriptHash"`
	AEADKey         string     `json:"aeadKey"`

	// LegacyContextSig is the initiator's signature over the very same
	// transcript prefix under the context string the Rust node used before it
	// spoke this protocol. A conforming verifier refuses it: the context is
	// part of the signed message, so the two are different signatures over
	// what looks like the same bytes.
	LegacyContextSig string `json:"legacyContextSig"`
}

type vectorFile struct {
	Note           string   `json:"note"`
	Producer       string   `json:"producer"`
	SigContextInit string   `json:"sigContextInitiator"`
	SigContextResp string   `json:"sigContextResponder"`
	LegacyContext  string   `json:"legacyContext"`
	Vectors        []vector `json:"vectors"`
}

// legacySigContext is the context the pre-conformance Rust handshake signed
// its transcript under. Carried here so the Rust test can prove the refusal
// rather than assert it.
const legacySigContext = "lux-validator-link-v1"

// fill returns n bytes derived from tag, so a vector's seeds are reproducible
// from the vector's own name and nothing else has to be stored to regenerate
// them.
func fill(tag string, n int) []byte {
	out := make([]byte, n)
	h := kem.HashTranscript([]byte(tag))
	for i := 0; i < n; i++ {
		if i%48 == 0 && i > 0 {
			h = kem.HashTranscript(h[:])
		}
		out[i] = h[i%48]
	}
	return out
}

// kemKeypair derives a scheme-appropriate ML-KEM keypair from seed (d‖z).
func kemKeypair(t *testing.T, scheme kem.KeyExchangeID, seed []byte) (pub, sec []byte) {
	t.Helper()
	switch scheme {
	case kem.KeyExchangeMLKEM768:
		if len(seed) != mlkem768.KeySeedSize {
			t.Fatalf("ML-KEM-768 seed is %d bytes, want %d", len(seed), mlkem768.KeySeedSize)
		}
		pk, sk := mlkem768.NewKeyFromSeed(seed)
		pub = make([]byte, mlkem768.PublicKeySize)
		sec = make([]byte, mlkem768.PrivateKeySize)
		pk.Pack(pub)
		sk.Pack(sec)
	case kem.KeyExchangeMLKEM1024:
		if len(seed) != mlkem1024.KeySeedSize {
			t.Fatalf("ML-KEM-1024 seed is %d bytes, want %d", len(seed), mlkem1024.KeySeedSize)
		}
		pk, sk := mlkem1024.NewKeyFromSeed(seed)
		pub = make([]byte, mlkem1024.PublicKeySize)
		sec = make([]byte, mlkem1024.PrivateKeySize)
		pk.Pack(pub)
		sk.Pack(sec)
	default:
		t.Fatalf("no ML-KEM variant for scheme 0x%02x", uint8(scheme))
	}
	return pub, sec
}

// kemEncapsulate encapsulates to pub under an explicit FIPS 203 m, so the
// ciphertext and shared secret are a function of the vector's seeds.
func kemEncapsulate(t *testing.T, scheme kem.KeyExchangeID, pub, m []byte) (ct, ss []byte) {
	t.Helper()
	switch scheme {
	case kem.KeyExchangeMLKEM768:
		var pk mlkem768.PublicKey
		if err := pk.Unpack(pub); err != nil {
			t.Fatalf("unpack ML-KEM-768 public key: %v", err)
		}
		ct = make([]byte, mlkem768.CiphertextSize)
		ss = make([]byte, mlkem768.SharedKeySize)
		pk.EncapsulateTo(ct, ss, m)
	case kem.KeyExchangeMLKEM1024:
		var pk mlkem1024.PublicKey
		if err := pk.Unpack(pub); err != nil {
			t.Fatalf("unpack ML-KEM-1024 public key: %v", err)
		}
		ct = make([]byte, mlkem1024.CiphertextSize)
		ss = make([]byte, mlkem1024.SharedKeySize)
		pk.EncapsulateTo(ct, ss, m)
	default:
		t.Fatalf("no ML-KEM variant for scheme 0x%02x", uint8(scheme))
	}
	return ct, ss
}

// identityFromSeed derives an ML-DSA-65 identity and the NodeID that key
// answers to, under the node-identity domain (ids.Empty) the running node
// binds — verifyPQIdentityBinding, unchanged.
func identityFromSeed(t *testing.T, seed []byte) *LocalIdentity {
	t.Helper()
	var xi [mldsa65.SeedSize]byte
	copy(xi[:], seed)
	pub, sec := mldsa65.NewKeyFromSeed(&xi)
	nodeID, _, err := ids.NodeIDSchemeMLDSA65.DeriveMLDSA(ids.Empty, packMLDSAPub(pub))
	if err != nil {
		t.Fatalf("derive NodeID: %v", err)
	}
	return &LocalIdentity{NodeID: nodeID, Public: pub, Secret: sec}
}

// spec is what a vector is generated from.
type spec struct {
	name    string
	profile ProfileID
	chain   [chainIDSize]byte
	scheme  kem.KeyExchangeID
}

func TestWritePQHandshakeVectors(t *testing.T) {
	out := os.Getenv("PQ_VECTORS_OUT")
	if out == "" {
		t.Skip("PQ_VECTORS_OUT is not set; nothing to write")
	}

	var chainZero, chainPrimary, chainOdd [chainIDSize]byte
	copy(chainPrimary[:], fill("chain/primary", chainIDSize))
	for i := range chainOdd {
		chainOdd[i] = byte(i)
	}

	specs := []spec{
		{"mlkem768/strict-pq/chain-zero/a", ProfileStrictPQ, chainZero, kem.KeyExchangeMLKEM768},
		{"mlkem768/strict-pq/chain-zero/b", ProfileStrictPQ, chainZero, kem.KeyExchangeMLKEM768},
		{"mlkem768/strict-pq/chain-zero/c", ProfileStrictPQ, chainZero, kem.KeyExchangeMLKEM768},
		{"mlkem768/strict-pq/chain-zero/d", ProfileStrictPQ, chainZero, kem.KeyExchangeMLKEM768},
		{"mlkem768/strict-pq/chain-primary/a", ProfileStrictPQ, chainPrimary, kem.KeyExchangeMLKEM768},
		{"mlkem768/strict-pq/chain-primary/b", ProfileStrictPQ, chainPrimary, kem.KeyExchangeMLKEM768},
		{"mlkem768/strict-pq/chain-counting/a", ProfileStrictPQ, chainOdd, kem.KeyExchangeMLKEM768},
		{"mlkem768/fips/chain-zero/a", ProfileFIPS, chainZero, kem.KeyExchangeMLKEM768},
		{"mlkem768/fips/chain-primary/a", ProfileFIPS, chainPrimary, kem.KeyExchangeMLKEM768},
		{"mlkem768/permissive/chain-zero/a", ProfilePermissive, chainZero, kem.KeyExchangeMLKEM768},
		{"mlkem768/permissive/chain-counting/a", ProfilePermissive, chainOdd, kem.KeyExchangeMLKEM768},
		{"mlkem1024/strict-pq/chain-zero/a", ProfileStrictPQ, chainZero, kem.KeyExchangeMLKEM1024},
		{"mlkem1024/strict-pq/chain-primary/a", ProfileStrictPQ, chainPrimary, kem.KeyExchangeMLKEM1024},
		{"mlkem1024/strict-pq/chain-counting/a", ProfileStrictPQ, chainOdd, kem.KeyExchangeMLKEM1024},
		{"mlkem1024/fips/chain-primary/a", ProfileFIPS, chainPrimary, kem.KeyExchangeMLKEM1024},
		{"mlkem1024/permissive/chain-zero/a", ProfilePermissive, chainZero, kem.KeyExchangeMLKEM1024},
	}

	file := vectorFile{
		Note: "Peer post-quantum handshake conformance vectors, emitted by mesh/peer/handshake.go " +
			"through a build overlay. Every hex string is produced by the Go node's own code.",
		Producer:       "github.com/luxfi/node mesh/peer",
		SigContextInit: "NODE_PQ_HANDSHAKE_V1/initiator",
		SigContextResp: "NODE_PQ_HANDSHAKE_V1/responder",
		LegacyContext:  legacySigContext,
	}

	for _, s := range specs {
		file.Vectors = append(file.Vectors, buildVector(t, s))
	}

	blob, err := json.MarshalIndent(file, "", "  ")
	if err != nil {
		t.Fatalf("marshal vectors: %v", err)
	}
	if err := os.WriteFile(out, append(blob, '\n'), 0o644); err != nil {
		t.Fatalf("write %s: %v", out, err)
	}
	t.Logf("wrote %d vectors to %s (%d bytes)", len(file.Vectors), out, len(blob)+1)
}

func buildVector(t *testing.T, s spec) vector {
	t.Helper()

	cfg := &HandshakeConfig{
		Profile:            s.profile,
		ChainID:            s.chain,
		KEMScheme:          s.scheme,
		ForbidClassicalKEM: s.profile.IsStrict(),
	}

	initLocal := identityFromSeed(t, fill(s.name+"/init/identity", mldsa65.SeedSize))
	respLocal := identityFromSeed(t, fill(s.name+"/resp/identity", mldsa65.SeedSize))

	kemSeedSize := mlkem768.KeySeedSize
	if s.scheme == kem.KeyExchangeMLKEM1024 {
		kemSeedSize = mlkem1024.KeySeedSize
	}
	kemSeed := fill(s.name+"/init/kem", kemSeedSize)
	kemPub, kemSec := kemKeypair(t, s.scheme, kemSeed)

	init := &HandshakeInit{
		ProtocolVersion: pqProtocolVersionV1,
		Profile:         s.profile,
		ChainID:         s.chain,
		KEMScheme:       s.scheme,
		NodeID:          initLocal.NodeID,
		MLDSAPub:        packMLDSAPub(initLocal.Public),
		KEMPub:          kemPub,
	}
	initPrefix := init.transcriptPrefix(roleInitiator)
	initSig := make([]byte, IdentitySignatureSize)
	if err := mldsa65.SignTo(initLocal.Secret, initPrefix,
		[]byte("NODE_PQ_HANDSHAKE_V1/initiator"), false, initSig); err != nil {
		t.Fatalf("sign INIT: %v", err)
	}
	init.Sig = initSig

	legacySig := make([]byte, IdentitySignatureSize)
	if err := mldsa65.SignTo(initLocal.Secret, initPrefix,
		[]byte(legacySigContext), false, legacySig); err != nil {
		t.Fatalf("sign INIT under the legacy context: %v", err)
	}

	// The real responder has to accept this INIT. Its own reply carries a
	// random ciphertext and is discarded — what is being established is that
	// the initiator half above is what production Go admits.
	if _, _, err := RespondHandshake(cfg, respLocal, init); err != nil {
		t.Fatalf("%s: the production responder refused this INIT: %v", s.name, err)
	}

	encapSeed := fill(s.name+"/resp/encapsulate", 32)
	ct, _ := kemEncapsulate(t, s.scheme, kemPub, encapSeed)

	resp := &HandshakeResp{
		ProtocolVersion: pqProtocolVersionV1,
		Profile:         s.profile,
		ChainID:         s.chain,
		KEMScheme:       s.scheme,
		NodeID:          respLocal.NodeID,
		MLDSAPub:        packMLDSAPub(respLocal.Public),
		KEMCiphertext:   ct,
	}
	respPrefix := resp.transcriptPrefix(init, roleResponder)
	respSig := make([]byte, IdentitySignatureSize)
	if err := mldsa65.SignTo(respLocal.Secret, respPrefix,
		[]byte("NODE_PQ_HANDSHAKE_V1/responder"), false, respSig); err != nil {
		t.Fatalf("sign RESP: %v", err)
	}
	resp.Sig = respSig

	// The session key comes out of the production initiator path, not out of
	// a re-derivation written for this file.
	result, err := FinishInitiatorHandshake(cfg, initLocal, init, resp, kemSec)
	if err != nil {
		t.Fatalf("%s: the production initiator refused this RESP: %v", s.name, err)
	}
	if derived, err := verifyPQIdentityBinding(result); err != nil {
		t.Fatalf("%s: identity binding: %v", s.name, err)
	} else if derived != respLocal.NodeID {
		t.Fatalf("%s: identity binding produced %s, want %s", s.name, derived, respLocal.NodeID)
	}

	// The wire form has to parse back to what was written.
	if _, err := parsePQHandshakeInit(init.canonicalBytes(), s.scheme); err != nil {
		t.Fatalf("%s: INIT does not parse: %v", s.name, err)
	}
	if _, err := parsePQHandshakeResp(resp.canonicalBytes(), s.scheme); err != nil {
		t.Fatalf("%s: RESP does not parse: %v", s.name, err)
	}

	return vector{
		Name:            s.name,
		ProtocolVersion: pqProtocolVersionV1,
		Profile:         uint8(s.profile),
		ChainID:         hex.EncodeToString(s.chain[:]),
		KEMScheme:       uint8(s.scheme),
		Init: vectorSide{
			MLDSASeed: hex.EncodeToString(fill(s.name+"/init/identity", mldsa65.SeedSize)),
			MLDSAPub:  hex.EncodeToString(init.MLDSAPub),
			NodeID:    hex.EncodeToString(initLocal.NodeID[:]),
			KEMSeed:   hex.EncodeToString(kemSeed),
			KEMPub:    hex.EncodeToString(kemPub),
			Prefix:    hex.EncodeToString(initPrefix),
			Sig:       hex.EncodeToString(initSig),
			Canonical: hex.EncodeToString(init.canonicalBytes()),
		},
		Resp: vectorSide{
			MLDSASeed: hex.EncodeToString(fill(s.name+"/resp/identity", mldsa65.SeedSize)),
			MLDSAPub:  hex.EncodeToString(resp.MLDSAPub),
			NodeID:    hex.EncodeToString(respLocal.NodeID[:]),
			EncapSeed: hex.EncodeToString(encapSeed),
			KEMCt:     hex.EncodeToString(ct),
			Prefix:    hex.EncodeToString(respPrefix),
			Sig:       hex.EncodeToString(respSig),
			Canonical: hex.EncodeToString(resp.canonicalBytes()),
		},
		SharedSecret:     hex.EncodeToString(result.KEMSession.SharedSecret[:]),
		TranscriptHash:   hex.EncodeToString(result.KEMSession.TranscriptHash[:]),
		AEADKey:          hex.EncodeToString(result.AEADKey[:]),
		LegacyContextSig: hex.EncodeToString(legacySig),
	}
}
