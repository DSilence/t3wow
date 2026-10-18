-- Pure helpers (no WoW frame API) so they can be tested outside the game.
local _, ns = ...
ns = ns or {}
local U = {}
ns.Util = U

local format, concat = string.format, table.concat

local STATUS = {
  idle = { "Idle", "808080" },
  preparing = { "Preparing", "ffd100" },
  queued = { "Queued", "ffd100" },
  starting = { "Starting", "ffd100" },
  running = { "Running", "33dd66" },
  waiting = { "Needs input", "ff9933" },
  completed = { "Done", "8fbf8f" },
  interrupted = { "Stopped", "aaaaaa" },
  failed = { "Failed", "ff5555" },
  cancelled = { "Cancelled", "aaaaaa" },
  rolled_back = { "Rolled back", "aaaaaa" },
}

U.ACTIVE = { preparing = true, queued = true, starting = true, running = true, waiting = true }

--- Label and hex color for a thread or run status.
function U.Status(status)
  local entry = STATUS[status or "idle"] or { tostring(status), "cccccc" }
  return entry[1], entry[2]
end

function U.Color(hex, text)
  return "|cff" .. hex .. text .. "|r"
end

--- Escape text for display: WoW treats `|` as the start of an escape sequence.
function U.Escape(text)
  return (tostring(text or ""):gsub("|", "||"))
end

--- Cut a UTF-8 string to at most `max` bytes without splitting a character.
function U.Truncate(text, max, ellipsis)
  text = tostring(text or "")
  if #text <= max then
    return text
  end
  local cut = max
  -- Step back over continuation bytes (10xxxxxx) to a character boundary.
  while cut > 0 do
    local byte = text:byte(cut + 1)
    if not byte or byte < 0x80 or byte >= 0xC0 then
      break
    end
    cut = cut - 1
  end
  return text:sub(1, cut) .. (ellipsis or "…")
end

function U.Trim(text)
  return (tostring(text or ""):gsub("^%s+", ""):gsub("%s+$", ""))
end

--- "now", "5m", "3h", "2d" for a unix timestamp.
function U.Ago(now, at)
  if not at or at <= 0 then
    return ""
  end
  local delta = now - at
  if delta < 45 then
    return "now"
  elseif delta < 3600 then
    return format("%dm", math.floor(delta / 60 + 0.5))
  elseif delta < 86400 then
    return format("%dh", math.floor(delta / 3600 + 0.5))
  end
  return format("%dd", math.floor(delta / 86400 + 0.5))
end

local ROLE = {
  user = { "You", "66bbff" },
  assistant = { "Agent", "d0a0ff" },
  plan = { "Plan", "ffd100" },
}

