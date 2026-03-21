{
  description = "Webcam background effects for the COSMIC™ desktop";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs {
        inherit system;
        config.allowUnfree = true;
      };

      runtimeLibs = with pkgs; [
        wayland
        libxkbcommon
        vulkan-loader
        onnxruntime
      ];
    in
    {
      devShells.${system}.default = pkgs.mkShell {
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

      # nix run .#fix-hashes -- iteratively builds and patches outputHash mismatches
      apps.${system}.fix-hashes = {
        type = "app";
        program = toString (pkgs.writeShellScript "fix-hashes" ''
          export PATH="${pkgs.lib.makeBinPath [ pkgs.nix pkgs.coreutils pkgs.gnugrep pkgs.gnused ]}:$PATH"
          set -euo pipefail
          flake="''${1:-./flake.nix}"
          max=30
          for (( i=1; i<=max; i++ )); do
            echo "=== Attempt $i ==="
            output=$(nix build --impure 2>&1) || true
            mismatches=$(echo "$output" | grep -A1 'specified:' | paste - - | \
              sed -n 's/.*specified: \(sha256-[^[:space:]]*\).*got: *\(sha256-[^[:space:]]*\).*/\1 \2/p') || true
            if [ -z "$mismatches" ]; then
              if echo "$output" | grep -q 'error:'; then
                echo "Build failed with non-hash error:"
                echo "$output" | grep 'error:' | head -5
                exit 1
              fi
              echo "Build succeeded!"
              exit 0
            fi
            while IFS=' ' read -r old new; do
              echo "  $old -> $new"
              sed -i "s|$old|$new|g" "$flake"
            done <<< "$mismatches"
          done
          echo "Exceeded $max iterations" >&2; exit 1
        '');
      };

      homeManagerModules.default = { pkgs, ... }:
        let
          pkg = self.packages.${system}.default;
        in
        {
          home.packages = [ pkg ];
          xdg.dataFile."dbus-1/services/dev.sindrir.CosmicExtWebcamEffectsApplet.service".source =
            "${pkg}/share/dbus-1/services/dev.sindrir.CosmicExtWebcamEffectsApplet.service";
        };

      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "cosmic-ext-webcam-effects-applet";
        version = "0.1.0";
        src = ./.;
        cargoLock = {
          lockFile = ./Cargo.lock;
          # libcosmic embeds iced as a git submodule. allowBuiltinFetchGit uses
          # builtins.fetchGit { submodules = true } at eval time — requires --impure.
          allowBuiltinFetchGit = true;
          outputHashes = {
            "accesskit-0.22.0" = "sha256-pP9CyiV1zIONQ7vbl5MkMtilemSPrHaZ0c/SyR+lb0k=";
            "atomicwrites-0.4.2" = "sha256-QZSuGPrJXh+svMeFWqAXoqZQxLq/WfIiamqvjJNVhxA=";
            "clipboard_macos-0.1.0" = "sha256-WO3JFbE+6ESRAfkxrnEFeZyGuhUHLOKOVHcGQyHwoK0=";
            "cosmic-client-toolkit-0.2.0" = "sha256-ymn+BUTTzyHquPn4hvuoA3y1owFj8LVrmsPu2cdkFQ8=";
            "cosmic-freedesktop-icons-0.4.0" = "sha256-D4bWHQ4Dp8UGiZjc6geh2c2SGYhB7mX13THpCUie1c4=";
            "cosmic-panel-config-0.1.0" = "sha256-DCeM9dpYpqLGdVW0MNQ4N9uWo97VpV7lSBhWJ0ufCC4=";
            "cosmic-settings-daemon-0.1.0" = "sha256-YRCNF2NQia6a9QlUIoEw0v2bMiZq94eViLsx+8NoghI=";
            "cosmic-text-0.18.2" = "sha256-fBtTOzS6DHkjoDI6dtUCY0/pk5/pwxvXErKNdnrlppk=";
            "cryoglyph-0.1.0" = "sha256-sSfgXlWgrM4wdczdquqzc/uuUmHL/GuK+Xvn0XNO+UQ=";
            "dpi-0.1.2" = "sha256-sOf5RuK4fs9FspaUnnviEx2SHNB+6oImg4Ox/owUGzo=";
            "smithay-clipboard-0.8.0" = "sha256-GojAFRbhJcP0Rpr+v9WOivgW9x38PZdeBWTbMhkDB3A=";
            "softbuffer-0.4.1" = "sha256-/ocK79Lr5ywP/bb5mrcm7eTzeBbwpOazojvFUsAjMKM=";
          };
        };

        nativeBuildInputs = with pkgs; [
          pkg-config
          just
          makeWrapper
          llvmPackages.libclang
        ];

        buildInputs = with pkgs; [
          wayland
          libxkbcommon
          vulkan-loader
          dbus
          v4l-utils
          libv4l
          linuxHeaders
          expat
          fontconfig
          freetype
          openssl
          seatd
        ];

        LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
        BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.linuxHeaders}/include -isystem ${pkgs.glibc.dev}/include";
        # ort-sys normally downloads prebuilt ONNX Runtime binaries, but the Nix
        # sandbox blocks network. Point it at nixpkgs onnxruntime and force dynamic
        # linking (the default path tries static linking which fails with .so-only).
        ORT_LIB_LOCATION = "${pkgs.onnxruntime}/lib";
        ORT_PREFER_DYNAMIC_LINK = "1";

        justFlags = "prefix=${placeholder "out"}";

        postInstall = ''
          install -Dm0644 resources/app.desktop $out/share/applications/dev.sindrir.CosmicExtWebcamEffectsApplet.desktop
          install -Dm0644 resources/app.metainfo.xml $out/share/appdata/dev.sindrir.CosmicExtWebcamEffectsApplet.metainfo.xml
          install -Dm0644 resources/icon.svg $out/share/icons/hicolor/scalable/apps/dev.sindrir.CosmicExtWebcamEffectsApplet.svg
          install -Dm0644 resources/dev.sindrir.CosmicExtWebcamEffectsApplet.service $out/share/dbus-1/services/dev.sindrir.CosmicExtWebcamEffectsApplet.service
          substituteInPlace $out/share/dbus-1/services/dev.sindrir.CosmicExtWebcamEffectsApplet.service \
            --replace-fail '@daemon-path@' "$out/bin/cosmic-ext-webcam-effects-applet-daemon"
        '';

        postFixup = ''
          patchelf --add-rpath ${pkgs.lib.makeLibraryPath runtimeLibs} $out/bin/cosmic-ext-webcam-effects-applet
          patchelf --add-rpath ${pkgs.lib.makeLibraryPath runtimeLibs} $out/bin/cosmic-ext-webcam-effects-applet-daemon
          wrapProgram $out/bin/cosmic-ext-webcam-effects-applet-daemon \
            --prefix LD_LIBRARY_PATH : /run/opengl-driver/lib
        '';
      };
    };
}
