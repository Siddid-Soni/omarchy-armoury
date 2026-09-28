import QtQuick
import QtQuick.Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui
import "Model.js" as Model

// Armoury Crate for Omarchy — ASUS laptop control backed by the daemons
// Omarchy already preinstalls and updates: asusd/asusctl + supergfxd/supergfxctl.
//
// Design rules:
//   - Reads AND writes go through bin/omarchy-armoury (never sudo/pkexec here;
//     the daemons own privilege escalation via polkit).
//   - GPU switching uses supergfxctl (Eco=Integrated, Standard=Hybrid,
//     Ultimate=AsusMuxDgpu). Pending reboot/logout is surfaced, not hidden.
//   - Keyboard light uses asusctl leds + aura.
//   - Non-ASUS machines: widget hides itself (visible: hasAsus).
Panel {
  id: root
  moduleName: "asus.armoury"
  ipcTarget: "asus.armoury"
  manageIpc: false

  property bool hasAsus: false
  property bool asusdUp: false
  property string productName: ""
  property var profiles: []
  property string activeProfile: ""
  property string gpuMode: ""
  property var gpuSupported: []
  property string gpuPendingMode: ""
  property string gpuPendingAction: ""
  property string gpuPower: ""
  property string batteryInfo: ""
  property string kbdBrightness: ""
  property var auraZones: []
  // Advanced state (populated only when the backends answer).
  property string fanData: ""
  property var limits: []
  property string dispName: ""
  property string dispCurrent: ""
  property string dispPos: ""
  property string dispScale: ""
  property var dispModes: []
  property string pendingRate: ""
  property bool animePresent: false
  property string animeBright: "med"
  property bool slashPresent: false
  property var slashModes: []
  property string pendingSlashMode: ""
  property int slashBright: 128
  property string pendingFan: "cpu"
  property string cpuEpp: ""
  property string cpuGov: ""
  property var eppOptions: []
  property bool tuningOn: false
  property bool tuningKnown: false
  // Collapsed sections (advanced ones start collapsed to keep the panel short).
  property var collapsed: ({ "fans": true, "power": true, "anime": true, "slash": true })

  function toggleSection(name) {
    var c = Object.assign({}, root.collapsed);
    c[name] = !c[name];
    root.collapsed = c;
  }

  function startAsusd() {
    // Needs a visible terminal for the sudo prompt — same pattern as the
    // trigger>hardware menu launching the GPU toggle flow.
    Quickshell.execDetached(["omarchy-launch-floating-terminal-with-presentation", "sudo", "systemctl", "start", "asusd"]);
  }
  property string armouryAttrs: ""
  property string keystoneState: ""
  property string lastError: ""
  property string gpuTransitionNote: ""

  readonly property string armouryLabel: Model.gpuModeForArmoury(gpuMode)
  // GPU state is always known (supergfxd runs); profile needs asusd.
  // Never show a half-built label with a dangling separator.
  readonly property string heroSub: {
    var parts = [];
    if (activeProfile !== "") parts.push(activeProfile);
    if (armouryLabel !== "") parts.push(armouryLabel);
    var s = parts.join(" · ");
    if (gpuPower !== "") s += (s !== "" ? " · " : "") + "dGPU " + gpuPower;
    return s;
  }
  // Icon-only bar label: the " icon + label" form overflowed its slot and
  // collided with neighboring widgets. Details live in the tooltip + panel.
  readonly property string barText: {
    if (!hasAsus) return "";
    if (activeProfile !== "") return Model.profileIcon(activeProfile);
    if (gpuMode !== "") return Model.gpuIcon(gpuMode);
    return "󰌢";
  }

  // Backend helper. Install with: ln -sf <repo>/bin/omarchy-armoury ~/.local/bin/
  readonly property string helper: "omarchy-armoury"

  function refresh() {
    if (!statusProc.running) statusProc.running = true;
    if (!profilesProc.running) profilesProc.running = true;
    if (!gpuProc.running) gpuProc.running = true;
    if (!batteryProc.running) batteryProc.running = true;
    if (!kbdProc.running) kbdProc.running = true;
    if (!zonesProc.running) zonesProc.running = true;
    if (!limitsProc.running) limitsProc.running = true;
    if (!animeProc.running) animeProc.running = true;
    if (!slashProc.running) slashProc.running = true;
    if (!dispProc.running) dispProc.running = true;
    if (!tuningProc.running) tuningProc.running = true;
    if (!cpuProc.running) cpuProc.running = true;
    // Fan data is per active profile; an empty arg would only produce noise.
    if (root.activeProfile !== "" && !fanProc.running) {
      fanProc.command = [root.helper, "fan-get", root.activeProfile];
      fanProc.running = true;
    }
  }

  function updateStatus(raw) {
    var kv = Model.parseKeyValue(raw);
    hasAsus = kv["asus"] === "true";
    asusdUp = kv["asusd"] === "true";
    productName = kv["product"] || "";
  }

  function updateProfiles(raw) {
    var parsed = Model.parseProfiles(raw, activeProfile);
    if (parsed.profiles.length === 0) return;
    profiles = parsed.profiles;
    if (parsed.activeProfile !== "") activeProfile = parsed.activeProfile;
  }

  function updateGpu(raw) {
    var kv = Model.parseKeyValue(raw);
    if (kv["mode"]) gpuMode = kv["mode"];
    if (kv["supported"]) gpuSupported = Model.parseSupportedModes(kv["supported"]);
    gpuPendingMode = kv["pending_mode"] || "";
    gpuPendingAction = kv["pending_action"] || "";
    gpuPower = kv["power"] || "";
  }

  function updateLimits(raw) {
    var parsed = Model.parseLimits(raw);
    if (parsed.length === 0) return; // keep last good (or stay hidden)
    limits = parsed;
  }

  function updateDisplay(raw) {
    var d = Model.parseDisplayModes(raw);
    if (d.name === "" || d.modes.length === 0) return;
    dispName = d.name;
    dispCurrent = d.current;
    dispPos = d.pos;
    dispScale = d.scale;
    dispModes = d.modes;
    if (pendingRate === "") {
      if (d.modes.indexOf(d.current) >= 0) { pendingRate = d.current; return; }
      var res = String(d.current).split("@")[0];
      var same = d.modes.filter(function (m) { return String(m).split("@")[0] === res; });
      pendingRate = same.length > 0 ? same[0] : d.modes[0];
    }
  }

  function updateAnime(raw) {
    animePresent = Model.parseAnime(raw);
  }

  function updateSlash(raw) {
    var s = Model.parseSlash(raw);
    slashPresent = s.present;
    slashModes = s.modes;
    if (pendingSlashMode === "" && s.modes.length > 0) pendingSlashMode = s.modes[0];
  }

  function updateCpu(raw) {
    if (Model.isErrorPayload(raw)) return;
    var c = Model.parseCpu(raw);
    if (c.eppAvailable.length === 0) return;
    cpuEpp = c.epp;
    cpuGov = c.governor;
    eppOptions = c.eppAvailable;
  }

  function updateTuning(raw) {
    var t = Model.parseTuning(raw);
    if (t === "") return;
    tuningKnown = true;
    tuningOn = t === "true";
  }

  function setProfile(name) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "profile-set", name];
    actionProc.running = true;
  }

  // GPU switching policy (see helper header + README matrix):
  //   Integrated<->Hybrid -> Omarchy's terminal flow (config rewrite +
  //     support files + reboot); Hybrid<->Ultimate -> direct supergfxctl -m
  //     + reboot; Ultimate<->Integrated -> two-step via Hybrid.
  function setGpu(armouryMode) {
    if (actionProc.running) return;
    var step = Model.transitionFor(gpuMode, armouryMode, gpuSupported);
    lastError = "";
    gpuTransitionNote = "";
    if (step.action === "noop") return;
    if (step.action === "unsupported") {
      lastError = step.note || (armouryMode + " not supported on this machine");
      return;
    }
    if (step.action === "toggle") {
      // Same flow as trigger>hardware>Hybrid GPU: floating terminal, gum
      // confirm, reboot. Must run detached so the panel stays alive.
      Quickshell.execDetached([root.helper, "gpu-toggle"]);
      return;
    }
    if (step.action === "direct" || step.action === "twostep") {
      if (step.note) gpuTransitionNote = step.note;
      actionProc.command = [root.helper, "gpu-set", step.setMode];
      actionProc.running = true;
      return;
    }
    lastError = "Unknown GPU transition";
  }

  function setBatteryLimit(pct) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "battery-set", String(pct)];
    actionProc.running = true;
  }

  function setKbd(level) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "kbd-set", level];
    actionProc.running = true;
  }

  // Aura zone power is fire-and-forget: asusctl offers no state getter,
  // so these are momentary buttons, not toggles.
  function setAuraZone(zone, state) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "aura-power", zone, state];
    actionProc.running = true;
  }

  // ---- Advanced writers (all daemon-mediated, never privileged here) ----
  function setLimit(attr, value) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "armoury-set", attr, String(Math.round(value))];
    actionProc.running = true;
  }

  function setFanEnable(on) {
    if (actionProc.running || root.activeProfile === "") return;
    lastError = "";
    actionProc.command = [root.helper, "fan-enable", root.activeProfile, on ? "true" : "false"];
    actionProc.running = true;
  }

  function resetFans() {
    if (actionProc.running || root.activeProfile === "") return;
    lastError = "";
    actionProc.command = [root.helper, "fan-default", root.activeProfile];
    actionProc.running = true;
  }

  function setFanCustom(fan, data) {
    if (actionProc.running || root.activeProfile === "") return;
    lastError = "";
    actionProc.command = [root.helper, "fan-set", root.activeProfile, fan, data];
    actionProc.running = true;
  }

  function setDisplayRate() {
    if (actionProc.running || root.pendingRate === "") return;
    lastError = "";
    actionProc.command = [root.helper, "display-rate", root.dispName, root.pendingRate, root.dispPos, root.dispScale];
    actionProc.running = true;
  }

  function setAnime(on) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "anime-set", on ? "on" : "off", root.animeBright];
    actionProc.running = true;
  }

  function setSlash(on) {
    if (actionProc.running) return;
    lastError = "";
    var mode = root.pendingSlashMode !== "" ? root.pendingSlashMode : "_";
    actionProc.command = [root.helper, "slash-set", on ? "on" : "off", mode, String(root.slashBright)];
    actionProc.running = true;
  }

  function setTuning(on) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "tuning-set", on ? "true" : "false"];
    actionProc.running = true;
  }

  function batteryOneshot() {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "battery-oneshot"];
    actionProc.running = true;
  }

  function setCpuEpp(pref) {
    if (actionProc.running) return;
    lastError = "";
    actionProc.command = [root.helper, "cpu-epp", pref];
    actionProc.running = true;
  }

  function open() { root.controller.show(); }
  function close() { root.controller.hide(); }
  function toggle() { root.opened ? root.close() : root.open(); }
  function switchPanel(direction) {
    if (root.bar && typeof root.bar.switchPanelFrom === "function")
      return root.bar.switchPanelFrom(root, direction);
    return false;
  }

  IpcHandler {
    target: "asus.armoury"
    function open() { root.open(); }
    function close() { root.close(); }
    function show() { root.open(); }
    function hide() { root.close(); }
    function toggle() { root.toggle(); }
  }

  onOpenedChanged: if (opened) refresh();
  Component.onCompleted: refresh();

  visible: hasAsus
  implicitWidth: hasAsus ? button.implicitWidth : 0
  implicitHeight: hasAsus ? button.implicitHeight : 0

  Process {
    id: statusProc
    command: [root.helper, "status"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateStatus(text); }
  }
  Process {
    id: profilesProc
    command: [root.helper, "profiles"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateProfiles(text); }
  }
  Process {
    id: gpuProc
    command: [root.helper, "gpu"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateGpu(text); }
  }
  Process {
    id: batteryProc
    command: [root.helper, "battery"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        if (Model.isErrorPayload(text)) { root.batteryInfo = ""; return; }
        root.batteryInfo = Model.stripAnsi(text).replace(/\n/g, " ").trim();
      }
    }
  }
  Process {
    id: kbdProc
    command: [root.helper, "kbd"]
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        var kv = Model.parseKeyValue(text);
        if (kv["brightness"]) root.kbdBrightness = kv["brightness"];
      }
    }
  }
  Process {
    id: zonesProc
    command: [root.helper, "aura-zones"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.auraZones = Model.parseAuraZones(text); }
  }
  Process {
    id: limitsProc
    command: [root.helper, "limits"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateLimits(text); }
  }
  Process {
    id: fanProc
    stdout: StdioCollector {
      waitForEnd: true
      onStreamFinished: {
        if (Model.isErrorPayload(text)) return;
        root.fanData = Model.stripAnsi(text).trim();
      }
    }
  }
  Process {
    id: animeProc
    command: [root.helper, "anime-probe"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateAnime(text); }
  }
  Process {
    id: slashProc
    command: [root.helper, "slash-probe"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateSlash(text); }
  }
  Process {
    id: dispProc
    command: [root.helper, "display-modes"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateDisplay(text); }
  }
  Process {
    id: tuningProc
    command: [root.helper, "tuning"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateTuning(text); }
  }
  Process {
    id: cpuProc
    command: [root.helper, "cpu"]
    stdout: StdioCollector { waitForEnd: true; onStreamFinished: root.updateCpu(text); }
  }
  Process {
    id: actionProc
    stderr: StdioCollector {      waitForEnd: true
      onStreamFinished: {
        var clean = Model.stripAnsi(text).trim();
        if (clean !== "") root.lastError = clean.split("\n")[0];
      }
    }
    onExited: root.refresh();
  }

  Timer { interval: 8000; running: root.opened; repeat: true; onTriggered: root.refresh(); }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    text: root.barText
    tooltipText: root.hasAsus ? ("Armoury" + (root.heroSub !== "" ? " · " + root.heroSub : "")) : "Armoury (no ASUS device)"
    onPressed: function(b) {
      if (!root.hasAsus) return;
      if (b === Qt.RightButton) { root.refresh(); return; }
      root.toggle();
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened && root.hasAsus
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(380))
    contentHeight: panel.fittedContentHeight(column.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction); }

      Flickable {
        id: panelFlick
        anchors.fill: parent
        contentWidth: width
        contentHeight: column.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        flickableDirection: Flickable.VerticalFlick
        interactive: contentHeight > height
        ScrollBar.vertical: ScrollBar {
          policy: ScrollBar.AsNeeded
          contentItem: Rectangle {
            implicitWidth: 4
            radius: 2
            color: root.bar.foreground
            opacity: 0.28
          }
        }

        Column {
          id: column
          width: panelFlick.width
          spacing: Style.space(14)

        // ---------- Hero ----------
        Item {
          width: parent.width
          implicitHeight: Math.max(heroIcon.implicitHeight, heroLabels.implicitHeight)

          Text {
            id: heroIcon
            textFormat: Text.PlainText
            text: root.activeProfile !== "" ? Model.profileIcon(root.activeProfile) : "󰌢"
            color: root.bar.foreground
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.display
            anchors.left: parent.left
            anchors.verticalCenter: parent.verticalCenter
          }

          Column {
            id: heroLabels
            anchors.left: heroIcon.right
            anchors.leftMargin: Style.space(14)
            anchors.right: parent.right
            anchors.verticalCenter: parent.verticalCenter
            spacing: Style.space(2)

            Text {
              text: "Armoury"
              color: root.bar.foreground
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.title
              font.bold: true
              elide: Text.ElideRight
              width: parent.width
            }

            Text {
              textFormat: Text.PlainText
              text: root.heroSub.toUpperCase()
              color: Qt.darker(root.bar.foreground, 1.4)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
              font.bold: true
              font.letterSpacing: 1.2
              elide: Text.ElideRight
              width: parent.width
            }

            Text {
              visible: root.productName !== ""
              textFormat: Text.PlainText
              text: root.productName
              color: Qt.darker(root.bar.foreground, 1.6)
              font.family: root.bar.fontFamily
              font.pixelSize: Style.font.caption
              elide: Text.ElideRight
              width: parent.width
            }
          }
        }

        Text {
          visible: Model.hasPendingAction(root.gpuPendingAction)
          width: parent.width
          wrapMode: Text.WordWrap
          textFormat: Text.PlainText
          text: "⚠ GPU switch pending" + (root.gpuPendingMode !== "" && root.gpuPendingMode !== "Unknown" ? ": " + root.gpuPendingMode : "") + " — " + root.gpuPendingAction + ". Log out / reboot to apply."
          color: root.bar.foreground
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.caption
          font.bold: true
        }

        Text {
          visible: root.hasAsus && !root.asusdUp
          width: parent.width
          wrapMode: Text.WordWrap
          textFormat: Text.PlainText
          text: "asusd is not running — profiles, fans, battery, keyboard and Aura zones need it. GPU via supergfxctl works regardless."
          color: root.bar.foreground
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.caption
          font.bold: true
        }

        Button {
          visible: root.hasAsus && !root.asusdUp
          width: parent.width
          text: "Start asusd"
          foreground: root.bar.foreground
          fontFamily: root.bar.fontFamily
          bordered: true
          onClicked: root.startAsusd()
        }

        Text {
          visible: root.lastError !== ""
          width: parent.width
          wrapMode: Text.WordWrap
          textFormat: Text.PlainText
          text: root.lastError
          color: Color.urgent
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.caption
        }

        Text {
          visible: root.gpuTransitionNote !== ""
          width: parent.width
          wrapMode: Text.WordWrap
          textFormat: Text.PlainText
          text: root.gpuTransitionNote
          color: root.bar.foreground
          font.family: root.bar.fontFamily
          font.pixelSize: Style.font.caption
        }

        // ---------- Performance profiles (g-helper: Silent/Balanced/Turbo) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "perf"
          title: "PERFORMANCE"

          Row {
            width: parent.width
            spacing: Style.space(8)
            Repeater {
              model: root.profiles
              Button {
                required property var modelData
                width: (parent.width - parent.spacing * 2) / 3
                iconText: Model.profileIcon(String(modelData))
                text: String(modelData)
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                active: modelData === root.activeProfile
                onClicked: root.setProfile(modelData)
              }
            }
          }

          Toggle {
            visible: root.tuningKnown
            width: parent.width
            label: "Profile tuning"
            description: "Allow asusd to tune clocks within the profile"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            checked: root.tuningOn
            onClicked: root.setTuning(!root.tuningOn)
          }
        }

        // ---------- CPU energy preference (sysfs, no daemon needed) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "cpu"
          title: "CPU"
          available: root.eppOptions.length > 0

          Text {
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: "Governor: " + root.cpuGov + " (read-only)"
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }

          Dropdown {
            width: parent.width
            label: "Energy preference"
            fontFamily: root.bar.fontFamily
            options: root.eppOptions
            value: root.cpuEpp
            showLabel: false
            onChanged: function(v) { root.setCpuEpp(v); }
          }
        }

        // ---------- GPU mode via supergfxctl ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "gpu"
          title: "GPU · SUPERGFXCTL"

          Row {
            width: parent.width
            spacing: Style.space(8)
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "󰢮 Eco"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              active: root.armouryLabel === "Eco"
              onClicked: root.setGpu("Eco")
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "󰢵 Std"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              active: root.armouryLabel === "Standard"
              onClicked: root.setGpu("Standard")
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "󰢾 Ult"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              enabled: root.gpuSupported.some(function (m) { return String(m).toLowerCase() === "asmusmuxdgpu"; })
              active: root.armouryLabel === "Ultimate"
              onClicked: root.setGpu("Ultimate")
            }
          }

          Text {
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: "supergfx: " + (root.gpuMode || "—") + (root.gpuPower !== "" ? " · dGPU " + root.gpuPower : "")
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }
        }

        // ---------- Battery charge limit (g-helper: 40–100%) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "battery"
          title: "BATTERY"

          Text {
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: root.batteryInfo !== "" ? root.batteryInfo : "Charge limit managed by asusd"
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }

          Row {
            width: parent.width
            spacing: Style.space(8)
            Repeater {
              model: ["60", "80", "100"]
              Button {
                required property var modelData
                width: (parent.width - parent.spacing * 2) / 3
                text: modelData + "%"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.setBatteryLimit(parseInt(modelData, 10))
              }
            }
          }

          Button {
            width: parent.width
            text: "Full charge once"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            bordered: true
            onClicked: root.batteryOneshot()
          }
        }

        // ---------- Keyboard backlight + Aura ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "kbd"
          title: "KEYBOARD · AURA"

          Row {
            width: parent.width
            spacing: Style.space(8)
            Repeater {
              model: ["off", "low", "med", "high"]
              Button {
                required property var modelData
                width: (parent.width - parent.spacing * 3) / 4
                text: modelData
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                active: root.kbdBrightness.toLowerCase() === modelData
                onClicked: root.setKbd(modelData)
              }
            }
          }

          Text {
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: "Aura effects: asusctl aura effect [--next-mode] (static, breathe, rainbow-cycle, stars, …). Single colour: omarchy-armoury aura-static ff00ff [logo|lightbar|lid|rear-glow]."
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }

          Text {
            visible: root.auraZones.length === 0 && root.asusdUp
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: "No extra Aura zones exposed by this machine's firmware — keyboard only."
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }

          Repeater {
            model: root.auraZones
            Row {
              required property var modelData
              width: parent.width
              spacing: Style.space(8)
              Text {
                width: (parent.width - parent.spacing * 2) / 3
                textFormat: Text.PlainText
                text: modelData
                color: root.bar.foreground
                font.family: root.bar.fontFamily
                font.pixelSize: Style.font.caption
                anchors.verticalCenter: parent.verticalCenter
              }
              Button {
                width: (parent.width - parent.spacing * 2) / 3
                text: "On"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.setAuraZone(modelData, "on")
              }
              Button {
                width: (parent.width - parent.spacing * 2) / 3
                text: "Off"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.setAuraZone(modelData, "off")
              }
            }
          }
        }

        // ---------- Fans (active platform profile) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "fans"
          title: ("FANS · " + root.activeProfile).toUpperCase()
          available: root.activeProfile !== ""

          Text {
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: root.fanData !== "" ? root.fanData : "No curve data — asusd picks this up when it runs."
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }

          Row {
            width: parent.width
            spacing: Style.space(8)
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "Enable"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.setFanEnable(true)
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "Disable"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.setFanEnable(false)
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "Reset"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.resetFans()
            }
          }

          Row {
            width: parent.width
            spacing: Style.space(8)
            Dropdown {
              width: (parent.width - parent.spacing) * 0.3
              label: "Fan"
              fontFamily: root.bar.fontFamily
              options: ["cpu", "gpu"]
              value: root.pendingFan
              showLabel: false
              onChanged: function(v) { root.pendingFan = v; }
            }
            TextField {
              id: fanDataField
              width: (parent.width - parent.spacing) * 0.7
              placeholderText: "30c:5%,40c:15%,… (8 points)"
              foreground: root.bar.foreground
              font.family: root.bar.fontFamily
            }
          }

          Button {
            width: parent.width
            text: "Apply custom curve"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
            bordered: true
            onClicked: root.setFanCustom(root.pendingFan, fanDataField.text)
          }
        }

        // ---------- Power limits + display modes (armoury attrs) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "power"
          title: "POWER"
          available: root.limits.length > 0

          Repeater {
            model: root.limits
            Row {
              required property var modelData
              width: parent.width
              spacing: Style.space(8)
              NumberField {
                id: limitField
                width: parent.width - setBtn.width - parent.spacing
                label: modelData.label + (modelData.unit !== "" ? " (" + modelData.unit + ")" : "")
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                from: modelData.from
                to: modelData.to
                stepSize: modelData.step
                value: modelData.current
              }
              Button {
                id: setBtn
                anchors.verticalCenter: parent.verticalCenter
                text: "Set"
                foreground: root.bar.foreground
                fontFamily: root.bar.fontFamily
                bordered: true
                onClicked: root.setLimit(modelData.attr, limitField.value)
              }
            }
          }
        }

        // ---------- Refresh rate (internal display, Hyprland) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "display"
          title: "DISPLAY"
          available: root.dispModes.length > 0

          Text {
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: root.dispName + ": " + root.dispCurrent
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }

          Row {
            width: parent.width
            spacing: Style.space(8)
            Dropdown {
              width: parent.width - applyRateBtn.width - parent.spacing
              label: "Refresh rate"
              fontFamily: root.bar.fontFamily
              options: root.dispModes
              value: root.pendingRate
              showLabel: false
              onChanged: function(v) { root.pendingRate = v; }
            }
            Button {
              id: applyRateBtn
              anchors.verticalCenter: parent.verticalCenter
              text: "Apply"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.setDisplayRate()
            }
          }
        }

        // ---------- Anime Matrix (if present) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "anime"
          title: "ANIME MATRIX"
          available: root.animePresent

          Row {
            width: parent.width
            spacing: Style.space(8)
            Dropdown {
              width: (parent.width - parent.spacing * 2) / 3
              label: "Brightness"
              fontFamily: root.bar.fontFamily
              options: ["off", "low", "med", "high"]
              value: root.animeBright
              showLabel: false
              onChanged: function(v) { root.animeBright = v; }
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "On"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.setAnime(true)
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "Off"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.setAnime(false)
            }
          }
        }

        // ---------- Slash LED bar (if present) ----------
        PanelSeparator { foreground: root.bar.foreground }

        Collapsible {
          section: "slash"
          title: "SLASH"
          available: root.slashPresent

          Dropdown {
            width: parent.width
            label: "Animation"
            fontFamily: root.bar.fontFamily
            options: root.slashModes
            value: root.pendingSlashMode
            showLabel: false
            onChanged: function(v) { root.pendingSlashMode = v; }
          }

          Row {
            width: parent.width
            spacing: Style.space(8)
            NumberField {
              id: slashBrightField
              width: (parent.width - parent.spacing * 2) / 3
              label: "Brightness"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              from: 0
              to: 255
              stepSize: 1
              value: root.slashBright
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "Enable"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: { root.slashBright = slashBrightField.value; root.setSlash(true); }
            }
            Button {
              width: (parent.width - parent.spacing * 2) / 3
              text: "Disable"
              foreground: root.bar.foreground
              fontFamily: root.bar.fontFamily
              bordered: true
              onClicked: root.setSlash(false)
            }
          }
        }

        // ---------- Advanced footer ----------
        PanelSeparator { foreground: root.bar.foreground }

        Column {
          width: parent.width
          spacing: Style.space(10)

          PanelSectionHeader {
            text: "ADVANCED"
            foreground: root.bar.foreground
            fontFamily: root.bar.fontFamily
          }

          Text {
            width: parent.width
            wrapMode: Text.WordWrap
            textFormat: Text.PlainText
            text: "Sections above appear when their backend answers (asusd for fans/power/Anime/Slash, Hyprland for refresh rate). Raw CLI stays available: `omarchy-armoury fan-get Balanced`, `armoury-list`, `asusctl anime …`. Keystone actions ship in Phase 2 (see README)."
            color: Qt.darker(root.bar.foreground, 1.4)
            font.family: root.bar.fontFamily
            font.pixelSize: Style.font.caption
          }
        } // ADVANCED footer Column
      } // main column
    } // panelFlick
  } // PanelKeyCatcher
} // KeyboardPanel

  // Collapsible section: header row (title + chevron) always visible,
  // body collapses. `available` hides the whole section (backend silent).
  component Collapsible: Column {
    property string section: ""
    property string title: ""
    property bool available: true
    default property alias body: bodyCol.children

    width: parent.width
    spacing: Style.space(10)
    visible: available

    readonly property bool isCollapsed: !!root.collapsed[section]

    Row {
      width: parent.width
      spacing: Style.space(8)

      PanelSectionHeader {
        id: collHeader
        text: title
        foreground: root.bar.foreground
        fontFamily: root.bar.fontFamily
      }

      Item {
        width: Math.max(0, parent.width - collHeader.implicitWidth - collBtn.width - parent.spacing * 2)
        height: 1
      }

      PanelActionButton {
        id: collBtn
        anchors.verticalCenter: parent.verticalCenter
        iconText: parent.parent.isCollapsed ? "+" : "-"
        foreground: root.bar.foreground
        tooltipText: parent.parent.isCollapsed ? "Expand" : "Collapse"
        onClicked: root.toggleSection(parent.parent.section)
      }
    }

    Column {
      id: bodyCol
      width: parent.width
      spacing: Style.space(10)
      visible: !parent.isCollapsed
    }
  }

  component InfoLabel: Text {    color: Qt.darker(root.bar.foreground, 1.4)
    font.family: root.bar.fontFamily
    font.pixelSize: Style.font.caption
  }
  component InfoValue: Text {
    textFormat: Text.PlainText
    color: root.bar.foreground
    font.family: root.bar.fontFamily
    font.pixelSize: Style.font.caption
    elide: Text.ElideRight
  }
}
