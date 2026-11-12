# klark feed: live test plan

Manual tests with real agents (Claude Code, Codex) for `klark feed` (ADR 0015).
Unit and integration tests cover the mechanics (`tests/feed_sessions.rs`); this
plan checks that real agents get the right messages at the right time, in the
situations people actually use feeds for.

Run everything in a named test session so the everyday servers are untouched:

```sh
klark --session feedtest          # a new server from the installed binary
```

Record each run in the table at the end: date, binary version, agent kind,
pass/fail, notes.

## Rules for every test

- Outbound actions (email, WhatsApp, posts) go through `notify send … --dry-run`
  unless the owner says to send for real. A test that "sends" proves the agent
  chose to send and composed the right message; the transport is tested once,
  separately (T9).
- Untrusted text (mail, web pages) is fed only to an agent told to report, never
  to one that acts without approval (ADR 0015, injection).
- After each test: `klark feed list` is empty again (`klark feed stop <id>`), and
  `pgrep -fl 'feed _worker'` shows nothing for this session.

## T1. Web page changed (poll + unified diff)

Setup: a page you control.

```sh
mkdir -p /tmp/kft/web && echo '<h1>status: ok</h1>' > /tmp/kft/web/index.html
python3 -m http.server 8765 -d /tmp/kft/web &
klark feed add watcher "The status page changed. Say in one line what changed." \
  --poll 'curl -fsS http://127.0.0.1:8765/' --diff unified --interval 10
```

| step | expect |
|---|---|
| wait 30 s, page unchanged | no message |
| `echo '<h1>status: DEGRADED</h1>' > /tmp/kft/web/index.html` | one message within ~10 s + gap, a diff showing ok → DEGRADED; the agent names the change |
| change it twice within 5 s | one message with both changes, or two in order, never out of order |
| stop the server (`kill %1`) | a `failed` notice once, not one per poll |
| start it again | a `recovered` notice; the next change is sent normally |
| page returns an empty body, then content again | the content comes back as a change (bug fixed 2026-11-12) |

## T2. Web check with refresh of a real site

`--poll 'curl -fsS https://example.com/' --diff unified --interval 300`. Expect
no message for an unchanged page over an hour (dynamic parts such as timestamps
would show up as noise; if they do, the fix is a `| grep`/`jq` in the command,
not in klark). Check `klark feed show <id>` shows last run and next run.

## T3. Email check-in (inbox poll)

With a mail CLI (`himalaya envelope list`) or, without one, a fake inbox:

```sh
mkdir -p /tmp/kft/inbox
klark feed add mail "New mail arrived. For each: sender, subject, and whether it needs an answer today." \
  --poll 'ls -1 /tmp/kft/inbox' --interval 10
echo "From: boss / Subject: invoice overdue" > /tmp/kft/inbox/001.txt
```

| step | expect |
|---|---|
| first run | existing mail is NOT sent (a new feed starts from now) |
| add one mail | one message with exactly that line |
| add three mails at once | one message, three lines |
| delete a mail | nothing (line diff sends only added lines) |
| the agent is busy (give it a long task first) | the mail waits; one combined message when it goes idle |
| `--filter "jevx is 'Does this need a person today?'"` | dull mail (newsletters) costs no agent turn; urgent mail goes through |

## T4. Email check-in that leads to an action (send an email)

Feed as T3, instruction: "If a mail asks for a reply today, draft a reply and
send it with `notify send --purpose outreach --to <sender> --body-file <f> --dry-run`.
Otherwise do nothing."

Expect: the agent runs `notify send … --dry-run` only for the urgent mail, with a
sensible body; nothing for the others. Verify with the dry-run output in the
agent's pane (`klark agent read mail --lines 60`).

## T5. Log watch → WhatsApp / group alert

```sh
klark feed add triage "An ERROR was logged. If it is new and serious, alert the owner with notify send --purpose owner-alert --body-file <f> --dry-run; otherwise do nothing." \
  --stream "tail -n0 -F /tmp/kft/app.log | grep --line-buffered ERROR"
```

| step | expect |
|---|---|
| `echo "INFO ok" >> /tmp/kft/app.log` | nothing |
| `echo "ERROR db down" >> /tmp/kft/app.log` | one message; the agent runs the dry-run alert |
| 50 ERROR lines in one second | one message (batched), cut at 4 KiB with a `klark feed batch` hint; one alert, not 50 |
| `mv app.log app.log.1; touch app.log; echo "ERROR again" >> app.log` | the new line arrives (`tail -F` follows rotation) |

The same with `--follow /tmp/kft/app.log` instead of `--stream`.

## T6. Two-tier: cheap triage agent wakes the expensive one

A small agent (`triage`) subscribed to the log; instruction tells it to run
`klark msg send main "<what and why>"` only when action is needed. Expect `main`
gets one `[klark msg from …]` message per real incident and none for noise.

## T7. Several agents, one feed

`klark feed add a "…" --stream X` then the same command for `b` with another
instruction: one worker process (`pgrep -fl 'feed _worker'`), each agent gets its
own instruction. `klark feed leave` from `a`'s pane: `b` keeps getting messages.

## T8. Restart and reboot

- `klark --session feedtest server stop`, append to the followed log, start the
  session again: the lines written while it was down arrive once (capped at
  64 KiB with a "skipped" note).
- A stream feed restarts its command from scratch.
- Reboot the machine (once per release): feeds come back on first server start.

## T9. Real send (owner only)

Once per release, with the owner present: T4 and T5 without `--dry-run`, to the
owner's own address / WhatsApp. Confirms the transport, not klark.

## T10. Limits

- More than 30 messages in an hour: the feed stops itself and the agent is told
  why once; `klark feed start <id>` brings it back.
- `--message-gap 5`: messages never closer than 5 s.
- The settings tab (Feeds) changes the defaults; new feeds use them.

## T11. Codex and other agents

Repeat T1, T3 and T5 with a Codex agent: the message is submitted (not left in
the input box), and a draft the person was typing is preserved.

## Results

| date | version | test | agent | result | notes |
|---|---|---|---|---|---|
| 2026-11-12 | 0.10.2 (93e0e024) | T1 web change | claude | pass | change arrived ~10 s after edit; agent named it. Diff showed `+` before `-`: fixed (removed lines first) |
| 2026-11-12 | 0.10.2 | T1 outage / recovery | claude | pass | one `failed` notice, one `recovered` notice |
| 2026-11-12 | 0.10.2 | T3+T4 inbox → reply | claude | pass | 2 mails in one message; urgent one drafted + `notify --dry-run`, newsletter ignored |
| 2026-11-12 | 0.10.2 | T5 log flood → WhatsApp | claude | pass | INFO filtered; 60 ERRORs in one message; one dry-run alert |
