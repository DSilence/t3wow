import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { dataChunk, toLua, type LuaValue } from "./lua.ts";
import { loadAddon, LuaVM } from "./harness.ts";

const NOW = 1760000000;

function snapshot(fields: Record<string, LuaValue>, seq = 1): string {
  return toLua({ v: 1, seq, generatedAt: NOW, bridge: { status: "ok", server: "env.example", canOperate: true }, ...fields });
}

const PROJECTS: LuaValue = [
  {
    id: "p1",
    title: "Game | Tools",
    threads: [
      { id: "t1", title: "Fix the | pipe", status: "running", updatedAt: NOW - 30, model: "claude-opus-5-5", provider: "claudeAgent" },
      { id: "t2", title: "Older", status: "completed", updatedAt: NOW - 7200 },
    ],
  },
];

const THREADS: LuaValue = {
  t1: {
    id: "t1",
    title: "Fix the | pipe",
    status: "running",
    activity: "Running npm test",
    messages: [
      { role: "user", text: "Please fix it", status: "completed", at: NOW - 60 },
      { role: "assistant", text: "Working on `a|b`\n```\ncode\n```", status: "running", at: NOW - 10 },
    ],
    questions: [
      {
        requestId: "r1",
        id: "q1",
        header: "Pick",
        question: "Which one?",
        options: [
          { label: "First", value: "first", description: "" },
          { label: "Second", value: "second", description: "the other" },
        ],
      },
    ],
  },
};

/** Start the addon like the client does: files, SavedVariables, ADDON_LOADED, PLAYER_LOGIN. */
function boot(options: { liveSlots?: number; savedVariables?: string; inbox?: string } = {}) {
  const vm = loadAddon(options);
  if (options.inbox) vm.runAddonFile(dataChunk(options.inbox, "inbox"), "T3Wow_Inbox/Data.lua");
  vm.run(`__stub.fire("ADDON_LOADED", "T3Wow")`);
  vm.run(`__stub.fire("PLAYER_LOGIN")`);
  return vm;
}

describe("Util", () => {
  const vm = loadAddon();
  const U = (expr: string) => vm.run(`local U = __ns.Util; return ${expr}`);

  it("escapes pipes so text cannot inject UI escape sequences", () => {
    assert.equal(U(`U.Escape("a|cffff0000b")`), "a||cffff0000b");
  });

  it("truncates on UTF-8 character boundaries", () => {
    // "é" is two bytes; cutting at 2 must not split it.
    assert.equal(U(`U.Truncate("aé€b", 2, "")`), "a");
    assert.equal(U(`U.Truncate("aé€b", 3, "")`), "aé");
    assert.equal(U(`U.Truncate("short", 10)`), "short");
  });

  it("maps answers to option values", () => {
    const q = `{ options = { { label = "First", value = "first" }, { label = "Second" } } }`;
    assert.equal(U(`U.AnswerFor(${q}, " 2 ")`), "Second");
    assert.equal(U(`U.AnswerFor(${q}, "first")`), "first");
    assert.equal(U(`U.AnswerFor(${q}, "something else")`), "something else");
  });

  it("maps multi-select answers to lists of option values", () => {
    const q = `{ multi = true, options = { { label = "First", value = "first" }, { label = "Second" }, { label = "Third" } } }`;
    assert.equal(U(`table.concat(U.AnswerFor(${q}, "1, 3"), "|")`), "first|Third");
    assert.equal(U(`table.concat(U.AnswerFor(${q}, "2 3"), "|")`), "Second|Third");
    assert.equal(U(`table.concat(U.AnswerFor(${q}, "second, first"), "|")`), "Second|first");
    assert.equal(U(`table.concat(U.AnswerFor(${q}, "first, and more"), "|")`), "first, and more");
  });

  it("derives thread titles from the first line", () => {
    assert.equal(U(`U.TitleFor("  ## Add login\\nmore")`), "Add login");
    assert.equal(U(`U.TitleFor("")`), "New thread from WoW");
  });

  it("formats relative times", () => {
    assert.equal(U(`U.Ago(1000, 990)`), "now");
    assert.equal(U(`U.Ago(10000, 10000 - 300)`), "5m");
    assert.equal(U(`U.Ago(100000, 100000 - 7200)`), "2h");
  });
});

