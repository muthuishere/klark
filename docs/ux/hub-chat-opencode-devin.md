# Hub chat for OpenCode and Devin

2026-11-27, branch `hubux`. What changed in how the hub shows OpenCode and Devin panes, why, and what is kept or removed.

## The problem

The hub showed both agents as a raw terminal dump in one grey block:

- **OpenCode:** its full-screen TUI went through the generic screen reader. The right sidebar (title, `Context 342,157 tokens`, `MCP • Connected`) was mixed into every line. The `┃` bars, the composer, `Build · GPT-5.6 Luna · 1m 13s` and the status line `… ctrl+p commands` all showed as chat. An open dialog (the model picker) was drawn over the transcript and read as text too.
- **Devin:** a new session showed the shell lines, the braille logo, `Devin CLI v3000.15.14`, the plan line, the placeholder `❭ Ask Devin to build features…` and the model footer. The Devin screen reader expected `•`/`└` markers from an invented fixture. Devin 3000 draws `❭` prompts, `✓` tool steps and unmarked reply text, so no turns were found.
- **Sending from the hub to Devin** typed the text but did not submit it. The hub pressed Enter immediately, and Devin swallowed it as part of a paste still in progress.

## The approach: read the agent's own store, and use the screen only as a fallback

Both agents keep the conversation in SQLite, so the machine (`klark transcript`, run by `klark expose`) reads it the way it already reads Claude Code's JSONL. The hub gets structured items (`text`, `thinking`, `tool_use`, `tool_result`) and shows them like Claude: markdown replies, collapsed "Ran 2 commands · thought" groups and tool cards.

| agent | store | conversation | cursor |
|---|---|---|---|
| OpenCode 2.x | `~/.local/share/opencode/opencode.db` (`XDG_DATA_HOME`) | `session_message` rows of one session (`session_v2`), by `seq` | `seq` |
| Devin 3000 | `~/.local/share/devin/cli/sessions.db` | `message_nodes` main chain from `sessions.main_chain_id` (existing) | `node_id` |

Session: the id the klark plugin reported for the pane (`agent_session`); otherwise the newest session in the pane's folder. For OpenCode that is a top-level session that is not archived (subagent sessions have a `parent_id`). The store is opened read-only. A missing, locked or empty store falls back to the screen.

Sources used:

- The OpenCode repo's generated client types (`packages/client/src/promise/generated/types.ts`, `SessionMessage*`).
- The live store's schema.
- Notes from the devinfinder agent, which decompiled Devin CLI 3000.15.14.
- Community viewers: opencode-session-viewer, opencode-viewer, opencode-observability and devin-session-dashboard. All of them read the store or an export, none scrape the screen.

## What is shown and what is removed

### OpenCode (structured)

| entry (`session_message.type`) | shown as | why |
|---|---|---|
| `user` `{text, files}` | the person's message, attached file names as `[name]` | |
| `assistant` `text` part | reply, rendered as markdown | OpenCode writes plain GFM, so it needs no reshaping |
| `assistant` `reasoning` part | thinking (collapsed) | |
| `assistant` `tool` part | tool card: name and input; result when `completed` (ok) or `error` (failed, error message as text) | a `running` tool shows the call only |
| `shell` (`!cmd` typed in OpenCode) | the person's `!cmd`, then its output as a tool result (ok = exit 0) | |
| `idle` with `outcome: failed` | `Error: <message>` (for example "Insufficient balance") | what the TUI shows in red. Shown once: the failed assistant entry carries the same error and is not repeated |
| `compaction` completed | "_Conversation compacted._" | marks where older context was summarised |
| `system`, `synthetic`, `agent-switched`, `model-switched`, `location-switched`, `idle` succeeded | removed | injected context and bookkeeping, not conversation |

Tool inputs longer than 1 KB keep their keys, and each long value is cut. So a `write` still shows its `path` in the card summary and does not turn into a JSON blob. Tool results are capped at 4 KB as before.

