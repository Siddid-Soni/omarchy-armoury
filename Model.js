// Model.js — pure logic for the Armoury plugin (no Qt imports).
// All hardware I/O happens in bin/omarchy-armoury; this file only parses.

function stripAnsi(s) {
  return String(s || "").replace(/\x1b\[[0-9;]*[mK]/g, "");
}

function isErrorPayload(raw) {
  // Helper emits real TSV tabs, but tolerate literal backslash-t too —
  // a literal marker must never render as panel content either way.
  return /^error(?:\t|\\t)/m.test(String(raw || ""));
}

// supergfxctl -p reports "No action required" and -P "Unknown" when idle.
// Only logout/reboot-class actions deserve the pending banner.
function hasPendingAction(pendingAction) {
  return /logout|reboot/i.test(String(pendingAction || ""));
}

function parseKeyValue(raw) {
  var next = {};
  var lines = String(raw || "").split("\n");
  for (var i = 0; i < lines.length; i++) {
    var idx = lines[i].indexOf("\t");
    if (idx <= 0) continue;
    next[lines[i].substring(0, idx)] = lines[i].substring(idx + 1).trim();
  }
  return next;
}

function parseLines(raw) {
  var out = [];
  var lines = String(raw || "").split("\n");
  for (var i = 0; i < lines.length; i++) {
    var line = lines[i].trim();
    if (line) out.push(line);
  }
  return out;
}

// asusctl profile list/get output varies by version; accept either
// "Quiet\nBalanced\nPerformance" or "profile\tactive" TSV.
function parseProfiles(raw, fallbackActive) {
  if (isErrorPayload(raw)) return { profiles: [], activeProfile: fallbackActive || "" };
  var lines = stripAnsi(raw).split("\n");
  var list = [];
  var active = fallbackActive || "";
  for (var i = 0; i < lines.length; i++) {
    var line = lines[i].trim();
    if (!line) continue;
    // Belt-and-braces: never let a daemon error become a profile button.
    if (/asusd|not running|error/i.test(line)) continue;
    if (line.indexOf("\t") >= 0) {
      var parts = line.split("\t");
      list.push(parts[0]);
      if (parts[1] === "1" || parts[1] === "active") active = parts[0];
    } else {
      list.push(line);
    }
  }
  // No fallback list: fake profiles with no active marker are worse than none.
  // The panel's asusd banner explains the empty state.
  return { profiles: list, activeProfile: active };
}

// Aura zones the daemon persists in enabled.states (lowercase).
// Keyboard is managed by the brightness row, so it is excluded here.
function parseAuraZones(raw) {
  var out = [];
  var lines = stripAnsi(raw).split("\n");
  for (var i = 0; i < lines.length; i++) {
    var parts = lines[i].trim().split("\t");
    if (parts.length < 2 || parts[0] !== "zone") continue;
    var z = parts[1].trim().toLowerCase();
    if (z === "" || z === "keyboard" || z === "none") continue;
    if (out.indexOf(z) < 0) out.push(z);
  }
  return out;
}
function parseSupportedModes(raw) {
  var m = String(raw || "").match(/\[(.*)\]/);
  var src = m ? m[1] : String(raw || "");
  return src.split(",").map(function (s) { return s.trim(); }).filter(function (s) { return s.length > 0; });
}

// ---- Advanced: power limits / display / anime / fans ----

// Fallback ranges when asusd won't disclose min/max (it validates on set,
// so a rejected value surfaces as an error, never a bad write).
var LIMIT_DEFS = [
  { attr: "ppt_pl1_spl", label: "CPU PL1", unit: "W", from: 5, to: 120, step: 1 },
  { attr: "ppt_pl2_sppt", label: "CPU PL2", unit: "W", from: 5, to: 150, step: 1 },
  { attr: "ppt_pl3_fppt", label: "CPU FPPT", unit: "W", from: 5, to: 150, step: 1 },
  { attr: "nv_dynamic_boost", label: "Dynamic Boost", unit: "W", from: 0, to: 25, step: 1 },
  { attr: "nv_temp_target", label: "GPU temp target", unit: "°C", from: 75, to: 100, step: 1 },
  { attr: "mini_led_mode", label: "MiniLED", unit: "", from: 0, to: 2, step: 1 },
  { attr: "panel_od", label: "Panel Overdrive", unit: "", from: 0, to: 1, step: 1 }
];

function limitDef(attr) {
  for (var i = 0; i < LIMIT_DEFS.length; i++)
    if (LIMIT_DEFS[i].attr === attr) return LIMIT_DEFS[i];
  return null;
}

// lines: limit<TAB>attr<TAB>current
function parseLimits(raw) {
  var out = [];
  var lines = stripAnsi(raw).split("\n");
  for (var i = 0; i < lines.length; i++) {
    var parts = lines[i].trim().split("\t");
    if (parts.length < 3 || parts[0] !== "limit") continue;
    var def = limitDef(parts[1]);
    if (!def) continue;
    var cur = parseInt(parts[2], 10);
    if (isNaN(cur)) continue;
    out.push({ attr: def.attr, label: def.label, unit: def.unit,
      from: def.from, to: def.to, step: def.step, current: cur });
  }
  return out;
}

function parseDisplayModes(raw) {
  var kv = {};
  var modes = [];
  var lines = String(raw || "").split("\n");
  for (var i = 0; i < lines.length; i++) {
    var idx = lines[i].indexOf("\t");
    if (idx <= 0) continue;
    var k = lines[i].substring(0, idx), v = lines[i].substring(idx + 1).trim();
    if (k === "mode") { if (v !== "") modes.push(v); }
    else kv[k] = v;
  }
  return { name: kv["monitor"] || "", current: kv["current"] || "",
    pos: kv["pos"] || "", scale: kv["scale"] || "", modes: modes };
}

function parseSlash(raw) {
  var modes = [];
  var lines = stripAnsi(raw).split("\n");
  for (var i = 0; i < lines.length; i++) {
    var parts = lines[i].trim().split("\t");
    if (parts.length < 2 || parts[0] !== "slashmode") continue;
    if (parts[1] !== "" && modes.indexOf(parts[1]) < 0) modes.push(parts[1]);
  }
  return { present: modes.length > 0, modes: modes };
}

function parseAnime(raw) {
  return stripAnsi(raw).indexOf("present") >= 0;
}

function parseCpu(raw) {
  var kv = parseKeyValue(raw);
  var avail = String(kv["epp_available"] || "").split(" ").map(function (s) { return s.trim(); })
    .filter(function (s) { return s.length > 0; });
  return { epp: kv["epp"] || "", governor: kv["governor"] || "", eppAvailable: avail };
}

function parseTuning(raw) {
  if (isErrorPayload(raw)) return "";
  var t = stripAnsi(raw).trim().toLowerCase();
  if (t === "true" || t === "false") return t;
  return "";
}

function profileIcon(name) {
  var n = String(name || "").toLowerCase();
  if (n.indexOf("quiet") >= 0 || n.indexOf("silent") >= 0 || n.indexOf("power-saver") >= 0) return "󰌪";
  if (n.indexOf("performance") >= 0 || n.indexOf("turbo") >= 0) return "󰓅";
  return "󰊚"; // Balanced / default
}

function gpuIcon(mode) {
  var m = String(mode || "").toLowerCase();
  if (m.indexOf("integrated") >= 0) return "󰢮"; // Eco
  if (m.indexOf("mux") >= 0 || m.indexOf("dgpu") >= 0 || m.indexOf("ultimate") >= 0) return "󰢾";
  if (m.indexOf("vfio") >= 0) return "󰘚";
  return "󰢵"; // Hybrid / Standard
}

// ---- GPU transition matrix (Omarchy-aware) ----
// Canonical supergfx modes we care about.
function normalizeSupergfxMode(raw) {
  var m = String(raw || "").toLowerCase();
  if (m.indexOf("asmusmuxdgpu") >= 0 || m.indexOf("mux") >= 0 || m.indexOf("ultimate") >= 0) return "AsusMuxDgpu";
  if (m.indexOf("integrated") >= 0 || m === "eco") return "Integrated";
  if (m.indexOf("vfio") >= 0) return "Vfio";
  if (m.indexOf("hybrid") >= 0 || m === "standard") return "Hybrid";
  return "";
}

function supports(supported, mode) {
  return (supported || []).some(function (s) { return String(s).toLowerCase() === mode.toLowerCase(); });
}

// Returns a step object for the panel:
//   { action: "noop" }
//   { action: "toggle" }                              -> launch omarchy-toggle-hybrid-gpu terminal
//   { action: "direct", setMode: "Hybrid"|"AsusMuxDgpu" } -> helper gpu-set + reboot
//   { action: "twostep", setMode, then: "toggle", note } -> direct now, Omarchy toggle after reboot
//   { action: "unsupported", note }
function transitionFor(currentRaw, armouryTarget, supported) {
  var current = normalizeSupergfxMode(currentRaw);
  if (current === "") return { action: "unsupported", note: "Unknown current GPU mode" };

  if (armouryTarget === "Eco" || armouryTarget === "Standard") {
    if ((armouryTarget === "Eco" && current === "Integrated") ||
        (armouryTarget === "Standard" && current === "Hybrid")) return { action: "noop" };
    if (current === "Integrated" || current === "Hybrid")
      return { action: "toggle" }; // Omarchy flow: config rewrite + support files + reboot
    if (current === "AsusMuxDgpu") {
      if (armouryTarget === "Standard") {
        if (!supports(supported, "Hybrid")) return { action: "unsupported", note: "Hybrid not in supported modes" };
        return { action: "direct", setMode: "Hybrid" };
      }
      // Eco from Ultimate: MUX must unlatch first via Hybrid + reboot.
      return { action: "twostep", setMode: "Hybrid", then: "toggle",
        note: "Step 1/2: switch to Hybrid and reboot, then use Eco (Omarchy toggle) to reach Integrated." };
    }
    return { action: "unsupported", note: "No path from " + current };
  }

  if (armouryTarget === "Ultimate") {
    if (current === "AsusMuxDgpu") return { action: "noop" };
    if (!supports(supported, "AsusMuxDgpu"))
      return { action: "unsupported", note: "No MUX on this machine (AsusMuxDgpu not reported)" };
    if (current === "Hybrid") return { action: "direct", setMode: "AsusMuxDgpu" };
    if (current === "Integrated")
      return { action: "twostep", setMode: "AsusMuxDgpu", then: "none",
        note: "Leaves Omarchy's Integrated delay-start.conf behind (harmless 5s delay); it is cleaned on the next Omarchy toggle run. Reboot required." };
    return { action: "unsupported", note: "No path from " + current };
  }

  return { action: "unsupported", note: "Unknown target " + armouryTarget };
}
function gpuModeForArmoury(supergfxMode) {
  if (String(supergfxMode || "").trim() === "") return "";
  var m = String(supergfxMode || "").toLowerCase();
  if (m.indexOf("integrated") >= 0) return "Eco";
  if (m.indexOf("muxdgpu") >= 0) return "Ultimate";
  if (m.indexOf("vfio") >= 0) return "Vfio";
  return "Standard";
}

function supergfxModeForArmoury(armouryMode, supported) {
  var sup = supported || [];
  function has(mode) {
    return sup.some(function (s) { return s.toLowerCase() === mode.toLowerCase(); });
  }
  if (armouryMode === "Eco") return has("Integrated") ? "Integrated" : null;
  if (armouryMode === "Ultimate") return has("AsusMuxDgpu") ? "AsusMuxDgpu" : null;
  if (armouryMode === "Vfio") return has("Vfio") ? "Vfio" : null;
  return has("Hybrid") ? "Hybrid" : null; // Standard
}

function clampInt(v, lo, hi, fallback) {
  var n = parseInt(v, 10);
  if (isNaN(n)) return fallback;
  return Math.max(lo, Math.min(hi, n));
}

if (typeof module !== "undefined") {
  module.exports = {
    stripAnsi: stripAnsi,
    isErrorPayload: isErrorPayload,
    hasPendingAction: hasPendingAction,
    parseKeyValue: parseKeyValue,
    parseLines: parseLines,
    parseProfiles: parseProfiles,
    parseAuraZones: parseAuraZones,
    parseLimits: parseLimits,
    parseDisplayModes: parseDisplayModes,
    parseSlash: parseSlash,
    parseAnime: parseAnime,
    parseCpu: parseCpu,
    parseTuning: parseTuning,
    parseSupportedModes: parseSupportedModes,
    profileIcon: profileIcon,
    gpuIcon: gpuIcon,
    gpuModeForArmoury: gpuModeForArmoury,
    supergfxModeForArmoury: supergfxModeForArmoury,
    normalizeSupergfxMode: normalizeSupergfxMode,
    transitionFor: transitionFor,
    clampInt: clampInt
  };
}
