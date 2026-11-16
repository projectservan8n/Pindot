// Stands in for Tauri while taking screenshots in plain headless Chrome:
// `window.__TAURI_INTERNALS__.invoke` answers from a table. Only for the
// screenshot tool (shots.sh); the app never loads this file.
(function () {
  const q = new URLSearchParams(location.search);
  const scenario = q.get('s') || 'default';
  const listeners = {};
  let cb = 1;
  const callbacks = {};
  const GB = 1024 * 1024 * 1024;
  const settings = {
    version: 1, hotkey: 'Ctrl+Alt+Space', port: 47831, voice_enabled: true, onboarding_done: true,
    cursor_color: '#ff2d95', streaming_mode: false,
    voice: {
      stt_model: 'parakeet-tdt-0.6b-v3', tts_model: 'phonon', tts_voice: 'Aria', idle_reset_secs: 600,
      speak_mode: 'each_message', volume: 0.8, model: null, effort: null, permission_mode: 'ask',
      tap_to_talk: true, companion: true, captions: true, assistant_name: 'Rusty', talk_style: 'friendly',
      talk_notes: '', brain_folder: scenario === 'nobrain' ? null : '/Users/sam/Notes', input_device: null,
    },
    brain: scenario === 'codex'
      ? { kind: 'codex', provider: 'ollama', base_url: 'http://127.0.0.1:11434/v1', model: '', vision: 'auto', context_tokens: null, tested: [] }
      : scenario === 'loop'
      ? { kind: 'loop', provider: 'ollama', base_url: 'http://127.0.0.1:11434/v1', model: 'qwen3:8b', vision: 'auto', context_tokens: null,
          tested: [{ base_url: 'http://127.0.0.1:11434/v1', model: 'qwen3:8b', ok: true, note: '', at: 1794840000 }] }
      : { kind: 'claude_code', provider: 'ollama', base_url: 'http://127.0.0.1:11434/v1', model: '', vision: 'auto', context_tokens: null, tested: [] },
  };
  const info = {
    version: '0.4.0', os: 'macos', data_dir: '/Users/sam/Library/Application Support/Pindot',
    settings_path: '/Users/sam/Library/Application Support/Pindot/settings.json', load_note: '',
    server_url: 'http://127.0.0.1:47831/mcp', server_error: null, voice: 'running', voice_note: null,
    hotkey_label: 'Ctrl+Option+Space', capturable: true, test_switches: false,
  };
  if (scenario === 'setup') info.voice = 'off';
  if (scenario === 'chatonly') { info.voice = 'chat_only'; info.voice_note = 'Voice needs its models. Chat works.'; }
  const perms = [
    ['Accessibility', 'Accessibility', 'Lets Pindot click and type for you.', 'granted'],
    ['InputMonitoring', 'Input Monitoring', 'Lets Pindot hear the hold-to-talk key.', 'granted'],
    ['ScreenRecording', 'Screen Recording', 'Lets Pindot see your screen.', scenario === 'setup' ? 'not_asked' : 'granted'],
    ['Microphone', 'Microphone', 'Lets Pindot hear you.', scenario === 'setup' ? 'denied' : 'granted'],
  ].map(([permission, name, why, state]) => ({ permission, granted: state === 'granted', state, name, why, settings_url: 'x-apple.systempreferences:', note: null }));
  const m = (id, display, kind, size, present) => ({ id, display, kind, license: 'CC-BY-4.0', license_url: 'https://example.com', license_text: '', source: 'huggingface', size, present, in_use: true, fallback: null, dir: null });
  const models = [
    m('parakeet-tdt-0.6b-v3', 'Parakeet speech recognition', 'SpeechToText', 0.6 * GB, scenario !== 'setup'),
    m('phonon', 'Phonon voice', 'TextToSpeech', 0.3 * GB, scenario !== 'setup'),
  ];
  const voices = ['Aria', 'Marcus', 'Ivy', 'Theo'].map((name, i) => ({ name, locale: i % 2 ? 'en-gb' : 'en-us', gender: i % 2 ? 'male' : 'female', age: 'adult', description: 'Warm and clear' }));
  const act = (i, tool, args, ok = true) => ({ ts: `2026-11-16T09:${10 + i}:00Z`, session: 'f73c7cb0', tool, args, outcome: ok ? 'ok' : 'denied', ok });
  const entries = scenario === 'empty' ? [] : [
    act(1, 'screenshot', ''), act(2, 'click', 'x=412 y=301'), act(3, 'type_text', '"hello"'),
    act(4, 'press_key', 'Return'), act(5, 'click', 'x=80 y=900', false),
  ];
  const routes = {
    app_info: () => info,
    settings_get: () => ({ settings, overrides: [], stt_models: models.filter((x) => x.kind === 'SpeechToText').map((x) => ({ id: x.id, display: x.display, license: x.license, size: x.size })), tts_models: models.filter((x) => x.kind === 'TextToSpeech').map((x) => ({ id: x.id, display: x.display, license: x.license, size: x.size })), tts_voices: voices, default_port: 47831, running_port: 47831, os: 'macos', streaming_supported: true, brain_folder_notice: null }),
    list_input_devices: () => ({ devices: ['MacBook Pro Microphone', 'Studio Display Microphone'], default: 'MacBook Pro Microphone' }),
    claude_status: () => ({ path: '/usr/local/bin/claude', version: '2.1.325', signed_in: true, method: 'Claude subscription', problem: null, hint: '' }),
    permissions: () => perms,
    models_status: () => models,
    connect_status: () => ({ status: { state: 'not_connected' }, command: 'claude mcp add --transport http desktop http://127.0.0.1:47831/mcp', remove_command: 'claude mcp remove desktop', claude_found: true, port: 47831 }),
    activity_all: () => ({ path: '/Users/sam/Library/Application Support/Pindot/activity.jsonl', entries, sessions: ['f73c7cb0'], tools: ['click', 'press_key', 'screenshot', 'type_text'], total: entries.length }),
    hotkey_check: () => ({ ok: true, normalized: 'ctrl+alt+space', label: 'Ctrl+Option+Space', caution: null, error: null }),
    voice_check: () => [],
    settings_save: () => ({ notes: [] }),
    brain_providers: () => [
      ['ollama', 'Ollama', 'http://127.0.0.1:11434/v1', true], ['lmstudio', 'LM Studio', 'http://127.0.0.1:1234/v1', true],
      ['llamacpp', 'llama.cpp', 'http://127.0.0.1:8080/v1', true], ['openai', 'OpenAI', 'https://api.openai.com/v1', false],
      ['anthropic', 'Anthropic', 'https://api.anthropic.com/v1', false], ['openrouter', 'OpenRouter', 'https://openrouter.ai/api/v1', false], ['custom', 'Other (OpenAI-compatible)', '', false],
    ].map(([id, name, base_url, local]) => ({ id, name, base_url, local, needs_key: !local && id !== 'custom', has_key: id === 'openai' })),
    brain_find_local: () => [{ provider: 'ollama', name: 'Ollama', base_url: 'http://127.0.0.1:11434/v1', models: [
      { id: 'qwen3:8b', vision: false, tools: true, context: 40960 }, { id: 'gemma3:12b', vision: true, tools: false, context: 131072 }] }],
    brain_codex_status: () => ({ path: '/Users/sam/.npm-global/bin/codex', signed_in: false, problem: null }),
    brain_models: () => [{ id: 'qwen3:8b', vision: false, tools: true, context: 40960 }, { id: 'gemma3:12b', vision: true, tools: false, context: 131072 }],
  };
  Object.assign(routes, window.__MOCK_ROUTES || {});
  window.__MOCK_EMIT = (event, payload) => (listeners[event] || []).forEach((id) => callbacks[id] && callbacks[id]({ event, id, payload }));
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
  window.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main', windowLabel: 'main' } },
    transformCallback(fn) { const id = cb++; callbacks[id] = fn; return id; },
    unregisterCallback(id) { delete callbacks[id]; },
    convertFileSrc: (p) => p,
    async invoke(cmd, args) {
      if (cmd === 'plugin:event|listen') { (listeners[args.event] ||= []).push(args.handler); return args.handler; }
      if (cmd.startsWith('plugin:')) return null;
      const r = (window.__MOCK_ROUTES && window.__MOCK_ROUTES[cmd]) || routes[cmd];
      return r ? r(args) : null;
    },
  };
})();
