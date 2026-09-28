#!/bin/bash
# Remove omarchy-armoury and hand hardware back to G-Helper.
set -euo pipefail

LIB=/usr/local/lib/omarchy-armoury
UNIT_DIR=~/.config/systemd/user

armoury handback 2>/dev/null || true
systemctl --user disable --now armouryd.service 2>/dev/null || true
rm -f ~/.local/bin/armouryd ~/.local/bin/armoury "$UNIT_DIR/armouryd.service"
rm -f "$UNIT_DIR/app-ghelper@autostart.service.d/omarchy-armoury.conf"
rmdir "$UNIT_DIR/app-ghelper@autostart.service.d" 2>/dev/null || true
systemctl --user daemon-reload

if [ -f /var/lib/omarchy-armoury/aura_support.ron.orig ]; then
  sudo install -m644 /var/lib/omarchy-armoury/aura_support.ron.orig /usr/share/asusd/aura_support.ron
fi
sudo rm -rf "$LIB" /var/lib/omarchy-armoury
sudo rm -f /usr/share/polkit-1/actions/org.omarchy.armoury.policy \
  /etc/polkit-1/rules.d/50-omarchy-armoury.rules \
  /etc/pacman.d/hooks/omarchy-armoury-asusd.hook

echo "Removed omarchy-armoury."
