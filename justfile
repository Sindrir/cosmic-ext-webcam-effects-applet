name := 'cosmic-ext-webcam-effects-applet'
appid := 'dev.sindrir.CosmicExtWebcamEffectsApplet'

rootdir := ''
prefix := '/usr'

# Installation paths
base-dir := absolute_path(clean(rootdir / prefix))
cargo-target-dir := env('CARGO_TARGET_DIR', 'target')
appdata-dst := base-dir / 'share' / 'appdata' / appid + '.metainfo.xml'
bin-dst := base-dir / 'bin' / name
daemon-dst := base-dir / 'bin' / name + '-daemon'
desktop-dst := base-dir / 'share' / 'applications' / appid + '.desktop'
icon-dst := base-dir / 'share' / 'icons' / 'hicolor' / 'scalable' / 'apps' / appid + '.svg'
service-dst := base-dir / 'share' / 'dbus-1' / 'services' / appid + '.service'

# Default recipe which runs `just build-release`
default: build-release

# Runs `cargo clean`
clean:
    cargo clean

# Removes vendored dependencies
clean-vendor:
    rm -rf .cargo vendor vendor.tar

# `cargo clean` and removes vendored dependencies
clean-dist: clean clean-vendor

# Compiles with debug profile
build-debug *args:
    cargo build {{args}}

# Compiles with release profile
build-release *args: (build-debug '--release' args)

# Compiles release profile with vendored dependencies
build-vendored *args: vendor-extract (build-release '--frozen --offline' args)

# Runs a clippy check
check *args:
    cargo clippy --all-features {{args}} -- -W clippy::pedantic

# Runs a clippy check with JSON message format
check-json: (check '--message-format=json')

# Run the application for testing purposes
run *args:
    env RUST_BACKTRACE=full cargo run --release {{args}}

# Installs files
install:
    install -Dm0755 {{ cargo-target-dir / 'release' / name }} {{bin-dst}}
    install -Dm0755 {{ cargo-target-dir / 'release' / name + '-daemon' }} {{daemon-dst}}
    install -Dm0644 resources/app.desktop {{desktop-dst}}
    install -Dm0644 resources/app.metainfo.xml {{appdata-dst}}
    install -Dm0644 resources/icon.svg {{icon-dst}}
    install -Dm0644 resources/{{ appid + '.service' }} {{service-dst}}
    sed -i "s|@daemon-path@|{{daemon-dst}}|g" {{service-dst}}

# Build debug binaries and hot-reload via user-local .desktop files (no sudo, no PATH changes)
dev:
    #!/usr/bin/env bash
    set -euo pipefail
    touch src/i18n.rs
    just build-debug
    mkdir -p target/debug/wrappers ~/.local/share/applications ~/.local/share/dbus-1/services
    # Write wrapper scripts that bake in the nix dev-shell library paths (mimics wrapProgram)
    for bin in cosmic-ext-webcam-effects-applet cosmic-ext-webcam-effects-applet-daemon; do
        printf '#!/bin/sh\nexport LD_LIBRARY_PATH="%s"\nexport PATH="%s"\nexec "{{ justfile_directory() }}/target/debug/%s" "$@"\n' \
            "$LD_LIBRARY_PATH" "$PATH" "$bin" > "target/debug/wrappers/$bin"
        chmod +x "target/debug/wrappers/$bin"
    done
    sed 's|Exec=cosmic-ext-webcam-effects-applet %F|Exec={{ justfile_directory() }}/target/debug/wrappers/cosmic-ext-webcam-effects-applet %F|' resources/app.desktop > ~/.local/share/applications/{{ appid }}.desktop
    sed 's|@daemon-path@|{{ justfile_directory() }}/target/debug/wrappers/cosmic-ext-webcam-effects-applet-daemon|g' resources/{{ appid + '.service' }} > ~/.local/share/dbus-1/services/{{ appid }}.service
    pkill -f '[c]osmic-ext-webcam-effects-applet-daemon' || true
    pkill -f '[c]osmic-panel'

# Uninstalls installed files
uninstall:
    rm {{bin-dst}} {{daemon-dst}} {{desktop-dst}} {{icon-dst}} {{service-dst}}

# Vendor dependencies locally
vendor:
    mkdir -p .cargo
    cargo vendor --sync Cargo.toml | head -n -1 > .cargo/config.toml
    echo 'directory = "vendor"' >> .cargo/config.toml
    echo >> .cargo/config.toml
    rm -rf .cargo vendor

# Extracts vendored dependencies
vendor-extract:
    rm -rf vendor
    tar pxf vendor.tar

# Bump cargo version, create git commit, and create tag
tag version:
    find -type f -name Cargo.toml -exec sed -i '0,/^version/s/^version.*/version = "{{version}}"/' '{}' \; -exec git add '{}' \;
    cargo check
    cargo clean
    git add Cargo.lock
    git commit -m 'release: {{version}}'
    git tag -a {{version}} -m ''
