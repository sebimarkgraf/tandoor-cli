{
  description = "Tandoor Recipes SDK and CLI";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { nixpkgs, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      perSystem = system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          buildPackage = rustPkgs: rustPkgs.rustPlatform.buildRustPackage {
            pname = manifest.package.name;
            version = manifest.package.version;
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions [
                ./Cargo.toml ./Cargo.lock ./LICENSE.md ./README.md ./src ./tests
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            __darwinAllowLocalNetworking = true;
            nativeBuildInputs = [ pkgs.rustfmt pkgs.clippy ];
            preCheck = ''
              cargo fmt --all --check
              cargo clippy --offline --locked --release --target ${rustPkgs.stdenv.hostPlatform.rust.rustcTarget} --all-targets -- -D warnings
            '';
            meta.mainProgram = "tandoor";
          };
          package = buildPackage pkgs;
          release = pkgs.runCommand "${manifest.package.name}-${manifest.package.version}-${system}" {
            nativeBuildInputs = [ pkgs.gnutar pkgs.gzip ];
          } ''
            mkdir -p "$out"
            tar -czf "$out/tandoor-${manifest.package.version}-${system}.tar.gz" \
              -C ${buildPackage pkgs.pkgsStatic}/bin tandoor \
              -C ${package.src} LICENSE.md README.md
          '';
          publish = pkgs.writeShellApplication {
            name = "publish";
            runtimeInputs = [ pkgs.cargo pkgs.rustc pkgs.stdenv.cc pkgs.git pkgs.gh ];
            text = ''
              tag="''${1:?Usage: publish v${manifest.package.version} [--dry-run]}"
              shift
              if [[ "$tag" != "v${manifest.package.version}" ]]; then
                echo "Tag must match Cargo.toml: v${manifest.package.version}" >&2
                exit 1
              fi
              if [[ $# -gt 1 || ( $# -eq 1 && "$1" != --dry-run ) ]]; then
                echo "Only --dry-run is supported after the tag" >&2
                exit 1
              fi
              export SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt
              cargo publish --locked "$@"
              if [[ "''${1:-}" == --dry-run ]]; then
                exit 0
              fi
              gh release create "$tag" ${release}/*.tar.gz \
                --verify-tag --title "$tag" --generate-notes
            '';
          };
        in {
          packages = { default = package; } // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            inherit release;
          };
          checks = { default = package; } // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            inherit release publish;
          };
          apps = pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
            publish = {
              type = "app";
              meta.description = "Publish to crates.io and GitHub Releases";
              program = "${publish}/bin/publish";
            };
          };
          devShells.default = pkgs.mkShell {
            inputsFrom = [ package ];
            packages = [ pkgs.rust-analyzer ];
          };
        };
    in {
      packages = forAllSystems (system: (perSystem system).packages);
      checks = forAllSystems (system: (perSystem system).checks);
      apps = forAllSystems (system: (perSystem system).apps);
      devShells = forAllSystems (system: (perSystem system).devShells);
    };
}