### Devin (structured, existing reader, two fixes)

- **Only what the person typed is a user message:** `metadata.is_user_input` is true. Devin also stores injected context as `user` rows without that flag, such as compaction summaries ("Conversation to summarize: …") and skill lists. 14 such rows in the live store used to show as if the person had written them.
- **Command output:** the `Output from command in shell <id>:` header is removed, and `Exit code: 0` is dropped. A non-zero exit code is kept.

### Screen fallback (only when there is no store)

**Devin 3000** (`hub-go/chat/screen_tui.go`, `devin3`):

- Removed: everything before the first real `❭` prompt: shell lines, the logo, `Devin CLI` / `v…` and `Free plan, use /upgrade…`. A new session therefore shows an empty chat, not the banner.
- Removed: the composer and footer. That is the `────` rules (including the one with the session title), the `⏺ /cwd` line, the idle placeholders ("Ask Devin to build features…", "Guide Devin while it works", …), `SWE-2 Flash Medium … ctrl+v to paste image`, `Context: 26k / 262k tokens`, `esc twice to interrupt` and the mode names.
- Kept: `❭ …` as the person's message, with its wrapped lines joined. `✓ Listed ./` and `✗ …` become a tool card. The reply text is kept, and box-drawn tables become markdown tables.

**OpenCode** (`opencode`):

- Removed: the right sidebar, cut at the column of its section labels (Context, MCP, LSP, Modified Files, Todo).
- Removed: the composer (the `┃` block over `╹▀▀▀`) and the status line (`… ctrl+p commands`).
- Removed: the per-turn `Build · <model> · 1m 13s · 3.6s ttft` line.
- Kept: `┃` blocks become the person's message, joined. `+ 6 commands, 4 thoughts` becomes one step row. The reply text is reshaped into paragraphs, lists and code as for other screens.
- A dialog drawn over the transcript (model picker, command palette) cannot be read reliably from a screen. That is one more reason the store comes first.

### Markdown: each agent's spec, and what the hub does

Sources:
- OpenCode 2.4.1 source (`~/Downloads/opencode-dev`), files below.
- devinfinder, which decompiled Devin CLI 3000.15.14 and inspected the installed OpenCode 2.3.0 binary.
- Checks against the live stores.

**What each side stores and renders**

| | OpenCode | Devin | the hub |
|---|---|---|---|
| stored text | the model's raw markdown, unchanged | the model's raw markdown, unchanged (`chat_message.content`) | renders the stored markdown, not the screen |
| spec | CommonMark + GFM (tables, strikethrough, task lists, autolinks). Web: `marked` 18 plus its streaming parser `patchmark` (micromark + `micromark-extension-gfm`, `gfm: true, math: true, heal: true, renderHtml: false`, `packages/session-ui/src/components/markdown-stream.ts`). TUI: `@opentui/core` `MarkdownRenderable` (`<markdown … conceal>` in `packages/tui/src/routes/session/message-parts.tsx`) | CommonMark → HTML in its `barista` TUI (`rendered_commits.rendered_html`) | goldmark with GFM: tables, strikethrough, task lists, linkify (`hub-go/chat/markdown.go`) |
| raw HTML | not rendered | not rendered | not rendered |
| links | TUI: clickable (OSC 8). Web: `target=_blank` | text only (no `<a>` in its HTML) | http, https and mailto links only. File paths open in the hub's file viewer |
| images | preview dialog | text only | alt text, never loaded |
| code | tree-sitter (TUI) or Shiki (web) highlighting | code block | code segment with a language label and a copy button |
| math, mermaid | KaTeX, mermaid | none | **shown as a code block** (not rendered) |

**Rewrites before rendering (OpenCode, copied from its TUI)**

