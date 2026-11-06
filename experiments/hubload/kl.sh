#!/usr/bin/env bash
# Run JS in viewer browser N (headless chrome-agent with a throwaway profile).
#   kl.sh N up            start browser N, sign in as alice, load viewers.js
#   kl.sh N '<js>'        evaluate (async) JS in it, e.g. 'return KL.stats()'
#   kl.sh N down          stop browser N
. "$(dirname "$0")/env.sh"
N=$1; shift
export CHROME_AGENT_PROFILE=$K/chrome$N CHROME_AGENT_ID=kload-$N
HUB=${HUB:-http://127.0.0.1:5100}
case "$1" in
  up)
    mkdir -p "$CHROME_AGENT_PROFILE"
    chrome-agent up --headless >/dev/null 2>&1
    chrome-agent goto "$HUB/login" >/dev/null 2>&1
    chrome-agent evalAsync 'const f=document.forms[0]; f.querySelector("[name=username]").value="alice"; f.querySelector("[name=password]").value="alice"; const r=await fetch(f.action,{method:"POST",body:new FormData(f)}); return r.status' >/dev/null
    chrome-agent goto "$HUB/token" >/dev/null 2>&1
    chrome-agent evalAsync "$(cat "$(dirname "$0")/viewers.js")"
    ;;
  down)
    pid=$(pgrep -f "MacOS/Chromium --user-data-dir=$CHROME_AGENT_PROFILE")
    [ -n "$pid" ] && kill ${SIG:-} $pid
    ;;
  *) chrome-agent evalAsync "$1" ;;
esac