describe("Markdown", () => {
  const vm = loadAddon();
  /** Parse and describe each block as `kind[:detail]=text`. */
  const blocks = (source: string) => {
    vm.run(`__src = ${toLua(source)}`);
    return (vm.run(`
      local out = {}
      for _, b in ipairs(__ns.Markdown.Parse(__src)) do
        local detail = b.level and (":" .. b.level) or b.lang and (":" .. b.lang) or ""
        if b.marker then detail = detail .. ":" .. b.marker end
        local text = b.text or (b.rows and (#b.rows .. " rows, " .. table.concat(b.align, ","))) or ""
        out[#out + 1] = b.kind .. detail .. "=" .. text
      end
      return table.concat(out, string.char(1))`) as string).split("\u0001");
  };
  const inline = (source: string) => vm.run(`return __ns.Markdown.Inline(${toLua(source)})`);

  it("splits a message into blocks", () => {
    const source = [
      "## Plan",
      "First line",
      "second line",
      "",
      "- one",
      "  continued",
      "  - nested",
      "1. first",
      "- [x] done",
      "> quoted",
      "> more",
      "---",
      "```lua",
      "local a = b | c",
      "```",
      "| Name | Count |",
      "|:-----|------:|",
      "| a    | 1     |",
    ].join("\n");
    assert.deepEqual(blocks(source), [
      "heading:2=Plan",
      "paragraph=First line\nsecond line",
      "item:0:•=one continued",
      "item:1:•=nested",
      "item:0:1.=first",
      "item:0:[x]=done",
      "quote=quoted\nmore",
      "rule=",
      "code:lua=local a = b || c",
      "table=2 rows, left,right",
    ]);
  });

  it("shows code that is still streaming in", () => {
    assert.deepEqual(blocks("Look:\n```\npartial"), ["paragraph=Look:", "code=partial"]);
  });

  it("formats inline Markdown as escape codes", () => {
    assert.equal(inline("**bold** and *it* and _it_"), "|cffffffffbold|r and |cffe6d7b0it|r and |cffe6d7b0it|r");
    assert.equal(inline("`a*b*|c` stays"), "|cff9cdcfea*b*||c|r stays");
    assert.equal(inline("see [docs](https://x.dev/a_b)."), "see |cff7fb2ff|Ht3url:https://x.dev/a_b|hdocs|h|r.");
    assert.equal(inline("at https://x.dev/p."), "at |cff7fb2ff|Ht3url:https://x.dev/p|hhttps://x.dev/p|h|r.");
    assert.equal(inline("snake_case_name and 2*3*4"), "snake_case_name and 2*3*4");
    assert.equal(inline("~~old~~ a|b"), "|cff808080old|r a||b");
  });

  it("lays out tables in aligned columns", () => {
    vm.run(`__t = __ns.Markdown.TableLines({ rows = { { "Name", "N" }, { "**é**", "10" } }, align = { "left", "right" } })`);
    assert.equal(vm.run(`return __t[1].text`), "Name    N");
    assert.equal(vm.run(`return __t[2].rule and __t[2].text`), "────  ───");
    assert.equal(vm.run(`return __t[3].text`), "é      10");
  });

  it("shrinks wide tables to fit, widest column first", () => {
    vm.run(`__t = __ns.Markdown.TableLines({ rows = { { "id", "${"description ".repeat(4).trim()}" } } }, 20)`);
    assert.equal(vm.run(`return __t[1].text`), "id   description de…");
    assert.equal(vm.run(`return __ns.Markdown.Width(__t[1].text)`), 20);
  });
});

