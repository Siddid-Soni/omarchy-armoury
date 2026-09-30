#!/bin/bash
# Runs uninstall.sh against stubbed sudo/systemctl/armoury; no real system changes.
set -uo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
fail=0

setup() {
  T=$(mktemp -d); mkdir -p "$T/bin" "$T/home" "$T/rootstate"
  for c in systemctl; do printf '#!/bin/bash\necho "%s $*" >> %s/log\n' "$c" "$T" > "$T/bin/$c"; done
  printf '#!/bin/bash\necho "armoury $*" >> %s/log\nexit 1\n' "$T" > "$T/bin/armoury"   # daemon down
  printf '#!/bin/bash\necho "sudo $*" >> %s/log\n[[ "$*" == *handback* && -n "${FAIL_HANDBACK:-}" ]] && exit 1\nexit 0\n' "$T" > "$T/bin/sudo"
  chmod +x "$T/bin"/*; : > "$T/log"
}

run() { env -i PATH="$T/bin:/usr/bin" HOME="$T/home" ARMOURY_ROOT_STATE="$T/rootstate" "$@" bash "$repo/uninstall.sh" >/dev/null 2>&1; }
check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; cat "$T/log"; fail=1; fi; }

# 1. Active, daemon down, root handback fails -> abort before deleting anything
setup; mkdir -p "$T/home/.local/state/omarchy-armoury"; touch "$T/home/.local/state/omarchy-armoury/active"
run FAIL_HANDBACK=1; rc=$?
check "aborts when handback fails" '[ $rc -ne 0 ] && ! grep -q "rm -rf" "$T/log"'

# 2. Active, daemon down, root handback works -> handback, flag cleared, G-Helper started, support restored before removal
setup; mkdir -p "$T/home/.local/state/omarchy-armoury"; touch "$T/home/.local/state/omarchy-armoury/active"
run; rc=$?
check "direct root handback when daemon is down" '[ $rc -eq 0 ] && grep -q "sudo /usr/local/lib/omarchy-armoury/armoury-root handback" "$T/log"'
check "flag cleared" '[ ! -e "$T/home/.local/state/omarchy-armoury/active" ]'
check "G-Helper started" 'grep -q "systemctl --user start app-ghelper@autostart.service" "$T/log"'
check "support restored before helper removed" '[ "$(grep -n "asusd-support-restore" "$T/log" | cut -d: -f1)" -lt "$(grep -n "rm -rf" "$T/log" | cut -d: -f1)" ]'

# 3. Leftover masked marker (e.g. half-failed takeover) also triggers root handback
setup; touch "$T/rootstate/asusd-was-masked"
run; rc=$?
check "marker triggers handback" 'grep -q "armoury-root handback" "$T/log"'

# 5. uninstall removes the SUPER+W bind file and only its loader line
setup; mkdir -p "$T/home/.config/hypr"; touch "$T/home/.config/hypr/omarchy-armoury.lua"
printf 'o.bind("SUPER + E", nil, "nautilus")\nif package.searchpath("hypr.omarchy-armoury", package.path) then require("hypr.omarchy-armoury") end -- omarchy-armoury\n' > "$T/home/.config/hypr/bindings.lua"
run; rc=$?
check "bind file removed" '[ ! -e "$T/home/.config/hypr/omarchy-armoury.lua" ]'
check "loader line removed, user binds kept" '! grep -q omarchy-armoury "$T/home/.config/hypr/bindings.lua" && grep -q nautilus "$T/home/.config/hypr/bindings.lua"'

# 4. uninstall removes the udev rule
setup
run; rc=$?
check "udev rule removed" 'grep -q "sudo rm -f .*/etc/udev/rules.d/70-omarchy-armoury.rules" "$T/log"'

# 5. the shipped rules grant only via uaccess (N-KEY, touchpad, i2c, uinput), never world-writable
R="$repo/packaging/udev/70-omarchy-armoury.rules"
check "udev rules grant only via uaccess (N-KEY, touchpad, i2c, uinput)" 'grep -q "ATTRS{name}==\"ASUSTek Computer Inc. N-KEY Device\"" "$R" && grep -q "ASUE\* Touchpad" "$R" && grep -q "SUBSYSTEM==\"i2c-dev\", TAG+=\"uaccess\"" "$R" && grep -q "KERNEL==\"uinput\"" "$R" && ! grep -q "MODE=" "$R"'

exit $fail