| OpenCode rule (source) | in klark |
|---|---|
| reasoning: drop OpenRouter's `[REDACTED]` placeholder, trim (`reasoningContent`, message-parts.tsx) | `opencode.rs` |
| text parts are trimmed (message-parts.tsx) | `opencode.rs` |
| tool names: `bash`→`shell`, `task`→`subagent`, `apply_patch`→`patch` (`canonicalToolName`, `packages/tui/src/util/tool-display.ts`) | `opencode.rs` |
| a patch is shown by the files it changes, not as patch text | `opencode.rs` gives the first changed file as `path`, so the card summary names it |
| `<system-reminder>…</system-reminder>` blocks are hidden | already done by the hub for every agent (`hub-go/chat/clean.go` `cleanStructured`). 60 OpenCode rows carry them |
| reasoning is collapsible thinking, tools are status cards, not markdown | the hub's thinking rows and tool cards |

**Rewrites before rendering (Devin, from devinfinder and the live store)**

| Devin markup | in klark |
|---|---|
| `<ref_file file="P" />` and `<ref_snippet file="P" lines="A-B" />` citation tags in replies (10 rows in the live store) | `P`, or `P:A` (lines A-B), as inline code. The hub links it, and `P:A` opens the file at line A (`#LA`) |
| `<system_info>`, `<system_guidance>`, `<user-prompt-submit-hook>` blocks in user rows | removed. In the live store they only appear in injected rows, which the `is_user_input` filter already hides |
| `<truncation_notice>Full output written to: P</truncation_notice>` at the end of tool output (17 rows) | one `[full output: P]` line |
| `Output from command in shell <id>:` / `Exit code: 0` | header removed; a zero exit code dropped, a non-zero one kept |
| `@path` mentions | plain text, kept as written (not special in Devin's markdown) |

`rendered_commits.rendered_html` (Devin's own HTML) is not used. It only covers finished rows, has no links or images, and needs Devin's CSS classes mapped. The stored markdown gives the same structure.

**The screen fallback** cannot recover what the TUI already rendered:
- Headings become plain lines.
- Links lose their URL.
- Code loses its fence and language.

So the screen readers only rebuild what can be seen: box tables become pipe tables, `•` lines become lists, column-shaped runs become code blocks. This is why the store always comes first.

## Sending from the hub

`POST …/send` (and `/input` with `enter`) now waits between the text and Enter. It uses the machine's own `agent.prompt` rule: 300 ms plus 1 ms per 16 bytes, at most 1.5 s (`SubmitDelay`, same formula as `src/app/api/agents.rs` `submit_delay`). Routing through `agent.prompt` itself would need it on the machine call allow-list, and therefore an update on every machine. Checked live: a hub send to the Devin pane now submits without a second Enter.

## Files

| file | change |
|---|---|
| `src/transcript_cmd/opencode.rs` | new: OpenCode store reader, with 4 tests (one is an opt-in smoke run against the real store) |
| `src/transcript_cmd.rs` | `opencode` panes use it; same fallback rules as Devin |
| `src/transcript_cmd/devin.rs` | `is_user_input` filter, citation tags, harness blocks, truncation notices, command-output cleanup, tests |
| `hub-go/chat/screen_tui.go`, `screen.go` | Devin 3000 and OpenCode screen readers, reader detection |
| `hub-go/chat/screen_tui_test.go`, `testdata/devin3000*.screen.txt`, `testdata/opencode*.screen.txt` | tests on synthetic screens in the real layouts (no private content) |
| `hub-go/internal/app/api.go`, `submit_test.go` | Enter delay |
| `hub-go/ui/packages/hub-ui/src/panes.ts`, `ui/src/pages/PanePage.tsx`, `ui/test/agent-key.test.ts` | OpenCode badge and reader class (no forced monospace) |
| `hub-go/chat/render_real_test.go`, `hub-go/scripts/capture-render-fixtures.sh`, `render-check.sh`, `render-check.js` | rendering tests on real logs, Go and browser |

## Verification

- `cargo test --bin klark transcript_`: 18 passed. `KLARK_OPENCODE_SMOKE=1` read 5 recent real sessions (up to 199 items each, no errors).
- `go test ./...` in `hub-go`: everything passes except `TestPaneStreamIsLazyAndCountsViewers`, which fails the same way on unchanged `HEAD` (a known timing test).
- End to end: a hub built from this branch on :7471, with `klark expose` from this branch. The calendar-lane OpenCode pane (198 items), the devinfinder OpenCode pane (125) and the Devin pane (9) all came back structured (`reader: db`). In the browser, the Devin chat rendered its heading, a real table, code blocks and collapsed tool/thought groups.

## Rendering tests (every agent, real logs, one browser)

Two layers, the same rules in both:

1. **Go, the hub's pipeline** (`hub-go/chat/render_real_test.go`). It runs real transcripts (what `klark transcript` hands the hub) through `ToItem` / `Transform` + `ScreenItems` / `Linkify`, and checks every row:
   - no harness markup in prose (`<system-reminder>`, Devin's `<ref_*>` / `<truncation_notice>` / `<system_info>`, the hub note, `[REDACTED]`). Code spans and quotes are skipped, because agents also *talk about* these tags;
   - no TUI chrome in rows read from a screen;
   - safe HTML: no script, iframe or loaded image, links only http(s)/mailto/file;
   - markdown fidelity: a GFM table in the source renders as a `<table>`, a heading as an `<hN>`, a fence as a code segment, and no raw `## ` / `|---|` / `**x**` is left in prose.

   The transcripts are private, so they stay outside the repo:
   ```
   hub-go/scripts/capture-render-fixtures.sh <dir> <session/pane>...   # one file per pane
   KLARK_RENDER_FIXTURES=<dir> go test ./chat -run TestRenderRealTranscripts -v
   ```
2. **Browser, the real UI** (`hub-go/scripts/render-check.sh` + `render-check.js`). It uses one chrome-agent tab and opens each pane's chat page in turn. The chat is windowed, so the check scrolls it top to bottom and checks every row once. It applies the same rules to the DOM, plus:
   - content wider than the chat column,
   - empty bubbles,
   - for agents read from their own log, prose must use the theme's font: the reader must not force monospace.
   ```
   HUB=http://127.0.0.1:7470 HUB_USER=admin HUB_PASSWORD=… hub-go/scripts/render-check.sh <session/pane>...
   ```

Run on 2026-11-27, against 11 live panes covering all six agents on this machine (Claude ×3, OpenCode ×3, Devin, Codex, Grok, Agy ×2):

| | result |
|---|---|
| Go | 11/11 pass, 1,689 rows. A planted leak (`<ref_file>`, the hub note) fails as it should |
| browser | 11/11 pass in one tab. Claude: 131 + 100 + 4 rows, 6 tables, 224 code blocks. OpenCode: 13 + 7 + 21 turns, 271 code blocks, 213 tool groups. Devin: a table, 2 headings, 6 code blocks. An injected monospace rule fails as it should |

**UI fix found by the browser test.** The UI knew only `claude`, `devin` and `codex`. OpenCode panes got the "other" badge and the `reader-generic` page class, which sets **monospace** on assistant messages (`packages/hub-ui/style.css`). So even structured OpenCode markdown looked like terminal text, which is what the original screenshot showed. Now:
- `agentKey` knows `opencode` (badge `◇`);
- `PanePage` gives OpenCode its own `reader-opencode` class (proportional, like Claude);
- `reader-generic` stays for agents still read from the screen (Grok, Agy, other).

Test: `ui/test/agent-key.test.ts`.

## Not done

- The machine needs this klark build for the store readers (`klark transcript` runs on the machine). The hub-side changes (screen readers, Enter delay) work with any machine.
- Devin's `rendered_commits.rendered_html` (Devin's own per-row HTML) is not used. The hub renders the markdown itself, so all agents look the same.
- OpenCode 1.x (JSON files under `storage/`) and its older `session`/`message`/`part` tables are not read. Those panes use the screen reader.
