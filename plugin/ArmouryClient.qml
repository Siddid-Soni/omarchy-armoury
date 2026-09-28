import QtQuick
import Quickshell
import Quickshell.Io

// armouryd connection shared by the bar widget and the window.
// - `events`: subscribed stream; `snap` is always the latest snapshot.
// - `requests`: one JSON request per line, answered in order, so callbacks
//   are matched by position in `pending`.
// Both sockets reconnect on their own; nothing here touches hardware.
Item {
  id: root

  readonly property string socketPath: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/armoury.sock"

  property var snap: null
  readonly property bool online: events.connected && snap !== null
  readonly property bool active: !!snap && snap.control === "active"
  property string lastError: ""

  property var pending: []

  signal snapshotChanged()

  // Sends `req` (an object) and calls `cb(response)`; response is {ok, data, error}.
  function call(req, cb) {
    if (!requests.connected) {
      root.lastError = "armouryd is not running"
      if (cb) cb({ ok: false, error: root.lastError })
      return
    }
    root.pending.push(cb || null)
    requests.write(JSON.stringify(req) + "\n")
    requests.flush()
  }

  // Fire-and-forget with error capture for the UI's error line.
  function run(req, onDone) {
    call(req, function(r) {
      root.lastError = r.ok ? "" : (r.error || "failed")
      if (onDone) onDone(r)
    })
  }

  Socket {
    id: events
    path: root.socketPath
    connected: true
    onConnectedChanged: {
      if (connected) {
        write(JSON.stringify({ cmd: "subscribe" }) + "\n")
        flush()
      } else {
        root.snap = null
      }
    }
    parser: SplitParser {
      onRead: function(line) {
        var msg
        try { msg = JSON.parse(line) } catch (e) { return }
        if (msg.event === "snapshot") {
          root.snap = msg.data
          root.snapshotChanged()
        }
      }
    }
  }

  Socket {
    id: requests
    path: root.socketPath
    connected: true
    onConnectedChanged: {
      if (!connected) {
        // fail everything still waiting; the daemon went away
        var waiting = root.pending
        root.pending = []
        for (var i = 0; i < waiting.length; i++)
          if (waiting[i]) waiting[i]({ ok: false, error: "armouryd disconnected" })
      }
    }
    parser: SplitParser {
      onRead: function(line) {
        var cb = root.pending.shift()
        var msg
        try { msg = JSON.parse(line) } catch (e) { msg = { ok: false, error: "bad response" } }
        if (cb) cb(msg)
      }
    }
  }

  // Reconnect when armouryd restarts.
  Timer {
    interval: 2000
    running: !events.connected || !requests.connected
    repeat: true
    onTriggered: {
      if (!events.connected) events.connected = true
      if (!requests.connected) requests.connected = true
    }
  }
}