describe("addon", () => {
  it("boots without bridge data and explains how to start the bridge", () => {
    const vm = boot();
    assert.match(vm.run(`return T3WowDB.instanceId`) as string, /^[0-9a-f]+-[0-9a-f]{16}$/);
    vm.run(`T3Wow:Toggle()`);
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /t3wow setup/);
    assert.equal(vm.run(`return select(1, T3Wow:BridgeState())`), "missing");
  });

  it("renders projects, threads and the conversation from a snapshot", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle()`);
    vm.run(`T3Wow:Select("p1", "t1")`);
    const text = vm.run(`return T3Wow:MessagesText()`) as string;
    assert.match(text, /Please fix it/);
    assert.match(text, /a\|\|b/, "pipes in messages are escaped");
    assert.match(text, /^code$/m, "code blocks get their own block");
    assert.match(text, /Question 1: Pick/);
    assert.match(text, /Working: .*Running npm test/);
    assert.equal(vm.run(`return T3Wow.stopButton:IsShown()`), true);
    assert.equal(vm.run(`return T3Wow.answerButton:IsShown()`), true);
    assert.equal(vm.run(`return T3Wow.rows[1].title:GetText()`), "Fix the || pipe");
  });

  it("queues a message and reloads the UI so it reaches SavedVariables", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    vm.run(`T3WowComposerEdit:SetText("  ship it  "); T3Wow:Submit()`);
    assert.equal(vm.run(`return __stub.reloads`), 1);
    assert.equal(vm.run(`return #T3WowDB.outbox`), 1);
    assert.equal(vm.run(`local c = T3WowDB.outbox[1]; return c.kind .. ":" .. c.threadId .. ":" .. c.text .. ":" .. c.mode`), "send:t1:ship it:auto");
    assert.equal(vm.run(`return T3WowComposerEdit:GetText()`), "");
  });

  it("answers a pending question by option number", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    vm.run(`T3WowComposerEdit:SetText("2"); T3Wow:Submit(true)`);
    assert.equal(vm.run(`local c = T3WowDB.outbox[1]; return c.kind .. ":" .. c.requestId .. ":" .. c.answers.q1`), "answer:r1:second");
  });

  it("answers a pending question by clicking an option", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /2\. Second/);
    vm.run(`T3Wow.messageView.pools.option.items[2]:Click()`);
    assert.equal(vm.run(`local c = T3WowDB.outbox[1]; return c.kind .. ":" .. c.requestId .. ":" .. c.answers.q1`), "answer:r1:second");
  });

  it("ticks multi-select options and sends them together", () => {
    const threads: LuaValue = {
      t1: {
        id: "t1",
        title: "Pick",
        status: "running",
        messages: [],
        questions: [
          {
            requestId: "r1",
            id: "q1",
            header: "Extras",
            question: "Which extras?",
            multi: true,
            options: [{ label: "Tests" }, { label: "Docs" }, { label: "Bench" }],
          },
        ],
      },
    };
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    assert.equal(vm.run(`return T3Wow.messageView.pools.action.items[1].enabled`), false, "nothing ticked yet");
    vm.run(`T3Wow.messageView.pools.option.items[3]:Click()`);
    vm.run(`T3Wow.messageView.pools.option.items[1]:Click()`);
    vm.run(`T3Wow.messageView.pools.option.items[1]:Click()`);
    vm.run(`T3Wow.messageView.pools.option.items[2]:Click()`);
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /\[ \] 1\. Tests\n\[x\] 2\. Docs\n\[x\] 3\. Bench/);
    assert.equal(vm.run(`return #T3WowDB.outbox`), 0);
    vm.run(`T3Wow.messageView.pools.action.items[1]:Click()`);
    assert.equal(vm.run(`return table.concat(T3WowDB.outbox[1].answers.q1, ",")`), "Docs,Bench");
  });

  it("answers every question of a request together", () => {
    const threads: LuaValue = {
      t1: {
        id: "t1",
        title: "Pick",
        status: "waiting",
        messages: [],
        questions: [
          { requestId: "r1", id: "q1", header: "Size", question: "Which size?", multi: false, options: [{ label: "Small" }, { label: "Large" }] },
          {
            requestId: "r1",
            id: "q2",
            header: "Extras",
            question: "Which extras?",
            multi: true,
            options: [{ label: "Tests" }, { label: "Docs" }, { label: "Bench" }],
          },
        ],
      },
    };
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /Question 1 of 2: Size/);
    vm.run(`T3WowComposerEdit:SetText("2"); T3Wow:Submit(true)`);
    assert.equal(vm.run(`return #T3WowDB.outbox`), 0, "waits for the request's last question");
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /Your answer: .*Large/);
    assert.equal(vm.run(`return T3Wow.question.id`), "q2");
    vm.run(`T3WowComposerEdit:SetText("1, 3"); T3Wow:Submit(true)`);
    assert.equal(vm.run(`return #T3WowDB.outbox`), 1);
    assert.equal(
      vm.run(`local a = T3WowDB.outbox[1].answers; return a.q1 .. ":" .. table.concat(a.q2, ",")`),
      "Large:Tests,Bench",
    );
    assert.equal(vm.run(`return next(T3WowDB.answers) == nil`), true);
    assert.doesNotMatch(vm.run(`return T3Wow:MessagesText()`) as string, /Question 1/, "answered questions hide");
    assert.equal(vm.run(`return T3Wow.answerButton:IsShown()`), false);
  });

  it("does not reload in combat", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`__stub.inCombat = true; T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    vm.run(`T3WowComposerEdit:SetText("hi"); T3Wow:Submit()`);
    assert.equal(vm.run(`return __stub.reloads`), 0);
    assert.equal(vm.run(`return #T3WowDB.outbox`), 1);
    assert.match(vm.run(`return T3Wow.outboxText:GetText()`) as string, /after combat/);
  });

  it("drops acknowledged commands and reports failures after a reload", () => {
    const saved = `T3WowDB = { instanceId = "inst", nextSeq = 3, outbox = {
      { seq = 1, kind = "send", threadId = "t1", text = "a" },
      { seq = 2, kind = "send", threadId = "t1", text = "b" } } }`;
    const acks = { inst: { seq: 1, results: [{ seq: 1, ok: false, message: "thread busy", threadId: "t1" }] } };
    const vm = boot({ savedVariables: saved, inbox: snapshot({ projects: PROJECTS, threads: THREADS, acks }) });
    assert.equal(vm.run(`return #T3WowDB.outbox`), 1);
    assert.equal(vm.run(`return T3WowDB.outbox[1].seq`), 2);
    assert.match(vm.run(`return __stub.prints[#__stub.prints]`) as string, /Not delivered:.*thread busy/);
    // Unsent commands reopen the window after the reload.
    assert.equal(vm.run(`return T3WowFrame:IsShown()`), true);
    vm.run(`T3Wow:Select("p1", "t1")`);
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /Not delivered: thread busy/);
    assert.match(vm.run(`return T3Wow.outboxText:GetText()`) as string, /delivering 1/);
  });

  it("opens a thread launched from the game once the bridge reports it", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", nil); T3Wow.newButton:Click()`);
    vm.run(`T3WowComposerEdit:SetText("Build a raid planner\\nwith details"); T3Wow:Submit()`);
    assert.equal(
      vm.run(`local c = T3WowDB.outbox[1]; return c.kind .. ":" .. c.projectId .. ":" .. c.title`),
      "launch:p1:Build a raid planner",
    );
    const instanceId = vm.run(`return T3WowDB.instanceId`) as string;
    const acks = { [instanceId]: { seq: 1, results: [{ seq: 1, ok: true, message: "Thread started", threadId: "t9" }] } };
    vm.runAddonFile(dataChunk(snapshot({ projects: PROJECTS, threads: THREADS, acks }, 2), "live"), "live.lua");
    assert.equal(vm.run(`return T3WowDB.focus.threadId`), "t9");
    assert.equal(vm.run(`return #T3WowDB.outbox`), 0);
  });

  it("pulls live updates by loading the next on-demand slot", () => {
    const vm = boot({ liveSlots: 2, inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    // Each slot addon's Data.lua carries the newest snapshot when it is loaded.
    const newer = snapshot({ projects: PROJECTS, threads: { t1: { id: "t1", title: "x", status: "completed", messages: [{ role: "assistant", text: "All done", status: "completed", at: NOW }] } } }, 5);
    (globalThis as { __live?: string }).__live = newer;
    vm.run(`__stub.onLoadAddOn = function(name) __stub.lastLoaded = name end`);
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    vm.run(`__stub.clock = __stub.clock + 60; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.lastLoaded`), "T3Wow_Live001");
    vm.runAddonFile(dataChunk(newer, "live"), "T3Wow_Live001/Data.lua");
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /All done/);
    vm.run(`__stub.clock = __stub.clock + 60; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.lastLoaded`), "T3Wow_Live002");
    vm.run(`__stub.clock = __stub.clock + 60; __stub.ticker()`);
    assert.equal(vm.run(`return T3Wow:LiveRemaining()`), 0);
    // An older snapshot never replaces a newer one.
    vm.runAddonFile(dataChunk(snapshot({ projects: [], threads: {} }, 1), "live"), "old.lua");
    assert.equal(vm.run(`return T3Wow.snapshot.seq`), 5);
  });

  it("runs the generated inbox chunk as valid Lua", () => {
    const vm = new LuaVM();
    vm.run(`received = nil; function T3Wow_Receive(s, source) received = s; receivedFrom = source end`);
    const tricky = 'quote " backslash \\ newline\n tab\t nul\u0000 pipe | unicode ✓ 🐉 ]]';
    vm.runAddonFile(dataChunk(snapshot({ projects: [{ id: "p", title: tricky, threads: [] }] }), "inbox"), "Data.lua");
    assert.equal(vm.run(`return received.projects[1].title`), tricky);
    assert.equal(vm.run(`return receivedFrom`), "inbox");
  });
});

