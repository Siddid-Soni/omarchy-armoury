-- omarchy-armoury: SUPER+W also closes the Armoury window. It is a layer-shell
-- overlay, not a window, so the stock "Close window" bind cannot reach it.
-- Installed by install.sh and loaded from hypr/bindings.lua; uninstall.sh removes both.
local function armoury_open()
  for _, layer in ipairs(hl.get_layers() or {}) do
    if layer.namespace == "asus-armoury" then return true end
  end
  return false
end

hl.unbind("SUPER + W")
o.bind("SUPER + W", "Close window", function()
  if armoury_open() then
    hl.exec_cmd("omarchy-shell shell hide io.github.siddid-soni.armoury")
  else
    hl.dispatch(hl.dsp.window.close())
  end
end)
