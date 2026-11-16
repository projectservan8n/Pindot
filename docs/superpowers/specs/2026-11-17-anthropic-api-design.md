# The Anthropic API as a provider for Pindot's own loop

Status: step 3 of the four brains Tony asked for on 16 Nov 2026 ("do it
all"); follows `2026-11-16-pindot-own-loop-design.md`, whose loop, tools,
settings, keys and UI it reuses unchanged.

## What it adds

An "Anthropic" provider in Settings > Brain > An API key: Claude models
billed per use to the user's Anthropic API key, for people without a
Claude plan. Claude Code is untouched: the hard rule (no API key for Claude
Code) stands; this key is only ever used by Pindot's own loop.

## Decisions

- **A second wire format, not a second loop.** `providers::Provider` gains
  `wire: Wire` (`OpenAi` or `Anthropic`). `HttpModel` picks the request
  body and the stream parser by it; `run_turn`, the MCP client, context
  fitting, the conversation file and the model test are shared.
- **Raw HTTP with `ureq`.** Rust has no official Anthropic SDK, so the
  request follows the documented raw-HTTP shape: `POST /v1/messages`,
  `x-api-key`, `anthropic-version: 2026-10-21`, `stream: true`,
  `max_tokens: 64000`, tools as `{name, description, input_schema}` with
  `eager_input_streaming: true` (an incomplete input reaches the MCP
  client, which answers with an error the model can correct).
- **Thinking is left at each model's default** (no `thinking` field):
  current models think adaptively. Thinking blocks are kept as the API
  returned them (`Msg::Assistant.raw`) and replayed unchanged, as the API
  requires. When trimming edits the history, every thinking block is
  removed at once, which the API's history-editing check accepts.
- **Pictures** from tools go into the same user message as the tool
  results, after them.
- **Prompt caching**: top-level `cache_control: {type: "ephemeral"}`, which
  caches the growing conversation automatically.
- **Refusals**: on the models that support it (Claude Opus 5.5, Opus 5,
  Sonnet 5.5, Fable 5.1 and 5.2) requests carry server-side fallbacks
  (`fallbacks: "default"`, beta `server-side-fallback-2026-07-01`), so a
  declined request is retried on another model inside the same call. A
  final `refusal` is said plainly.
- **Models** come from `GET /v1/models` (`max_input_tokens` is the
  context, `capabilities.image_input.supported` is vision).
- **Errors**: 529 (overloaded) is "busy"; "prompt is too long" counts as a
  context overflow.

## Testing

Unit tests for the request body (system, roles, tool results with
pictures, raw replay, caching, fallbacks), the stream parser (text,
thinking with signature, a tool call in pieces, refusal, an error event),
the thinking strip after trimming, key warnings (`sk-ant-` against `sk-`);
an end-to-end test of a turn against a fake Messages API server and
Pindot's real MCP server. No real API call without a key from Tony.
