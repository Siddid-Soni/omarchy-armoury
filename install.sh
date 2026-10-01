#!/bin/bash
# Install (or update) omarchy-armoury's daemon, CLI and root helper, then let armouryd
# take control. Uses the prebuilt release matching this plugin's version when there is
# one, else builds with cargo.
#   --build    always build from source
set -euo pipefail
cd "$(dirname "$0")"

LIB=/usr/local/lib/omarchy-armoury
REPO=https://github.com/Siddid-Soni/omarchy-armoury
BUILD=0
for arg in "$@"; do
  case $arg in
    --build) BUILD=1 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
VERSION=$(sed -n 's/^ *"version": *"\([^"]*\)".*/\1/p' manifest.json | head -1)

# yes/no question with a default; without a terminal the answer is "no" (nothing
# optional changes without the user saying so)
ask() {
  local prompt=$1 default=$2 reply
  [ -t 0 ] || return 1
  read -r -p "$prompt " reply
  reply=${reply:-$default}
  [[ $reply =~ ^[Yy] ]]
}

cat <<'INFO'
omarchy-armoury installs:
  - armouryd (user service) and the armoury CLI in ~/.local/bin
  - a root helper for a fixed set of hardware commands, run via pkexec without a password
    (polkit rule for your user)
  - udev rules giving your seat access to the ASUS keyboard, touchpad, i2c and uinput
  - a pacman hook and a fix to asusd's model file (all lighting zones on the G533Z)
It downloads the prebuilt binaries for this version from GitHub releases (checked against
the checksum in this plugin's source), or builds them with cargo, and asks for sudo once. uninstall.sh reverses all of it.
INFO
if [ -t 0 ] && ! ask "Continue? [Y/n]" y; then echo "Nothing installed."; exit 0; fi
UNIT_DIR=~/.config/systemd/user

# Prebuilt binaries for this exact version. The expected sha256 comes from
# packaging/release.sha256 in this checkout (the reviewed source), never from the release
# itself: armoury-root runs as root, so replacing release assets must not be enough to change it.
fetch_release() {
  local dir=$1 name="armoury-$VERSION-$(uname -m)-linux.tar.gz" want
  want=$(awk -v n="$name" '$2 == n && $1 ~ /^[0-9a-f]{64}$/ { print $1 }' packaging/release.sha256)
  [ -n "$want" ] || return 1
  command -v curl >/dev/null || return 1
  curl -fsSL "$REPO/releases/download/v$VERSION/$name" -o "$dir/$name" 2>/dev/null || return 1
  if [ "$(sha256sum "$dir/$name" | cut -d' ' -f1)" != "$want" ]; then
    echo "The downloaded $name does not match the checksum in packaging/release.sha256; not using it." >&2
    return 1
  fi
  tar -xzf "$dir/$name" -C "$dir"
}

if [ "$BUILD" = 0 ] && fetch_release "$TMP"; then
  echo "Using the prebuilt release v$VERSION."
  BIN=$TMP
else
  if ! command -v cargo >/dev/null; then
    echo "No verified prebuilt release for v$VERSION is available, and cargo isn't installed." >&2
    if ask "Install Rust (sudo pacman -S --needed rust) and build from source? [Y/n]" y; then
      sudo pacman -S --needed --noconfirm rust
    else
      echo "Install Rust (sudo pacman -S rust) and run this again." >&2
      exit 1
    fi
  fi
  [ "$BUILD" = 1 ] || echo "No prebuilt release for v$VERSION; building from source."
  # Build outside the source tree: when installed with `omarchy plugin add`, this folder is
  # the live plugin, and a Rust target directory (several GB) doesn't belong in it.
  export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/omarchy-armoury/target}"
  cargo build --release --workspace
  BIN="$CARGO_TARGET_DIR/release"
fi

install -Dm755 "$BIN"/armouryd ~/.local/bin/armouryd
install -Dm755 "$BIN"/armoury ~/.local/bin/armoury
install -Dm644 packaging/systemd/armouryd.service "$UNIT_DIR/armouryd.service"
install -Dm644 packaging/systemd/ghelper-dropin.conf "$UNIT_DIR/app-ghelper@autostart.service.d/omarchy-armoury.conf"

# SUPER+W also closes the Armoury window (see the file); loaded from the user's bindings.lua
install -Dm644 packaging/hypr/omarchy-armoury.lua ~/.config/hypr/omarchy-armoury.lua
HYPR_LINE='if package.searchpath("hypr.omarchy-armoury", package.path) then require("hypr.omarchy-armoury") end -- omarchy-armoury'
if grep -qxF "$HYPR_LINE" ~/.config/hypr/bindings.lua 2>/dev/null; then
  :
elif ask "Add one line to ~/.config/hypr/bindings.lua so SUPER+W closes the Armoury window? [Y/n]" y; then
  printf '\n%s\n' "$HYPR_LINE" >> ~/.config/hypr/bindings.lua
else
  echo "Skipped. To add it later, append this line to ~/.config/hypr/bindings.lua:"
  echo "  $HYPR_LINE"
fi

sudo install -Dm755 "$BIN"/armoury-root "$LIB/armoury-root"
sudo install -Dm644 packaging/polkit/org.omarchy.armoury.policy /usr/share/polkit-1/actions/org.omarchy.armoury.policy
sed "s/@USER@/$USER/" packaging/polkit/50-omarchy-armoury.rules.in |
  sudo install -Dm644 /dev/stdin /etc/polkit-1/rules.d/50-omarchy-armoury.rules
sudo install -Dm644 packaging/pacman/omarchy-armoury-asusd.hook /etc/pacman.d/hooks/omarchy-armoury-asusd.hook
sudo install -Dm644 packaging/udev/70-omarchy-armoury.rules /etc/udev/rules.d/70-omarchy-armoury.rules
sudo install -Dm644 packaging/modules-load/omarchy-armoury.conf /etc/modules-load.d/omarchy-armoury.conf
sudo modprobe i2c-dev
sudo udevadm control --reload
# every device class the rules cover: N-KEY + touchpad (input), /dev/uinput (misc), /dev/i2c-* (i2c-dev), N-KEY Aura (hidraw)
sudo udevadm trigger --subsystem-match=input --subsystem-match=misc --subsystem-match=i2c-dev --subsystem-match=hidraw --action=change
sudo "$LIB/armoury-root" asusd-support-fix

systemctl --user daemon-reload
systemctl --user enable --now armouryd.service

# armouryd takes control: starts asusd, and stops G-Helper if it is installed (asked first)
for _ in $(seq 1 20); do ~/.local/bin/armoury status >/dev/null 2>&1 && break; sleep 0.5; done
if [ -e /opt/ghelper/ghelper ] || [ -e ~/.config/autostart/ghelper.desktop ] || pgrep -x ghelper >/dev/null; then
  if ! ask "G-Helper is installed. Stop it and block its autostart so Armoury can take control? [Y/n]" y; then
    echo "Installed, but G-Helper keeps control. When you're ready: armoury takeover"
    exit 0
  fi
fi
~/.local/bin/armoury takeover

echo "Installed. Armoury is in control; open it from the bar or with the ROG key."
