{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      fenix,
    }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      devShells.${system}.default = pkgs.mkShell rec {
        buildInputs = with pkgs; [
          fenix.packages.${system}.default.rustfmt
          clippy
          mdbook

          libxkbcommon.dev
          fontconfig.dev
          wayland
          vulkan-loader
          pkg-config
          dbus.dev
          pipewire.dev
        ];
        LIBCLANG_PATH = nixpkgs.lib.makeLibraryPath (with pkgs; [ libclang.lib ]);
        LD_LIBRARY_PATH = nixpkgs.lib.makeLibraryPath buildInputs;
      };
      formatter.${system} = pkgs.nixfmt-tree;

      packages.${system}.eucalyptus-twig = pkgs.callPackage (
        {
          lib,
          rustPlatform,
          pkg-config,
          dbus,
          fontconfig,
          freetype,
          libxkbcommon,
          pipewire,
          vulkan-loader,
          wayland,
        }:

        rustPlatform.buildRustPackage (finalAttrs: {
          pname = "eucalyptus-twig";
          version = "git-${self.shortRev or "dirty"}";

          src = ./.;

          cargoHash = "sha256-cPZUEHXjWllqLSxReufjw2PGy1vPPzPMMWQEOocqAR4=";

          nativeBuildInputs = [
            pkg-config
            rustPlatform.bindgenHook
          ];

          buildInputs = [
            dbus
            fontconfig
            freetype
            libxkbcommon
            pipewire
            vulkan-loader
            wayland
          ];

          RUSTFLAGS = "-C link-arg=-Wl,--push-state,--no-as-needed,-lvulkan,--pop-state";

          meta = {
            description = "";
            homepage = "https://github.com/Shiphan/eucalyptus-twig";
            license = lib.licenses.gpl2Plus;
            maintainers = with lib.maintainers; [ shiphan ];
            mainProgram = "eucalyptus-twig";
          };
        })
      ) { };
    };
}
