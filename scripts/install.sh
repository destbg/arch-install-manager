#!/usr/bin/env bash
set -euo pipefail

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$1"; }
ok()   { printf '\033[1;32m   \xe2\x9c\x93 %s\033[0m\n' "$1"; }
die()  { printf '\033[1;31m   \xe2\x9c\x97 %s\033[0m\n' "$1"; exit 1; }

proj="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$proj" || die "cannot enter project directory"

if [[ "${1:-}" == "-u" || "${1:-}" == "--uninstall" ]]; then
    step "Removing daim from the system"
    sudo systemctl disable --now daim-check.timer 2>/dev/null || true
    systemctl --user disable --now daim-tray.service 2>/dev/null || true
    sudo rm -f /usr/bin/daim /usr/bin/daim-gui /usr/bin/daim-helper /usr/bin/daim-tray /usr/bin/daim-check
    sudo rm -f /usr/share/polkit-1/actions/com.destbg.arch-install-manager.policy
    sudo rm -f /usr/share/polkit-1/rules.d/49-daim-check.rules
    sudo rm -f /usr/lib/sysusers.d/daim-build.conf
    sudo rm -rf /var/lib/daim
    sudo rm -f /usr/share/applications/arch-install-manager.desktop
    sudo rm -f /usr/lib/systemd/system/daim-check.service /usr/lib/systemd/system/daim-check.timer
    sudo rm -f /usr/lib/systemd/user/daim-tray.service
    sudo rm -f /usr/share/icons/hicolor/*/apps/arch-install-manager.png
    sudo rm -f /usr/share/icons/hicolor/symbolic/apps/arch-install-manager-*-symbolic.svg
    sudo systemctl daemon-reload
    ok "removed"
    exit 0
fi

step "Building daim (release)"
cargo build --release || die "build failed"
ok "built daim, daim-gui, daim-helper, daim-tray, daim-check"

step "Installing binaries, polkit policy, desktop entry and icons (sudo)"
for b in daim daim-gui daim-helper daim-tray daim-check; do
    sudo install -Dm755 "target/release/$b" "/usr/bin/$b" || die "failed to install $b"
done
sudo install -Dm644 com.destbg.arch-install-manager.policy \
    /usr/share/polkit-1/actions/com.destbg.arch-install-manager.policy
sudo install -Dm644 res/polkit/49-daim-check.rules \
    /usr/share/polkit-1/rules.d/49-daim-check.rules
sudo install -Dm644 res/sysusers/daim-build.conf \
    /usr/lib/sysusers.d/daim-build.conf
sudo systemd-sysusers /usr/lib/sysusers.d/daim-build.conf || die "failed to create the daim-build user"
sudo install -Dm644 arch-install-manager.desktop \
    /usr/share/applications/arch-install-manager.desktop
for size in 48x48 256x256 512x512; do
    if [[ -f "icons/$size/apps/arch-install-manager.png" ]]; then
        sudo install -Dm644 "icons/$size/apps/arch-install-manager.png" \
            "/usr/share/icons/hicolor/$size/apps/arch-install-manager.png"
    fi
done
for sym in arch-install-manager-arch-symbolic arch-install-manager-flatpak-symbolic; do
    if [[ -f "icons/symbolic/apps/$sym.svg" ]]; then
        sudo install -Dm644 "icons/symbolic/apps/$sym.svg" \
            "/usr/share/icons/hicolor/symbolic/apps/$sym.svg"
    fi
done
if command -v gtk-update-icon-cache >/dev/null; then
    sudo gtk-update-icon-cache -qtf /usr/share/icons/hicolor 2>/dev/null || true
fi
if command -v update-desktop-database >/dev/null; then
    sudo update-desktop-database /usr/share/applications 2>/dev/null || true
fi
ok "installed (coexists with arch-update-manager)"

step "Installing systemd units"
sudo install -Dm644 res/systemd/daim-check.service /usr/lib/systemd/system/daim-check.service
sudo install -Dm644 res/systemd/daim-check.timer /usr/lib/systemd/system/daim-check.timer
sudo install -Dm644 res/systemd/daim-tray.service /usr/lib/systemd/user/daim-tray.service
sudo systemctl daemon-reload
systemctl --user daemon-reload
sudo systemctl enable --now daim-check.timer 2>/dev/null && ok "root check timer enabled" || true
systemctl --user enable daim-tray.service 2>/dev/null || true
systemctl --user restart daim-tray.service 2>/dev/null && ok "user tray service (re)started with the new binary" || true

step "Launching daim-gui"
daim-gui
