{
  description = "the Rust EVM, and the corpus it is measured against";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Go is the oracle and Rust conforms, so the vectors are an INPUT here, not
    # a checkout the runner is assumed to have beside this one. Without them
    # tests/corpus panics and the whole Go-Rust differential — five tests,
    # including "rust encodes the genesis header to the bytes Go encodes" —
    # fails for the only reason that never means the code is wrong.
    conformance = { url = "git+ssh://git@github.com/luxfi/conformance"; flake = false; };
  };

  outputs = { self, nixpkgs, conformance }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      eachSystem = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs { inherit system; }));
    in {
      devShells = eachSystem (pkgs: {
        default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.pkg-config ];
          # The env var the corpus loader already reads. Its fallback is a
          # sibling path, which is true on one laptop and false everywhere else.
          LUX_VECTORS = "${conformance}/vectors";
        };
      });
    };
}
