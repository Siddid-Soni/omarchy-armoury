#!/bin/bash
# Remove omarchy-armoury and hand hardware back to G-Helper.
set -euo pipefail

LIB=/usr/local/lib/omarchy-armoury
ROOT_STATE=${ARMOURY_ROOT_STATE:-/var/lib/omarchy-armoury}
FLAG=~/.local/state/omarchy-armoury/active
UNIT_DIR=~/.config/systemd/user
GHELPER_UNIT=app-ghelper@autostart.service

armoury handback 2>/dev/null || true

# Daemon down or its handback failed: finish the handback directly, or stop here
# while armoury-root still exists to restore asusd's original masked state.
if [ -e "$FLAG" ] || [ -e "$ROOT_STATE/asusd-was-masked" ]; then
  if ! sudo "$LIB/armoury-root" handback; then
    echo "Handback failed; nothing removed. Fix asusd, then rerun ./uninstall.sh." >&2
    exit 1
  fi
  rm -f "$FLAG"
  systemctl --user start "$GHELPER_UNIT" || true
fi

systemctl --user disable --now armouryd.service 2>/dev/null || true
rm -f ~/.local/bin/armouryd ~/.local/bin/armoury "$UNIT_DIR/armouryd.service"
rm -f "$UNIT_DIR/$GHELPER_UNIT.d/omarchy-armoury.conf"
rmdir "$UNIT_DIR/$GHELPER_UNIT.d" 2>/dev/null || true
systemctl --user daemon-reload

sudo "$LIB/armoury-root" asusd-support-restore || echo "Could not restore aura_support.ron; run: sudo pacman -S asusctl" >&2
sudo rm -rf "$LIB" "$ROOT_STATE"
sudo rm -f /usr/share/polkit-1/actions/org.omarchy.armoury.policy \
  /etc/polkit-1/rules.d/50-omarchy-armoury.rules \
  /etc/pacman.d/hooks/omarchy-armoury-asusd.hook

echo "Removed omarchy-armoury."
