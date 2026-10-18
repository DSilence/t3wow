-- A loose stand-in for the WoW UI API, enough to run the addon's code paths
-- outside the game. Widgets accept any method call; the ones whose return
-- values the addon uses are implemented.

local stub = { events = {}, reloads = 0, loaded = {}, prints = {}, sounds = {}, clock = 1000, now = 1760000000 }
_G.__stub = stub

local Widget = {}
Widget.__index = function(self, key)
  local method = rawget(Widget, key)
  if method then
    return method
  end
  -- Unknown methods are no-ops.
  return function()
    return nil
  end
end

local function widget(kind, name)
  local w = setmetatable({ kind = kind, shown = true, text = "", scripts = {}, children = {}, width = 400, height = 300 }, Widget)
  if name then
    _G[name] = w
  end
  return w
end

function Widget:Show() self.shown = true; local s = self.scripts.OnShow; if s then s(self) end end
function Widget:Hide() self.shown = false; local s = self.scripts.OnHide; if s then s(self) end end
function Widget:SetShown(v) if v then self:Show() else self:Hide() end end
function Widget:IsShown() return self.shown end
function Widget:SetText(t)
  self.text = t or ""
  local s = self.scripts.OnTextChanged
  if s then s(self, false) end
end
function Widget:GetText() return self.text end
function Widget:Insert(t) self.text = self.text .. t end
function Widget:GetCursorPosition() return #self.text end
function Widget:SetScript(event, fn) self.scripts[event] = fn end
function Widget:HookScript(event, fn)
  local old = self.scripts[event]
  self.scripts[event] = function(...) if old then old(...) end fn(...) end
end
function Widget:GetScript(event) return self.scripts[event] end
function Widget:RegisterEvent(event) stub.events[event] = stub.events[event] or {}; table.insert(stub.events[event], self) end
function Widget:CreateTexture() return widget("Texture") end
function Widget:CreateFontString() return widget("FontString") end
function Widget:GetHighlightTexture() return widget("Texture") end
function Widget:GetFont() return "Fonts\\ARIALN.TTF", 14, "" end
function Widget:GetPoint() return "CENTER", nil, "CENTER", 0, 0 end
function Widget:GetSize() return self.width, self.height end
function Widget:SetSize(w, h) self.width, self.height = w, h end
function Widget:GetWidth() return self.width end
function Widget:SetWidth(w) self.width = w end
function Widget:GetHeight() return self.height end
function Widget:SetHeight(h) self.height = h end
-- One 14px line per line of text; wrapping is not simulated.
function Widget:GetStringHeight()
  local _, breaks = (self.text or ""):gsub("\n", "")
  return (breaks + 1) * 14
end
function Widget:IsMouseOver() return false end
function Widget:SetColorTexture(r, g, b, a) self.color = { r, g, b, a } end
function Widget:GetVerticalScroll() return 0 end
function Widget:GetVerticalScrollRange() return 0 end
function Widget:SetEnabled(v) self.enabled = v end
function Widget:Click() local s = self.scripts.OnClick; if s then s(self, "LeftButton") end end

function CreateFrame(kind, name) return widget(kind, name) end
function CreateFont(name) return widget("Font", name) end
UIParent = widget("Frame")
Minimap = widget("Frame")
SOUNDKIT = { TELL_MESSAGE = 3081, READY_CHECK = 8960 }
function PlaySound(id) table.insert(stub.sounds, id) end
stub.cvars = { screenshotFormat = "tga", screenshotQuality = "3" }
C_CVar = {
  GetCVar = function(name) return stub.cvars[name] end,
  SetCVar = function(name, value) stub.cvars[name] = tostring(value) end,
}
ChatFontNormal = widget("Font")
GameTooltip = widget("GameTooltip")
GameTooltip_Hide = function() end
UISpecialFrames = {}
SlashCmdList = {}
DEFAULT_CHAT_FRAME = { AddMessage = function(_, message) table.insert(stub.prints, message) end }
BackdropTemplateMixin = {}

function time() return stub.now end
function GetTime() return stub.clock end
date = os.date
function InCombatLockdown() return stub.inCombat or false end
function ReloadUI() stub.reloads = stub.reloads + 1 end
function IsControlKeyDown() return stub.ctrl or false end
function IsShiftKeyDown() return stub.shift or false end

C_Timer = {
  After = function(_, fn) fn() end,
  NewTicker = function(_, fn) stub.ticker = fn; return {} end,
}

C_AddOns = {
  GetAddOnInfo = function(name)
    local index = tonumber(name:match("^T3Wow_Live(%d+)$"))
    if index and index <= (stub.liveSlots or 0) then
      return name, name, "", true, nil
    end
    return nil, nil, nil, false, "MISSING"
  end,
  IsAddOnLoaded = function(name) return stub.loaded[name] or false end,
  LoadAddOn = function(name)
    stub.loaded[name] = true
    if stub.onLoadAddOn then stub.onLoadAddOn(name) end
    return true
  end,
}

function stub.fire(event, ...)
  for _, frame in ipairs(stub.events[event] or {}) do
    frame.scripts.OnEvent(frame, event, ...)
  end
end
