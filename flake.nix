{
  description = "NBping - A Ping tool developed in Rust with concurrent support, visual charts, and real-time data updates";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "nbping";
            version = "0.6.1";

            src = ./.;

            cargoHash = "sha256-6+drbq9dQ5/Atzoz9VPS4BoYEPeM5OqPXUuM1AXP72g=";

            meta = with pkgs.lib; {
              description = "NBping is a Ping tool developed in Rust. It supports concurrent Ping for multiple addresses, visual chart display, real-time data updates, and other features";
              homepage = "https://github.com/hanshuaikang/Nping";
              license = licenses.mit;
              mainProgram = "nbping";
            };
          };
        }
      );

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/nbping";
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              cargo
              rustc
              rust-analyzer
              rustfmt
              clippy
            ];

            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          };
        }
      );
    };
}
