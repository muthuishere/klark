# Klark: feed recipes

Part of the klark skill; `feed.md` explains how feeds work and its rules apply.

Pick the mode by the source:
- something that prints new lines as they happen → `--stream`
- a file that grows → `--follow <file>`
- something you can only ask "what is there now?" → `--poll`, with
  `--diff lines` (default) for a list where each new line is news, or
  `--diff unified` for one document whose changes are news.

For a list, print one stable line per item (id + title), so an item that did
not change does not look new. Never include a changing field (age, "updated 3
minutes ago", a count) in such a line.

## GitHub (`gh`)

```bash
# PRs waiting for my review
klark feed add reviewer "A PR now needs my review. Summarise what it changes and what to look at first." \
  --poll "gh pr list --search 'review-requested:@me' --json number,title,url --jq '.[] | \"\(.number) \(.title) \(.url)\"'" --interval 300

# New PRs in this repo
klark feed add reviewer "New PR opened. One line: what it does and whether it touches risky code." \
  --poll "gh pr list --state open --json number,title,author --jq '.[] | \"#\(.number) \(.title) (\(.author.login))\"'" --interval 300

# CI runs that failed on main
klark feed add fixer "A CI run failed. Read its log (gh run view <id> --log-failed) and tell me the cause; change nothing." \
  --poll "gh run list --branch main --status failure --limit 10 --json databaseId,name,headSha --jq '.[] | \"\(.databaseId) \(.name) \(.headSha[0:7])\"'" --interval 120

# New comments or reviews on my PR 123
klark feed add coder "New review activity on PR 123. Address what is asked, or tell me what is unclear." \
  --poll "gh pr view 123 --json comments,reviews --jq '(.comments[], .reviews[]) | \"\(.author.login): \(.body | gsub(\"\\n\"; \" \"))\"'" --interval 120

# Checks of PR 123 changed state
klark feed add coder "A check on PR 123 changed. If one failed, find out why." \
  --poll "gh pr checks 123 --json name,state --jq '.[] | \"\(.name) \(.state)\"'" --diff unified --interval 60

# Issues assigned to me
klark feed add triage "A new issue is assigned to me. Say how urgent it is and where in the code it lives." \
  --poll "gh issue list --assignee @me --json number,title --jq '.[] | \"#\(.number) \(.title)\"'" --interval 600
```

## Files and folders

```bash
# Lines appended to a log (rotation and truncation followed)
klark feed add watcher "New lines in the app log; report anything unusual." --follow /var/log/app.log

# Only the errors
klark feed add watcher "An ERROR was logged. Find the cause in the code; change nothing." \
  --stream "tail -n0 -F app.log | grep --line-buffered -E 'ERROR|FATAL|panic'"

# A config file changed
klark feed add reviewer "nginx.conf changed. Check the change is safe." --poll "cat /etc/nginx/nginx.conf" --diff unified

# New files dropped into a folder
klark feed add sorter "New files arrived in ~/Downloads/inbox. Say what each one is." --poll "ls -1 ~/Downloads/inbox" --interval 60

# Files changed in a working tree
klark feed add reviewer "Files changed in the repo. Say if anything looks unintended." --poll "git status --porcelain" --diff unified --interval 60
```

## Git

```bash
# New commits on main (after a fetch)
klark feed add reviewer "New commits on origin/main. Summarise them and flag risky ones." \
  --poll "git fetch -q && git log --oneline -20 origin/main" --interval 300
```

## Web and APIs

```bash
# A page or an endpoint changed
klark feed add watcher "The status page changed; say what broke." \
  --poll "curl -fsS https://status.example.com/api/v2/summary.json | jq -r '.components[] | \"\(.name) \(.status)\"'" --diff unified --interval 120

# With a token (single quotes: the feed's shell expands it, nothing is saved)
klark feed add watcher "The deploy API reports a new deploy." \
  --poll 'curl -fsS -H "Authorization: Bearer $DEPLOY_TOKEN" https://deploy.example.com/api/deploys' --diff unified --interval 120
```

When a whole page changes on every load (timestamps, tokens), pull out just
the part that matters with `jq`, `grep` or `htmlq` in the command; klark does
not filter.

## Mail

```bash
klark feed add mail "New mail. For each: sender, subject, and whether it needs an answer today." \
  --poll "himalaya envelope list --page-size 20 -o json | jq -r '.[] | \"\(.id) \(.from.addr) \(.subject)\"'" --interval 300
```

## Servers and containers

```bash
# Pods that are not running
klark feed add ops "A pod is not running. Find out why from its events and logs; change nothing." \
  --poll "kubectl get pods -A --no-headers | awk '\$4 != \"Running\" && \$4 != \"Completed\" {print \$1, \$2, \$4}'" --interval 60

# Containers that exited
klark feed add ops "A container stopped. Read its last logs and tell me why." \
  --poll "docker ps -a --filter status=exited --format '{{.ID}} {{.Names}} {{.Status}}'" --interval 60

# Disk over 90 %
klark feed add ops "A disk is over 90 % full. Say what is using the space." \
  --poll "df -P | awk 'NR>1 && \$1 ~ /^\/dev\// && \$5+0 > 90 {print \$6, \$5}'" --diff unified --interval 600
```

Acting on a feed (sending, a cheap agent waking the main one, `--filter`): `feed.md`.