describe("sync and models", () => {
  const MODELS: LuaValue = [
    {
      id: "claudeAgent",
      name: "Claude",
      models: [
        { id: "claude-opus-5-5", label: "Claude Opus 5.5", effortKey: "effort", defaultEffort: "medium",
          efforts: [{ id: "medium", label: "Medium" }, { id: "high", label: "High" }] },
        { id: "claude-haiku-4-5", label: "Claude Haiku 4.5", efforts: [] },
      ],
    },
  ];
  const withModels = (threads: LuaValue = THREADS) => snapshot({ projects: PROJECTS, threads, models: MODELS });

  it("refreshes without reloading when nothing needs the bridge", () => {
    const vm = boot({ liveSlots: 3, inbox: withModels() });
    vm.run(`__stub.onLoadAddOn = function(name) __stub.lastLoaded = name end`);
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1"); T3Wow.syncButton:Click()`);
    assert.equal(vm.run(`return __stub.reloads`), 0);
    assert.equal(vm.run(`return __stub.lastLoaded`), "T3Wow_Live001");
    // Shift-click always reloads.
    vm.run(`__stub.shift = true; T3Wow.syncButton:Click(); __stub.shift = false`);
    assert.equal(vm.run(`return __stub.reloads`), 1);
  });

  it("reloads to follow a thread the bridge only previews", () => {
    const partial = { t2: { id: "t2", title: "Older", status: "completed", partial: true,
      messages: [{ role: "assistant", text: "Latest only", status: "completed", at: NOW }] } };
    const vm = boot({ liveSlots: 3, inbox: withModels(partial) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t2")`);
    const text = vm.run(`return T3Wow:MessagesText()`) as string;
    assert.match(text, /Showing the latest messages/);
    assert.match(text, /Latest only/);
    vm.run(`T3Wow.syncButton:Click()`);
    assert.equal(vm.run(`return __stub.reloads`), 1);
  });

  it("switches a thread's model with the next message", () => {
    const vm = boot({ inbox: withModels() });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    assert.equal(vm.run(`return T3Wow.modelButton:IsShown()`), true);
    assert.equal(vm.run(`return T3Wow.modelButton:GetText()`), "Claude Opus 5.5");
    assert.equal(vm.run(`return T3Wow.effortButton:GetText()`), "Medium");
    // The menu lists provider, then models; pick Haiku.
    vm.run(`T3Wow:ShowModelMenu(T3Wow.modelButton)`);
    assert.match(vm.run(`return T3Wow.menu.rows[1].label:GetText()`) as string, /Claude/);
    vm.run(`T3Wow.menu.rows[3]:Click()`);
    assert.equal(vm.run(`return T3Wow.modelButton:GetText()`), "Claude Haiku 4.5");
    assert.equal(vm.run(`return T3Wow.effortButton:IsShown()`), false);
    vm.run(`T3WowComposerEdit:SetText("faster please"); T3Wow:Submit()`);
    assert.equal(vm.run(`local m = T3WowDB.outbox[1].model; return m.provider .. ":" .. m.model`), "claudeAgent:claude-haiku-4-5");
  });

  it("leaves the model alone unless one was picked", () => {
    const vm = boot({ inbox: withModels() });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    vm.run(`T3WowComposerEdit:SetText("same model"); T3Wow:Submit()`);
    assert.equal(vm.run(`return T3WowDB.outbox[1].model == nil`), true);
  });

  it("starts a new thread with the picked model and reasoning", () => {
    const vm = boot({ inbox: withModels() });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", nil); T3Wow.newButton:Click()`);
    vm.run(`T3Wow:ShowEffortMenu(T3Wow.effortButton); T3Wow.menu.rows[3]:Click()`);
    assert.equal(vm.run(`return T3Wow.effortButton:GetText()`), "High");
    vm.run(`T3WowComposerEdit:SetText("Plan a raid"); T3Wow:Submit()`);
    assert.equal(
      vm.run(`local m = T3WowDB.outbox[1].model; return m.provider .. ":" .. m.model .. ":" .. m.effortKey .. "=" .. m.effort`),
      "claudeAgent:claude-opus-5-5:effort=high",
    );
  });

  it("opens messages in a copy box", () => {
    const vm = boot({ inbox: withModels() });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1"); T3Wow:ShowCopy("Message", "a | b")`);
    assert.equal(vm.run(`return T3WowCopyTextEdit:GetText()`), "a || b");
    assert.equal(vm.run(`return T3WowCopy:IsShown()`), true);
  });
});

describe("notifications", () => {
  const finished = (status = "completed") =>
    snapshot({ projects: [{ id: "p1", title: "Game | Tools", threads: [
      { id: "t1", title: "Fix the | pipe", status, updatedAt: NOW - 5, model: "claude-opus-5-5", provider: "claudeAgent" },
      { id: "t2", title: "Older", status: "completed", updatedAt: NOW - 7200 },
    ] }], threads: THREADS }, 9);
  const prints = (vm: LuaVM) => vm.run(`return table.concat(__stub.prints, "\\n")`) as string;

  it("announces a thread that finished and marks it new", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    assert.doesNotMatch(prints(vm), /finished/, "the first snapshot only sets a baseline");
    vm.runAddonFile(dataChunk(finished(), "live"), "live.lua");
    assert.match(prints(vm), /Fix the \|\| pipe.*finished: done/);
    assert.equal(vm.run(`return __stub.sounds[1]`), 3081);
    assert.equal(vm.run(`return T3WowToast:IsShown()`), true);
    assert.equal(vm.run(`return T3WowToast.threadId`), "t1");
    assert.match(vm.run(`return T3WowMinimapButton.badge:GetText()`) as string, /1/);
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", nil)`);
    assert.match(vm.run(`return T3Wow.rows[1].sub:GetText()`) as string, /new/);
    // Clicking the toast opens the thread and clears the mark.
    vm.run(`T3WowToast:Click()`);
    assert.equal(vm.run(`return T3WowDB.focus.threadId`), "t1");
    assert.equal(vm.run(`return T3WowDB.unread.t1 == nil`), true);
    assert.equal(vm.run(`return T3WowMinimapButton.badge:GetText()`), "");
  });

  it("only rings when you are already looking at the thread", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    vm.runAddonFile(dataChunk(finished("failed"), "live"), "live.lua");
    assert.match(prints(vm), /finished: failed/);
    assert.equal(vm.run(`return T3WowToast == nil or not T3WowToast:IsShown()`), true);
    assert.equal(vm.run(`return T3WowDB.unread.t1 == nil`), true);
  });

  it("notices threads that finished during a reload", () => {
    const saved = `T3WowDB = { instanceId = "inst", statuses = { t1 = "running", t2 = "completed" } }`;
    const vm = boot({ savedVariables: saved, inbox: finished() });
    assert.match(prints(vm), /finished: done/);
    assert.equal(vm.run(`return T3WowDB.statuses.t1`), "completed");
  });

  it("asks for input when an agent waits", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.runAddonFile(dataChunk(finished("waiting"), "live"), "live.lua");
    assert.match(prints(vm), /needs your input/);
    assert.equal(vm.run(`return __stub.sounds[1]`), 8960);
  });

  it("announces a new question while the thread keeps running", () => {
    const quiet: LuaValue = { t1: { id: "t1", title: "Fix the | pipe", status: "running", messages: [], questions: [] } };
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: quiet }) });
    vm.runAddonFile(dataChunk(snapshot({ projects: PROJECTS, threads: THREADS }, 2), "live"), "live.lua");
    assert.match(prints(vm), /Fix the \|\| pipe.*asks: Which one\?/);
    assert.equal(vm.run(`return __stub.sounds[1]`), 8960);
    assert.equal(vm.run(`return T3WowToast:IsShown()`), true);
    vm.runAddonFile(dataChunk(snapshot({ projects: PROJECTS, threads: THREADS }, 3), "live"), "live.lua");
    assert.equal(vm.run(`return #__stub.sounds`), 1, "a question is announced once");
  });

  it("can be turned off, still marking threads new", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`SlashCmdList.T3WOW("notify off")`);
    vm.runAddonFile(dataChunk(finished(), "live"), "live.lua");
    assert.doesNotMatch(prints(vm), /finished/);
    assert.equal(vm.run(`return T3WowDB.unread.t1 ~= nil`), true);
  });

  it("keeps loading updates with the window closed while a thread runs", () => {
    const vm = boot({ liveSlots: 3, inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`__stub.onLoadAddOn = function(name) __stub.lastLoaded = name end`);
    // One update shortly after login, then every 10 seconds.
    vm.run(`__stub.clock = __stub.clock + 5; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.lastLoaded`), "T3Wow_Live001");
    vm.run(`__stub.clock = __stub.clock + 5; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.lastLoaded`), "T3Wow_Live001", "not before the background interval");
    vm.run(`__stub.clock = __stub.clock + 6; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.lastLoaded`), "T3Wow_Live002");
    // Nothing running: no background polling.
    vm.runAddonFile(dataChunk(finished(), "live"), "live.lua");
    vm.run(`__stub.lastLoaded = nil; __stub.clock = __stub.clock + 60; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.lastLoaded`), null);
  });

  it("hides the minimap button on request", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`SlashCmdList.T3WOW("minimap off")`);
    assert.equal(vm.run(`return T3WowMinimapButton:IsShown()`), false);
    assert.equal(vm.run(`return T3WowDB.minimap.hide`), true);
  });
});

