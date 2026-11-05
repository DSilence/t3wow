-- Voice dictation. Addons cannot use the microphone: the T3 WoW app listens
-- while the player holds its shortcut, recognizes the speech and publishes
-- the text in the snapshot (`dictation`). It goes into the composer once,
-- for the player to check and send.
local ADDON, ns = ...
local T3 = ns.T3

-- A dictation older than this (seconds) is from an earlier session.
local FRESH_FOR = 120
-- The app stops listening after 55 seconds; a "listening" state older than
-- this was left behind by a bridge that went away.
local LISTENING_FOR = 70
-- Live update interval while the app listens, so the text shows up quickly.
T3.DICTATION_INTERVAL = 1

local function dictation(self)
  local take = self.snapshot and self.snapshot.dictation
  if type(take) == "table" and type(take.id) == "number" then
    return take
  end
end

local function age(take)
  return time() - math.floor(take.id / 1000)
end

-- The shortcut while the app listens for it (dictation on, game running).
function T3:DictationShortcut()
  local bridge = self.snapshot and self.snapshot.bridge
  return type(bridge) == "table" and type(bridge.dictation) == "table" and bridge.dictation.shortcut or nil
end

function T3:IsDictating()
  local take = dictation(self)
  return take ~= nil and take.state == "listening" and age(take) < LISTENING_FOR
end

-- Called with every snapshot: insert a finished dictation once.
function T3:CheckDictation()
  local take = dictation(self)
  if not take or take.state == "listening" or T3WowDB.lastDictation == take.id then
    return
  end
  T3WowDB.lastDictation = take.id
  if age(take) > FRESH_FOR then
    return
  end
  if take.state == "done" and type(take.text) == "string" and take.text ~= "" then
    self:InsertDictation(take.text)
  elseif take.state == "error" then
    self:Print("Dictation: " .. tostring(take.message or "it did not work"))
  end
end

-- Put dictated text into the composer: at the cursor while typing, else at
-- the end. Keyboard focus stays where it was, so movement keys keep working.
function T3:InsertDictation(text)
  if self.pendingDictation then
    text = text == "" and self.pendingDictation or (self.pendingDictation .. " " .. text)
    self.pendingDictation = nil
  end
  if text == "" then
    return
  end
  if not self.frame then
    self.pendingDictation = text
    return
  end
  if not self.frame:IsShown() then
    self.frame:Show()
  end
  if not self.composerEnabled then
    self.pendingDictation = text
    self:Print("Dictated text is waiting: open a thread or start a new one and it goes into the message box.")
    return
  end
  local composer = self.composer
  local current = composer:GetText()
  local typing = composer:HasFocus()
  local cursor = typing and composer:GetCursorPosition() or #current
  local before, after = current:sub(1, cursor), current:sub(cursor + 1)
  if before ~= "" and not before:match("%s$") then
    text = " " .. text
  end
  if after ~= "" and not after:match("^%s") then
    text = text .. " "
  end
  if typing then
    composer:SetCursorPosition(cursor)
    composer:Insert(text)
  else
    composer:SetText(before .. text .. after)
  end
end

-- Dictated text that arrived while no composer was open.
function T3:ApplyPendingDictation()
  if self.pendingDictation and self.composerEnabled then
    self:InsertDictation("")
  end
end
