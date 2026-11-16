#!/bin/bash
# Re-renders every screen and state of the new look into docs/ui-pass/design/{dark,light}.
here="$(cd "$(dirname "$0")" && pwd)"
S="$here/shots.sh"
render() {
  out="$1"
  "$S" "$out" "activity=#activity" "activity-empty=?s=empty#activity" \
    "chat-approval=?c=approval#chat" "chat-attachment=?c=attach#chat" "chat-conversation=?c=convo#chat" \
    "chat-empty=?c=empty#chat" "chat-error=?c=error#chat" "chat-history=?c=history#chat" \
    "chat-not-ready=?c=nosession#chat" "chat-steps-expanded=?c=steps#chat" \
    "chat-dictation-listening=?c=dict-listening#chat" "chat-dictation-transcribing=?c=dict-transcribing#chat" \
    "chat-dictation-error=?c=dict-error#chat" "chat-dictation-done=?c=dict-done#chat" \
    "chat-voice-off=?s=chatonly&c=chatonly#chat" \
    "settings=#settings" "settings-no-brain=?s=nobrain#settings" "setup=#setup" "setup-first-run=?s=setup#setup"
  H=2300 "$S" "$out" "settings-full=#settings" "setup-first-run-full=?s=setup#setup"
  "$S" "$out" "vault-empty=?v=empty#vault" "vault-list=?v=list#vault" "vault-add=?v=add#vault" \
    "vault-import=?v=import#vault" "vault-import-result=?v=result#vault" "vault-history=?v=history#vault" \
    "vault-login-approval=?v=approval#chat"
}
render "$here/../design/dark"
LIGHT=1 render "$here/../design/light"