describe("sending without a reload", () => {
  const withShots = (threads: LuaValue = THREADS) =>
    snapshot({ projects: PROJECTS, threads, bridge: { status: "ok", server: "env.example", canOperate: true, shots: true } });
  /** A client that can take screenshots; returns the VM with the window on thread t1. */
  const ready = (inbox = withShots()) => {
    const vm = boot({ liveSlots: 3, inbox });
    vm.run(`
      __stub.shots = 0
      function Screenshot() __stub.shots = __stub.shots + 1 end
      function GetPhysicalScreenSize() return 1920, 1080 end
      T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    return vm;
  };
  const send = (vm: LuaVM, text: string) => vm.run(`T3WowComposerEdit:SetText(${toLua(text)}); T3Wow:Submit()`);
  const takeShot = (vm: LuaVM, ok = true) => {
    vm.run(`T3WowShot.scripts.OnUpdate(T3WowShot)`);
    vm.run(`__stub.fire("${ok ? "SCREENSHOT_SUCCEEDED" : "SCREENSHOT_FAILED"}")`);
  };

  it("draws the outbox, screenshots it and keeps the UI", () => {
    const vm = ready();
    send(vm, "no reload please");
    assert.equal(vm.run(`return __stub.reloads`), 0);
    assert.equal(vm.run(`return T3WowShot:IsShown()`), true);
    assert.equal(vm.run(`return __stub.cvars.screenshotFormat`), "jpeg", "full-quality JPEG while shooting");
    // The clock row alternates black and white; the grid is pixel-scaled.
    assert.equal(vm.run(`return T3WowShot.cells[1].color[1] + T3WowShot.cells[2].color[1]`), 1);
    assert.equal(vm.run(`return T3WowShot.cells[1].color[1]`), 0);
    takeShot(vm);
    assert.equal(vm.run(`return __stub.shots`), 1);
    assert.equal(vm.run(`return T3WowShot:IsShown()`), false);
    assert.equal(vm.run(`return __stub.cvars.screenshotFormat`), "tga", "the player's settings come back");
    assert.equal(vm.run(`return #T3WowDB.outbox`), 1, "kept until the bridge acknowledges it");
    assert.match(vm.run(`return T3Wow.outboxText:GetText()`) as string, /delivering 1/);
    assert.equal(vm.run(`return T3Wow.composerHint:GetText()`), "Ctrl+Enter to send.");
  });

  it("puts the outbox, open thread and watch list in the screenshot", () => {
    const vm = ready();
    send(vm, "hello");
    vm.run(`__payload = T3Wow:ShotPayload()`);
    const payload = JSON.parse(vm.run(`return __payload`) as string);
    assert.equal(payload.outbox[0].text, "hello");
    assert.equal(payload.focus.threadId, "t1");
    assert.equal(typeof payload.watch.t1, "number");
  });

  it("works in combat, where a reload cannot", () => {
    const vm = ready();
    vm.run(`__stub.inCombat = true`);
    send(vm, "mid-fight");
    takeShot(vm);
    assert.equal(vm.run(`return __stub.shots`), 1);
    assert.doesNotMatch(vm.run(`return T3Wow.outboxText:GetText()`) as string, /after combat/);
  });

  it("falls back to a reload when the bridge does not answer", () => {
    const vm = ready();
    send(vm, "anyone there?");
    takeShot(vm);
    vm.run(`__stub.clock = __stub.clock + 10; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.reloads`), 0);
    vm.run(`__stub.clock = __stub.clock + 15; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.reloads`), 1);
    assert.match(vm.run(`return __stub.prints[#__stub.prints]`) as string, /reload/);
  });

  it("falls back to a reload when the screenshot fails", () => {
    const vm = ready();
    send(vm, "hi");
    takeShot(vm, false);
    assert.equal(vm.run(`return __stub.reloads`), 1);
    assert.equal(vm.run(`return T3Wow:CanFastSend()`), false, "not retried this session");
  });

  it("drops the command once the bridge acknowledges it", () => {
    const vm = ready();
    send(vm, "hi");
    takeShot(vm);
    const instanceId = vm.run(`return T3WowDB.instanceId`) as string;
    const acked = snapshot({ projects: PROJECTS, threads: THREADS, bridge: { status: "ok", canOperate: true, shots: true },
      acks: { [instanceId]: { seq: 1, results: [{ seq: 1, ok: true, message: "Sent", threadId: "t1" }] } } }, 3);
    vm.runAddonFile(dataChunk(acked, "live"), "live.lua");
    assert.equal(vm.run(`return #T3WowDB.outbox`), 0);
    vm.run(`__stub.clock = __stub.clock + 60; __stub.ticker()`);
    assert.equal(vm.run(`return __stub.reloads`), 0);
  });

  it("asks the bridge to follow a previewed thread by screenshot too", () => {
    const partial = { t2: { id: "t2", title: "Older", status: "completed", partial: true, messages: [] } };
    const vm = ready(withShots(partial));
    vm.run(`T3Wow:Select("p1", "t2"); T3Wow.syncButton:Click()`);
    assert.equal(vm.run(`return __stub.reloads`), 0);
    assert.equal(vm.run(`return T3WowShot:IsShown()`), true);
  });

  it("can be turned off", () => {
    const vm = ready();
    vm.run(`SlashCmdList.T3WOW("fastsend off")`);
    send(vm, "reload me");
    assert.equal(vm.run(`return __stub.reloads`), 1);
  });
});

