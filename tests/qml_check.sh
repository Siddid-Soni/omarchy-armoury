#!/bin/bash
# Compiles every plugin QML file in a throwaway, window-less Quickshell instance
# (the only engine that can load Quickshell + Omarchy's qs.* modules) and
# prints OK or the compile error per file. Exit 1 on any error.
set -uo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
shell=${OMARCHY_PATH:-/usr/share/omarchy}/shell
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
ln -s "$shell/Ui" "$T/Ui"
ln -s "$shell/Commons" "$T/Commons"
files=$(cd "$repo/plugin" && ls *.qml | sed 's/\.qml$//' | tr '\n' ' ')
cat > "$T/shell.qml" <<QML
import QtQuick
import Quickshell
Scope {
  Component.onCompleted: {
    var files = "$files".trim().split(" ")
    for (var i = 0; i < files.length; i++) {
      var c = Qt.createComponent("file://$repo/plugin/" + files[i] + ".qml")
      console.log("CHECK " + files[i] + ": " + (c.status === Component.Ready ? "OK" : c.errorString().replace(/\n/g, " | ")))
    }
    console.log("CHECK-DONE")
  }
}
QML
quickshell -p "$T" >/dev/null 2>&1 &
pid=$!
log=""
for _ in $(seq 40); do
  sleep 0.5
  log=$(grep -rh 'CHECK' /run/user/$UID/quickshell/by-pid/$pid/log.log 2>/dev/null || true)
  [[ $log == *CHECK-DONE* ]] && break
done
kill $pid 2>/dev/null
printf '%s\n' "$log" | sed -n 's/.*CHECK \(.*\)/\1/p' | grep -v '^-DONE'
! printf '%s\n' "$log" | grep 'CHECK' | grep -v 'CHECK-DONE' | grep -vq ': OK$'
