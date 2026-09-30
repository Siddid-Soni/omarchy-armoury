#!/bin/bash
# Build and install omarchy-armoury. armouryd starts in observe mode (G-Helper keeps working).
set -euo pipefail
cd "$(dirname "$0")"

LIB=/usr/local/lib/omarchy-armoury
UNIT_DIR=~/.config/systemd/user

cargo build --release --workspace

install -Dm755 target/release/armouryd ~/.local/bin/armouryd
install -Dm755 target/release/armoury ~/.local/bin/armoury
install -Dm644 packaging/systemd/armouryd.service "$UNIT_DIR/armouryd.service"
install -Dm644 packaging/systemd/ghelper-dropin.conf "$UNIT_DIR/app-ghelper@autostart.service.d/omarchy-armoury.conf"

# SUPER+W also closes the Armoury window (see the file); loaded from the user's bindings.lua
install -Dm644 packaging/hypr/omarchy-armoury.lua ~/.config/hypr/omarchy-armoury.lua
HYPR_LINE='if package.searchpath("hypr.omarchy-armoury", package.path) then require("hypr.omarchy-armoury") end -- omarchy-armoury'
grep -qxF "$HYPR_LINE" ~/.config/hypr/bindings.lua 2>/dev/null || printf '\n%s\n' "$HYPR_LINE" >> ~/.config/hypr/bindings.lua

sudo install -Dm755 target/release/armoury-root "$LIB/armoury-root"
sudo install -Dm644 packaging/polkit/org.omarchy.armoury.policy /usr/share/polkit-1/actions/org.omarchy.armoury.policy
sed "s/@USER@/$USER/" packaging/polkit/50-omarchy-armoury.rules.in |
  sudo install -Dm644 /dev/stdin /etc/polkit-1/rules.d/50-omarchy-armoury.rules
sudo install -Dm644 packaging/pacman/omarchy-armoury-asusd.hook /etc/pacman.d/hooks/omarchy-armoury-asusd.hook
sudo install -Dm644 packaging/udev/70-omarchy-armoury.rules /etc/udev/rules.d/70-omarchy-armoury.rules
sudo install -Dm644 packaging/modules-load/omarchy-armoury.conf /etc/modules-load.d/omarchy-armoury.conf
sudo modprobe i2c-dev
sudo udevadm control --reload
# every device class the rules cover: N-KEY + touchpad (input), /dev/uinput (misc), /dev/i2c-* (i2c-dev)
sudo udevadm trigger --subsystem-match=input --subsystem-match=misc --subsystem-match=i2c-dev --action=change
sudo "$LIB/armoury-root" asusd-support-fix

systemctl --user daemon-reload
systemctl --user enable --now armouryd.service

echo "Installed. armouryd is observing; run 'armoury takeover' to let it control the hardware."
