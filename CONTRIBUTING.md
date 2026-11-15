# Contributing

Thanks for helping. Pindot drives a real desktop with real input, so the
rules are mostly about safety.

## Before you start

- Read `CLAUDE.md`: it holds the decisions and the reasons behind them.
- The brain is the user's interactive `claude` CLI in a PTY. Pull requests
  that add `claude -p`, the Agent SDK, an API key or
  `--dangerously-skip-permissions` will not be merged.
- The product name never contains "Claude". Say "works with Claude Code".

## Build and test

```sh
cargo test --workspace
cd app && npm ci && npm run check && npm test && npm run build
```

All four must pass. New behaviour needs a test; OS code goes behind the
traits in `crates/pindot-core/src/platform/` with a mock for tests.

## Testing on a real desktop

- Only click and type in windows your test opened itself, and close them
  without saving. Never in an app the user already has open.
- Never synthesise a click on the approval window's Allow button, not even
  in a test. Test builds have `PINDOT_TEST_AUTO_APPROVE` for that, behind
  `--features test-switches`.
- Do not run `claude mcp add --scope user`, `claude mcp remove`,
  `claude login` or `claude logout` from tests; they change the user's own
  Claude Code setup. Use `--mcp-config <file> --strict-mcp-config`.
- Keep nested `claude` turns few and short (`--model haiku`): they use the
  tester's own Claude plan.
- Leave nothing running: every Pindot process, event tap, overlay and
  child `claude` your test started.

## Pull requests

- Small and focused, with what you tested and on which OS. "Tested" means
  you ran it and checked the result; otherwise say "not tested, because ...".
- Never commit tokens, model weights, audio (except the one test phrase in
  `docs/phase2/`), build output or `node_modules`.
- By contributing you agree your work is released under the MIT License.
