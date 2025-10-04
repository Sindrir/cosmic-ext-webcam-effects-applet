{
  description = "COSMIC Webcam Effects Applet";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config.allowUnfree = true;
        };
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            pkg-config
            just
            llvmPackages.libclang
          ];

          buildInputs = with pkgs; [
            # D-Bus (for zbus)
            dbus

            # V4L2 / webcam
            v4l-utils
            libv4l
            linuxHeaders

            # Wayland / COSMIC dependencies
            libxkbcommon
            wayland
            libinput
            mesa
            vulkan-loader

            # Font / text rendering
            expat
            fontconfig
            freetype

            # SSL (needed by some deps)
            openssl

            # Desktop integration
            seatd
          ];

          LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
          BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.linuxHeaders}/include -isystem ${pkgs.glibc.dev}/include";

          shellHook = ''
            export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath [
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.vulkan-loader
              pkgs.cudaPackages.cudatoolkit
              pkgs.cudaPackages.cudnn
            ]}:/run/opengl-driver/lib:$LD_LIBRARY_PATH"
          '';
        };
      }
    );
}
