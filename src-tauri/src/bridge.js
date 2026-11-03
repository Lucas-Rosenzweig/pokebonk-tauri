// What the page can ask the desktop client for: `window.pokebonkDesktop`, the same object the Electron client's
// preload exposed (the game reads it in src/core/desktop.ts). Injected at document start into the game's webview.
// Kept minimal on purpose: the page is a website, so nothing here reaches files or the system.
(() => {
  if (window.pokebonkDesktop) return;
  const C = __PB_CONFIG__;
  const invoke = (cmd, args) => window.__TAURI_INTERNALS__.invoke(cmd, args || {});
  const send = (cmd, args) => { invoke(cmd, args).catch(() => {}); };

  // Same site as the game: its domain, its www and its subdomains. Anything else opens in the system browser.
  const sameSite = (u) => (u.protocol === C.protocol && (u.hostname === C.host || u.hostname.endsWith('.' + C.base))) || u.origin === C.origin;

  // ---- leaving during a run ----
  // WebKit in Tauri shows no «leave this page?» dialog, so the game's beforeunload would be ignored. We keep track of
  // its handlers and the client asks them before it closes or reloads (lib.rs, `ask_leave`), then shows its own dialog.
  const unload = new Set();
  const addEL = EventTarget.prototype.addEventListener;
  const remEL = EventTarget.prototype.removeEventListener;
  EventTarget.prototype.addEventListener = function (type, fn, opts) {
    if (this === window && type === 'beforeunload' && fn) unload.add(fn);
    return addEL.call(this, type, fn, opts);
  };
  EventTarget.prototype.removeEventListener = function (type, fn, opts) {
    if (this === window && type === 'beforeunload') unload.delete(fn);
    return remEL.call(this, type, fn, opts);
  };

  let fullscreen = false;
  const internal = {
    /** The window went in or out of fullscreen (called by the client). */
    fs(on) {
      on = !!on;
      if (fullscreen === on) return;
      fullscreen = on;
      document.dispatchEvent(new Event('fullscreenchange'));
    },
    /** Would the page ask before being left? True if a beforeunload handler cancels or sets returnValue. */
    askLeave() {
      let asks = false, rv;
      const ev = {
        type: 'beforeunload', target: window, currentTarget: window, defaultPrevented: false, isTrusted: true,
        preventDefault() { asks = true; this.defaultPrevented = true; },
        stopPropagation() {}, stopImmediatePropagation() {},
        get returnValue() { return rv; },
        set returnValue(v) { rv = v; asks = true; },
      };
      const call = (fn) => {
        try {
          const r = typeof fn === 'function' ? fn.call(window, ev) : fn && typeof fn.handleEvent === 'function' ? fn.handleEvent(ev) : undefined;
          if (typeof r === 'string') asks = true;
        } catch { /* a broken handler doesn't keep the player in */ }
      };
      unload.forEach(call);
      if (typeof window.onbeforeunload === 'function') call(window.onbeforeunload);
      return asks;
    },
  };
  Object.defineProperty(window, '__pb', { value: internal });
  invoke('pb_fullscreen').then((on) => internal.fs(on)).catch(() => {});

  // ---- keys the browser used to handle ----
  // F11 fullscreen, F5 reload (the client asks first during a run), F12 developer tools when they are on.
  // On macOS, ⌃⌘F, ⌘R and ⌥⌘I come from the app menu instead. No zoom: it blurs the pixel art.
  addEL.call(window, 'keydown', (e) => {
    const k = (e.key || '').toLowerCase(), ctrl = e.ctrlKey || e.metaKey;
    let action = null;
    if (e.key === 'F11') action = 'fullscreen';
    else if (e.key === 'F5' || (!C.mac && ctrl && !e.shiftKey && k === 'r')) action = 'reload';
    else if (e.key === 'F12' || (!C.mac && ctrl && e.shiftKey && k === 'i')) action = 'devtools';
    else if (ctrl && ['+', '-', '=', '0'].includes(k)) { e.preventDefault(); return; }
    if (!action) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    send('pb_key', { action });
  }, true);

  // ---- links to other sites (Discord, X, credits…) open in the system browser ----
  addEL.call(document, 'click', (e) => {
    if (e.defaultPrevented || e.button > 1) return;
    const a = e.target && e.target.closest && e.target.closest('a[href]');
    if (!a) return;
    let u;
    try { u = new URL(a.href, location.href); } catch { return; }
    if (!/^(https?|mailto):$/.test(u.protocol) || sameSite(u)) return;
    e.preventDefault();
    send('pb_open', { url: u.href });
  }, true);

  window.pokebonkDesktop = Object.freeze({
    version: C.version,
    platform: C.platform,
    /** Closes the client (during a run the client asks first). */
    quit: () => send('pb_quit'),
    /** Ajustes → Pantalla: window or fullscreen, size, monitor, V-Sync and graphics backend. */
    display: () => invoke('pb_display'),
    /** The window is fullscreen. The page can't see it in document.fullscreenElement, so it asks here. */
    fullscreen: () => fullscreen,
    setDisplay: (p) => invoke('pb_set_display', { p: p || {} }),
    /** Restarts the client; during a run it asks first, like closing. */
    relaunch: () => send('pb_relaunch'),
    /** The offline page's button: load the game again. */
    retry: () => send('pb_retry'),
    /** Discord Rich Presence: what the player is doing; null goes back to plain «Jugando a PokéBonk». */
    presence: (p) => send('pb_presence', { p: p ?? null }),
    /** No self-update in this client: the game's bar keeps offering the download, as on macOS with Electron. */
    update: Object.freeze({
      state: () => Promise.resolve({ state: 'off' }),
      onChange: () => {},
      install: () => {},
    }),
  });
})();
