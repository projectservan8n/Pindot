#!/bin/bash
# Re-renders every main-window screenshot in docs/ui-pass/after.
here="$(cd "$(dirname "$0")" && pwd)"
out="$here/../after"
S="$here/shots.sh"
"$S" "$out" "activity=#activity" "activity-empty=?s=empty#activity" \
  "chat-approval=?c=approval#chat" "chat-attachment=?c=attach#chat" "chat-conversation=?c=convo#chat" \
  "chat-empty=?c=empty#chat" "chat-error=?c=error#chat" "chat-history=?c=history#chat" \
  "chat-not-ready=?c=nosession#chat" "chat-steps-expanded=?c=steps#chat" \
  "settings=#settings" "settings-no-brain=?s=nobrain#settings" "setup=#setup" "setup-first-run=?s=setup#setup"
H=2300 "$S" "$out" "settings-full=#settings" "setup-first-run-full=?s=setup#setup"