--- What the message view shows for a thread, as a list of entries:
---   { kind = "header", text, copy = raw message text }   who wrote it, when
---   { kind = "markdown", text = raw Markdown }
---   { kind = "note", text = display-ready text }
---   { kind = "option", text, action, picked = nil (single) or boolean (multi) }
---   { kind = "button", text, action, enabled }
-- `pending` lists outbox commands for this thread not yet delivered,
-- `results` lists delivery results to show (failures). `answering` is
-- { drafts, picks, interactive }: answers given to questions whose request
-- still waits for its other questions, ticked multi-select options by
-- U.PickKey, and whether options can be clicked.
function U.ThreadEntries(thread, pending, results, answering)
  answering = answering or {}
  local drafts, picks = answering.drafts, answering.picks or {}
  local entries = {}
  local function add(kind, text, copy)
    entries[#entries + 1] = { kind = kind, text = text, copy = copy }
    return entries[#entries]
  end
  if thread.partial then
    add("note", U.Color("808080", "Showing the latest messages. Press Sync to load the whole thread (reloads the UI)."))
  elseif thread.more then
    add("note", U.Color("808080", "Earlier messages are in T3 Code."))
  end
  for _, message in ipairs(thread.messages or {}) do
    if message.role == "user" and U.Trim(message.text) == "/compact" then
      -- T3 runs "/compact" as a compaction, not as a prompt.
      add("note", U.Color("808080", "Conversation compacted" .. (message.status ~= "completed" and " (in progress)" or "")))
    else
      local role = ROLE[message.role] or ROLE.assistant
      local header = U.Color(role[2], role[1])
      if message.at and message.at > 0 then
        header = header .. U.Color("808080", "  " .. date("%H:%M", message.at))
      end
      if message.status and message.status ~= "completed" then
        local label = U.Status(message.status)
        header = header .. U.Color("808080", "  (" .. label:lower() .. ")")
      end
      add("header", header, message.text)
      add("markdown", message.text)
      if message.cut then
        add("note", U.Color("808080", "[Shortened. Open T3 Code for the full message.]"))
      end
    end
  end
  for _, command in ipairs(pending or {}) do
    if command.kind == "send" and command.text == "/compact" then
      add("note", U.Color("808080", "Compact requested (waiting for the bridge)"))
    elseif command.kind == "send" or command.kind == "answer" then
      add("header", U.Color("66bbff", "You") .. U.Color("808080", "  (waiting for the bridge)"), command.text)
      add("note", U.Color("a0a0a0", U.Escape(command.text or command.preview or "")))
    elseif command.kind == "interrupt" then
      add("note", U.Color("808080", "Stop requested (waiting for the bridge)"))
    end
  end
  for _, result in ipairs(results or {}) do
    if not result.ok then
      add("note", U.Color("ff5555", "Not delivered: " .. U.Escape(result.message or "unknown error")))
    end
  end
  local questions = U.OpenQuestions(thread.questions, pending)
  local current = U.CurrentQuestion(questions, drafts)
  for index, question in ipairs(questions) do
    local title = "Question " .. index
    if #questions > 1 then
      title = title .. " of " .. #questions
    end
    add("header", U.Color("ff9933", title .. ": " .. U.Escape(question.header or "")), question.question)
    add("markdown", question.question)
    local answer = U.DraftAnswer(drafts, question)
    if answer ~= nil then
      add("note", U.Color("66bbff", "Your answer: ") .. U.Color("a0a0a0", U.Escape(U.AnswerText(answer)))
        .. U.Color("808080", "  (sent with the last question)"))
    elseif answering.interactive then
      local ticked = picks[U.PickKey(question)] or {}
      local count = 0
      for n, option in ipairs(question.options or {}) do
        local entry = add("option", format("%d. %s", n, U.OptionText(option)))
        entry.action = { kind = "pick", question = question, index = n }
        if question.multi then
          entry.picked = ticked[n] == true
          count = count + (entry.picked and 1 or 0)
        end
      end
      if question.multi then
        local button = add("button", "Send choices")
        button.action = { kind = "submit", question = question }
        button.enabled = count > 0
      end
      if question == current then
        local how = question.multi and "Tick options, then press Send choices." or "Click an option."
        if question.custom ~= false then
          how = how .. " Or type your own answer and press Answer."
        end
        add("note", U.Color("808080", how))
      end
    else
      local options = {}
      for n, option in ipairs(question.options or {}) do
        options[#options + 1] = format("  %d. %s", n, U.OptionText(option))
      end
      add("note", concat(options, "\n"))
    end
  end
  if U.ACTIVE[thread.status] and thread.activity then
    add("note", U.Color("33dd66", "Working: ") .. U.Color("c0c0c0", U.Escape(thread.activity)))
  end
  return entries
end

--- The option `text` names, by number or label.
local function findOption(options, text)
  local index = tonumber(text)
  if index and options[index] then
    return options[index]
  end
  for _, option in ipairs(options) do
    if option.label:lower() == text:lower() then
      return option
    end
  end
end

--- Turn composer text into an answer for a pending question: a number picks an
--- option. A multi-select question takes a comma or space separated list of
--- numbers, or comma separated labels, and answers with a list of values.
function U.AnswerFor(question, text)
  text = U.Trim(text)
  local options = question.options or {}
  if not question.multi then
    local option = findOption(options, text)
    return option and (option.value or option.label) or text
  end
  local parts = {}
  local separator = text:match("^[%d%s,]+$") and "[^%s,]+" or "[^,]+"
  for part in text:gmatch(separator) do
    part = U.Trim(part)
    if part ~= "" then
      local option = findOption(options, part)
      if not option then
        -- Not a list of options: the whole text is a custom answer.
        return { text }
      end
      parts[#parts + 1] = option.value or option.label
    end
  end
  return #parts > 0 and parts or { text }
end

--- An option's label and description as display text.
function U.OptionText(option)
  local text = U.Escape(option.label)
  if option.description and option.description ~= "" then
    text = text .. U.Color("a0a0a0", " - " .. U.Escape(option.description))
  end
  return text
end

--- Key for a question's ticked multi-select options.
function U.PickKey(question)
  return tostring(question.requestId) .. "\001" .. tostring(question.id)
end

--- The values of the ticked options, in option order.
function U.PickedValues(question, ticked)
  local values = {}
  for n, option in ipairs(question.options or {}) do
    if ticked and ticked[n] then
      values[#values + 1] = option.value or option.label
    end
  end
  return values
end

--- An answer (text or a list) as display text.
function U.AnswerText(answer)
  return type(answer) == "table" and concat(answer, ", ") or tostring(answer)
end

--- The answer already given to `question`, while its request waits for the rest.
function U.DraftAnswer(drafts, question)
  local draft = drafts and drafts[question.requestId]
  return draft and draft.answers and draft.answers[question.id]
end

--- Questions whose request has no answer waiting in the outbox.
function U.OpenQuestions(questions, pending)
  local sent = {}
  for _, command in ipairs(pending or {}) do
    if command.kind == "answer" then
      sent[command.requestId] = true
    end
  end
  local open = {}
  for _, question in ipairs(questions or {}) do
    if not sent[question.requestId] then
      open[#open + 1] = question
    end
  end
  return open
end

--- The first question still needing an answer.
function U.CurrentQuestion(questions, drafts)
  for _, question in ipairs(questions or {}) do
    if U.DraftAnswer(drafts, question) == nil then
      return question
    end
  end
end

--- First line of a prompt, shortened, as a thread title.
function U.TitleFor(text)
  local first = U.Trim(text):match("^[^\n]*") or ""
  first = first:gsub("^#+%s*", "")
  if first == "" then
    first = "New thread from WoW"
  end
  return U.Truncate(first, 80)
end

return U