describe("new thread project", () => {
  const TWO_PROJECTS: LuaValue = [
    ...(PROJECTS as LuaValue[]),
    { id: "p2", title: "Raid tools", threads: [] },
  ];

  it("starts from the project list and picks the project in the composer", () => {
    const vm = boot({ inbox: snapshot({ projects: TWO_PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select(nil, nil)`);
    assert.equal(vm.run(`return T3Wow.newButton:IsShown()`), true, "+ New on the project list too");
    vm.run(`T3Wow.newButton:Click()`);
    assert.equal(vm.run(`return T3Wow.projectButton:IsShown()`), true);
    assert.equal(vm.run(`return T3Wow.projectButton:GetText()`), "Game || Tools", "defaults to the most recent project");
    vm.run(`T3WowComposerEdit:SetText("Plan the raid")`);
    // The menu lists every project; pick the second.
    vm.run(`T3Wow:ShowProjectMenu(T3Wow.projectButton)`);
    assert.match(vm.run(`return T3Wow.menu.rows[3].label:GetText()`) as string, /Raid tools/);
    vm.run(`T3Wow.menu.rows[3]:Click()`);
    assert.equal(vm.run(`return T3Wow.projectButton:GetText()`), "Raid tools");
    assert.match(vm.run(`return T3Wow.threadMeta:GetText()`) as string, /in Raid tools/);
    assert.equal(vm.run(`return T3WowComposerEdit:GetText()`), "Plan the raid", "the draft moves along");
    vm.run(`T3Wow:Submit()`);
    assert.equal(vm.run(`local c = T3WowDB.outbox[1]; return c.kind .. ":" .. c.projectId`), "launch:p2");
    // Once started, the new thread opens in its project.
    const instanceId = vm.run(`return T3WowDB.instanceId`) as string;
    const acks = { [instanceId]: { seq: 1, results: [{ seq: 1, ok: true, message: "Thread started", threadId: "t9" }] } };
    vm.runAddonFile(dataChunk(snapshot({ projects: TWO_PROJECTS, threads: THREADS, acks }, 2), "live"), "live.lua");
    assert.equal(vm.run(`return T3WowDB.focus.projectId .. ":" .. T3WowDB.focus.threadId`), "p2:t9");
  });

  it("keeps the open project when starting inside one", () => {
    const vm = boot({ inbox: snapshot({ projects: TWO_PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p2", nil); T3Wow.newButton:Click()`);
    assert.equal(vm.run(`return T3Wow.projectButton:GetText()`), "Raid tools");
    assert.equal(vm.run(`return T3Wow.modeButton:IsShown()`), false);
  });
});

describe("compact", () => {
  it("queues /compact for the open thread", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    assert.equal(vm.run(`return T3Wow.compactButton:IsShown()`), true);
    vm.run(`T3Wow.compactButton:Click()`);
    assert.equal(vm.run(`local c = T3WowDB.outbox[1]; return c.kind .. ":" .. c.text .. ":" .. c.mode`), "send:/compact:queue");
    assert.match(vm.run(`return T3Wow:MessagesText()`) as string, /Compact requested/);
  });

  it("shows a compaction as a note, not a message", () => {
    const threads = { t1: { id: "t1", title: "x", status: "completed", messages: [
      { role: "user", text: "/compact", status: "completed", at: NOW },
      { role: "assistant", text: "Earlier work summarized.", status: "completed", at: NOW },
    ] } };
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    const text = vm.run(`return T3Wow:MessagesText()`) as string;
    assert.match(text, /Conversation compacted/);
    assert.doesNotMatch(text, /^\/compact$/m);
  });

  it("is hidden for read-only sign-ins", () => {
    const vm = boot({ inbox: snapshot({ projects: PROJECTS, threads: THREADS, bridge: { status: "ok", canOperate: false } }) });
    vm.run(`T3Wow:Toggle(); T3Wow:Select("p1", "t1")`);
    assert.equal(vm.run(`return T3Wow.compactButton:IsShown()`), false);
  });
});
