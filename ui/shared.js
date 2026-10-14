// Bridge between the UI and the Rust side.
//
// Inside the Tauri app, `window.__TAURI__` exists and every call goes to Rust.
// In a normal browser it falls back to mock data so the UI can be previewed on its own.
// Mock phases: ?mock=nearby | connecting | connected | error
(() => {
  const tauri = window.__TAURI__;
  const params = new URLSearchParams(location.search);
  const listeners = new Set();

  const ALL_FEATURES = [
    'battery', 'nc', 'ambient-level', 'voice-focus', 'speak-to-chat', 'wear-pause',
    'auto-power-off', 'connection-quality', 'dsee', 'eq',
  ];
  const PRESETS = [[0, 'Off'], [0x10, 'Bright'], [0x11, 'Excited'], [0x12, 'Mellow'], [0x13, 'Relaxed'],
    [0x14, 'Vocal'], [0x15, 'Treble Boost'], [0x16, 'Bass Boost'], [0x17, 'Speech'], [0xA0, 'Manual'],
    [0xA1, 'Custom 1'], [0xA2, 'Custom 2']].map(([id, name]) => ({ id, name }));

  const mock = (presence) => ({
    presence,
    rssi: -44,
    error: presence === 'error' ? 'Could not connect. If the headphones are in use by your phone, turn on "Connect to 2 devices" in Sony Sound Connect, or disconnect them from the phone first.' : null,
    name: 'WH-1000XM5',
    color: params.get('color') || 'black',
    firmware: '2.5.1',
    battery: { level: 53, charging: false },
    nc: { mode: 'nc', ambientLevel: 12, voiceFocus: false },
    speakToChat: false,
    wearPause: true,
    autoPowerOff: 'removed',
    autoPowerOffOptions: ['removed', 'off'],
    connectionQuality: 'sound',
    dsee: true,
    eq: { preset: 0xA0, presets: PRESETS, bands: [0, 0, 0, 0, 0], clearBass: 10 },
    settings: [
      { id: 0xD1, key: 'TOUCH_PANEL_SETTING', label: 'Touch sensor control panel', summary: 'Use the touch panel on the right ear cup for playback and volume.', on: true },
      { id: 0xD2, key: 'MULTIPOINT_SETTING', label: 'Connect to 2 devices simultaneously', summary: 'Keep the headphones connected to two devices at once, such as your phone and this Mac.', on: false },
    ],
    alert: null,
    features: ALL_FEATURES,
  });

  let state = mock(params.get('mock') || 'nearby');
  const emit = () => listeners.forEach((cb) => cb(state));

  function mockCall(cmd, args) {
    switch (cmd) {
      case 'connect':
        state = { ...state, presence: 'connecting' };
        emit();
        setTimeout(() => { state = { ...state, presence: 'connected' }; emit(); }, 1800);
        break;
      case 'set_nc': {
        const nc = { ...state.nc };
        if (args.mode) nc.mode = args.mode;
        if (args.ambientLevel !== undefined) nc.ambientLevel = args.ambientLevel;
        if (args.voiceFocus !== undefined) nc.voiceFocus = args.voiceFocus;
        state = { ...state, nc };
        emit();
        break;
      }
      case 'set_toggle':
      case 'set_choice':
        state = { ...state, [args.key]: args.value };
        emit();
        break;
      case 'set_eq': {
        const eq = { ...state.eq, bands: [...state.eq.bands] };
        if (args.preset !== undefined) eq.preset = args.preset;
        if (args.band !== undefined) { eq.bands[args.band] = args.value; if (eq.preset < 0xA0) eq.preset = 0xA0; }
        if (args.clearBass !== undefined) { eq.clearBass = args.clearBass; if (eq.preset < 0xA0) eq.preset = 0xA0; }
        state = { ...state, eq };
        emit();
        break;
      }
      case 'set_setting':
        if (args.id === 0xD2) {
          state = { ...state, alert: { message: 7, text: 'Switching multipoint will briefly disconnect the headphones.', choice: true }, _pending: args.on };
        } else {
          state = { ...state, settings: state.settings.map((x) => (x.id === args.id ? { ...x, on: args.on } : x)) };
        }
        emit();
        break;
      case 'answer_alert': {
        const on = args.positive && state._pending !== undefined ? state._pending : null;
        state = { ...state, alert: null, _pending: undefined, settings: on === null ? state.settings : state.settings.map((x) => (x.id === 0xD2 ? { ...x, on } : x)) };
        emit();
        break;
      }
      default:
        break;
    }
    return Promise.resolve();
  }

  // A bundled app has no console. Send page errors to the Rust log file.
  const report = (msg) => { if (tauri) tauri.core.invoke('ui_log', { msg: String(msg) }).catch(() => {}); };
  window.addEventListener('error', (e) => report(`error: ${e.message} at ${e.filename}:${e.lineno}`));
  window.addEventListener('unhandledrejection', (e) => report(`rejection: ${e.reason}`));

  window.xm5 = {
    get state() { return state; },
    onState(cb) { listeners.add(cb); cb(state); },
    call(cmd, args = {}) {
      if (!tauri) return mockCall(cmd, args);
      return tauri.core.invoke(cmd, args).catch((e) => report(`${cmd} failed: ${e}`));
    },
    isMock: !tauri,
    setMockPhase(p) { state = mock(p); emit(); },
  };

  if (tauri) {
    tauri.event.listen('xm5-state', (e) => { state = e.payload; emit(); });
    tauri.core.invoke('get_state').then((s) => { state = s; emit(); }).catch(() => {});
  }
})();
