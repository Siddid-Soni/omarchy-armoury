import QtQuick
import Quickshell
import Quickshell.Io

// armouryd connection shared by the bar widget and the window.
// - `events`: subscribed stream; `snap` is always the latest snapshot.
// - `requests`: one JSON request per line, answered in order, so callbacks
//   are matched by position in `pending`.
// Both sockets reconnect on their own; nothing here touches hardware.
// Each Socket lives in a Loader: once a connection is refused (armouryd restarting), a
// Quickshell Socket never connects again whatever is set on it (measured on 0.3.1), so the
// reconnect recreates it.
Item {
  id: root

  readonly property string socketPath: (Quickshell.env("XDG_RUNTIME_DIR") || "/tmp") + "/armoury.sock"

  property var snap: null
  readonly property bool eventsConnected: !!eventsLoader.item && eventsLoader.item.connected
  readonly property bool requestsConnected: !!requestsLoader.item && requestsLoader.item.connected
  readonly property bool online: eventsConnected && snap !== null
  readonly property bool active: !!snap && snap.control === "active"
  property string lastError: ""

  property var pending: []

  // Music lighting's band levels (bass → treble, 0..1) while it runs; [] otherwise.
  property var bands: []

  // Short histories for the dashboard graphs, sampled from the snapshot (it only arrives
  // on change). hist: every 2 s for 3 min; battery: every 20 s for 30 min.
  property var hist: ({ cpuTemp: [], cpuFan: [], gpuFan: [], power: [] })
  property var batteryHist: []
  readonly property int histLen: 90
  function push(list, v, len) { var l = list.slice(Math.max(0, list.length - len + 1)); l.push(v); return l }
  Timer {
    interval: 2000
    running: root.online
    repeat: true
    triggeredOnStart: true
    property int tick: 0
    onTriggered: {
      var p = root.snap.perf || {}
      var h = root.hist
      root.hist = {
        cpuTemp: root.push(h.cpuTemp, p.cpu_temp_c || 0, root.histLen),
        cpuFan: root.push(h.cpuFan, p.cpu_fan_rpm || 0, root.histLen),
        gpuFan: root.push(h.gpuFan, p.gpu_fan_rpm || 0, root.histLen),
        power: root.push(h.power, p.power_draw_w || 0, root.histLen)
      }
      var b = root.snap.battery_info || {}
      if (tick % 10 === 0 && b.capacity !== undefined && b.capacity !== null) root.batteryHist = root.push(root.batteryHist, b.capacity, root.histLen)
      tick++
    }
  }

  signal snapshotChanged()

  // Sends `req` (an object) and calls `cb(response)`; response is {ok, data, error}.
  function call(req, cb) {
    if (!root.requestsConnected) {
      root.lastError = "armouryd is not running"
      if (cb) cb({ ok: false, error: root.lastError })
      return
    }
    root.pending.push(cb || null)
    requestsLoader.item.write(JSON.stringify(req) + "\n")
    requestsLoader.item.flush()
  }

  // Fire-and-forget with error capture for the UI's error line.
  function run(req, onDone) {
    call(req, function(r) {
      root.lastError = r.ok ? "" : (r.error || "failed")
      if (onDone) onDone(r)
    })
  }

  Loader {
    id: eventsLoader
    sourceComponent: Socket {
      path: root.socketPath
      connected: true
      onConnectedChanged: {
        if (connected) {
          write(JSON.stringify({ cmd: "subscribe" }) + "\n")
          flush()
        } else {
          root.snap = null
          root.bands = []
        }
      }
      parser: SplitParser {
        onRead: function(line) {
          var msg
          try { msg = JSON.parse(line) } catch (e) { return }
          if (msg.event === "snapshot") {
            root.snap = msg.data
            root.snapshotChanged()
            if (!msg.data.lighting || msg.data.lighting.music !== "on") root.bands = []
          } else if (msg.event === "music_bands") {
            root.bands = msg.data || []
          }
        }
      }
    }
  }

  Loader {
    id: requestsLoader
    sourceComponent: Socket {
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
  }

  // Reconnect when armouryd restarts: a fresh Socket each try (see the top).
  Timer {
    interval: 2000
    running: !root.eventsConnected || !root.requestsConnected
    repeat: true
    onTriggered: {
      if (!root.eventsConnected) { root.snap = null; eventsLoader.active = false; eventsLoader.active = true }
      if (!root.requestsConnected) { requestsLoader.active = false; requestsLoader.active = true }
    }
  }
}
