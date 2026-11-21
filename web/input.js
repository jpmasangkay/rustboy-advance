/**
 * Unified GBA input controller merging Keyboard, Web Gamepad API, and multi-touch
 * on-screen controls into a sanitized 10-bit active-high GBA button mask.
 */

export const GBA_KEYS = {
  A: 1 << 0,
  B: 1 << 1,
  SELECT: 1 << 2,
  START: 1 << 3,
  RIGHT: 1 << 4,
  LEFT: 1 << 5,
  UP: 1 << 6,
  DOWN: 1 << 7,
  R: 1 << 8,
  L: 1 << 9,
};

export const KEY_LABELS = {
  UP: "D-Pad Up",
  DOWN: "D-Pad Down",
  LEFT: "D-Pad Left",
  RIGHT: "D-Pad Right",
  A: "A Button",
  B: "B Button",
  L: "L Shoulder",
  R: "R Shoulder",
  START: "Start",
  SELECT: "Select",
};

export const DEFAULT_KEYBINDINGS = {
  UP: "ArrowUp",
  DOWN: "ArrowDown",
  LEFT: "ArrowLeft",
  RIGHT: "ArrowRight",
  A: "KeyZ",
  B: "KeyX",
  L: "KeyA",
  R: "KeyS",
  START: "Enter",
  SELECT: "Backspace",
};

const STORAGE_KEY_BINDINGS = "rustboy.keybindings.v1";
const STORAGE_KEY_SWAP_AB = "rustboy.gamepadSwapAB.v1";

export function formatKeyCode(code) {
  if (!code) return "Unbound";
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (code.startsWith("Arrow")) return code.slice(5);
  if (code === "Backspace") return "Backspace";
  if (code === "Enter") return "Enter";
  if (code === "ShiftLeft" || code === "ShiftRight") return "Shift";
  if (code === "Space") return "Space";
  return code;
}

export class InputManager {
  constructor() {
    this.bindings = this.loadBindings();
    this.swapGamepadAB = this.loadSwapAB();
    this.codeToBit = new Map();
    this.rebuildLookup();

    this.heldCodes = new Set();
    this.touchMask = 0;
    this.faceTouchMask = 0;
    this.dpadTouchMask = 0;
    this.touchResetters = [];
    this.activeGamepadName = "";
    this.rebindingButton = null;
    this.onRebindComplete = null;
    this.onGamepadChange = null;
    this.onShortcut = null;
  }

  clearAllInputs() {
    this.heldCodes.clear();
    this.touchMask = 0;
    this.faceTouchMask = 0;
    this.dpadTouchMask = 0;
    for (const reset of this.touchResetters) {
      reset();
    }
  }

  loadBindings() {
    try {
      const raw = localStorage.getItem(STORAGE_KEY_BINDINGS);
      if (!raw) return { ...DEFAULT_KEYBINDINGS };
      const parsed = JSON.parse(raw);
      return { ...DEFAULT_KEYBINDINGS, ...parsed };
    } catch {
      return { ...DEFAULT_KEYBINDINGS };
    }
  }

  saveBindings() {
    try {
      localStorage.setItem(STORAGE_KEY_BINDINGS, JSON.stringify(this.bindings));
    } catch {
      // Ignore storage quota errors in restricted browsing modes.
    }
  }

  loadSwapAB() {
    try {
      return localStorage.getItem(STORAGE_KEY_SWAP_AB) === "true";
    } catch {
      return false;
    }
  }

  setSwapGamepadAB(enabled) {
    this.swapGamepadAB = Boolean(enabled);
    try {
      localStorage.setItem(STORAGE_KEY_SWAP_AB, String(this.swapGamepadAB));
    } catch {
      // Ignore storage quota errors.
    }
  }

  resetBindings() {
    this.bindings = { ...DEFAULT_KEYBINDINGS };
    this.rebuildLookup();
    this.saveBindings();
  }

  setBinding(buttonName, code) {
    if (!(buttonName in GBA_KEYS)) return;
    // Prevent duplicate bindings: if another button already used `code`, swap with previous code.
    const prevCode = this.bindings[buttonName];
    for (const [otherBtn, otherCode] of Object.entries(this.bindings)) {
      if (otherBtn !== buttonName && otherCode === code) {
        this.bindings[otherBtn] = prevCode || "";
      }
    }
    this.bindings[buttonName] = code;
    this.rebuildLookup();
    this.saveBindings();
  }

  rebuildLookup() {
    this.codeToBit.clear();
    for (const [btn, code] of Object.entries(this.bindings)) {
      if (code && btn in GBA_KEYS) {
        this.codeToBit.set(code, GBA_KEYS[btn]);
      }
    }
  }

  attachListeners() {
    window.addEventListener("keydown", (e) => this.handleKeyDown(e));
    window.addEventListener("keyup", (e) => this.handleKeyUp(e));
    window.addEventListener("blur", () => {
      this.clearAllInputs();
    });
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") {
        this.clearAllInputs();
      }
    });

    window.addEventListener("gamepadconnected", (e) => {
      this.activeGamepadName = e.gamepad?.id || "Gamepad connected";
      this.onGamepadChange?.(this.activeGamepadName);
    });
    window.addEventListener("gamepaddisconnected", () => {
      this.refreshGamepadLabel();
    });
  }

  refreshGamepadLabel() {
    const pads = navigator.getGamepads ? navigator.getGamepads() : [];
    const active = Array.from(pads || []).find((p) => p && p.connected);
    this.activeGamepadName = active ? active.id : "";
    this.onGamepadChange?.(this.activeGamepadName);
  }

  isUiInteractiveFocused() {
    const active = document.activeElement;
    if (!active) return false;
    if (active.closest("dialog[open]")) return true;
    const tag = active.tagName;
    return tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA" || tag === "BUTTON" || tag === "A";
  }

  handleKeyDown(e) {
    if (this.rebindingButton) {
      e.preventDefault();
      e.stopPropagation();
      if (e.code === "Escape") {
        const cb = this.onRebindComplete;
        this.rebindingButton = null;
        cb?.(null);
        return;
      }
      const btn = this.rebindingButton;
      this.setBinding(btn, e.code);
      this.rebindingButton = null;
      this.onRebindComplete?.(btn, e.code);
      return;
    }

    if (document.querySelector("dialog[open]")) {
      return;
    }

    // Never hijack browser/OS modifier shortcuts (Ctrl+R, Cmd+W, Alt+Tab, etc.)
    // or risk leaving held keys stuck when a modifier combo shifts window focus.
    if (e.ctrlKey || e.metaKey || e.altKey) {
      return;
    }

    const mappedBit = this.codeToBit.get(e.code);
    const uiFocused = this.isUiInteractiveFocused();

    // User-configured game bindings always take precedence over built-in shortcuts
    // (e.g. if the player maps Space, P, F, or M to a GBA button), while still letting
    // focused UI controls (select dropdowns, toolbar buttons, sliders) handle keyboard input.
    if (mappedBit !== undefined) {
      if (uiFocused) {
        return;
      }
      e.preventDefault();
      this.heldCodes.add(e.code);
      return;
    }

    if (!uiFocused) {
      if (e.code === "KeyP" || e.code === "Space") {
        e.preventDefault();
        if (!e.repeat) this.onShortcut?.("toggle-pause");
        return;
      }
      if (e.code === "KeyF") {
        e.preventDefault();
        if (!e.repeat) this.onShortcut?.("toggle-fast-forward");
        return;
      }
      if (e.code === "KeyM") {
        e.preventDefault();
        if (!e.repeat) this.onShortcut?.("toggle-mute");
        return;
      }
    }
  }

  handleKeyUp(e) {
    if (this.heldCodes.has(e.code)) {
      this.heldCodes.delete(e.code);
    }
  }

  bindTouchDeck(deckEl, dpadEl, buttonEls) {
    const faceButtons = [];
    const standardButtons = [];
    for (const btn of buttonEls) {
      if (btn.classList.contains("touch-circle")) {
        faceButtons.push(btn);
      } else {
        standardButtons.push(btn);
      }
    }

    const updateButtonPress = (el, pressed) => {
      const key = el.dataset.key;
      const bit = GBA_KEYS[key];
      if (!bit) return;
      if (pressed) {
        this.touchMask |= bit;
        el.setAttribute("data-pressed", "true");
        if (navigator.vibrate) navigator.vibrate(6);
      } else {
        this.touchMask &= ~bit;
        el.removeAttribute("data-pressed");
      }
    };

    for (const btn of standardButtons) {
      const activePointers = new Set();
      this.touchResetters.push(() => {
        activePointers.clear();
        btn.removeAttribute("data-pressed");
      });
      btn.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        btn.setPointerCapture?.(e.pointerId);
        activePointers.add(e.pointerId);
        updateButtonPress(btn, true);
      });
      const release = (e) => {
        if (activePointers.delete(e.pointerId) && activePointers.size === 0) {
          updateButtonPress(btn, false);
        }
      };
      btn.addEventListener("pointerup", release);
      btn.addEventListener("pointercancel", release);
      btn.addEventListener("lostpointercapture", release);
    }

    // Face buttons (B & A): track pointer sliding across the face pad so thumb rolls
    // from B to A smoothly transfer or press both buttons without lifting.
    const faceContainer = faceButtons[0]?.parentElement;
    if (faceContainer && faceButtons.length > 0) {
      const facePointers = new Map();
      const recomputeFace = () => {
        let mask = 0;
        for (const m of facePointers.values()) mask |= m;
        this.faceTouchMask = mask;
        for (const btn of faceButtons) {
          const bit = GBA_KEYS[btn.dataset.key];
          if (bit && (mask & bit)) {
            btn.setAttribute("data-pressed", "true");
          } else {
            btn.removeAttribute("data-pressed");
          }
        }
      };
      this.touchResetters.push(() => {
        facePointers.clear();
        recomputeFace();
      });
      const faceMaskAt = (clientX, clientY) => {
        let mask = 0;
        const pad = 10;
        for (const btn of faceButtons) {
          const r = btn.getBoundingClientRect();
          if (
            clientX >= r.left - pad &&
            clientX <= r.right + pad &&
            clientY >= r.top - pad &&
            clientY <= r.bottom + pad
          ) {
            const bit = GBA_KEYS[btn.dataset.key];
            if (bit) mask |= bit;
          }
        }
        return mask;
      };
      faceContainer.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        faceContainer.setPointerCapture?.(e.pointerId);
        facePointers.set(e.pointerId, faceMaskAt(e.clientX, e.clientY));
        recomputeFace();
        if (navigator.vibrate) navigator.vibrate(6);
      });
      faceContainer.addEventListener("pointermove", (e) => {
        if (!facePointers.has(e.pointerId)) return;
        facePointers.set(e.pointerId, faceMaskAt(e.clientX, e.clientY));
        recomputeFace();
      });
      const releaseFace = (e) => {
        if (facePointers.delete(e.pointerId)) {
          recomputeFace();
        }
      };
      faceContainer.addEventListener("pointerup", releaseFace);
      faceContainer.addEventListener("pointercancel", releaseFace);
      faceContainer.addEventListener("lostpointercapture", releaseFace);
    }

    if (dpadEl) {
      const dpadPointers = new Map();
      const recomputeDpad = () => {
        let mask = 0;
        for (const m of dpadPointers.values()) mask |= m;
        this.dpadTouchMask = mask;
        dpadEl.dataset.dirUp = String(Boolean(mask & GBA_KEYS.UP));
        dpadEl.dataset.dirDown = String(Boolean(mask & GBA_KEYS.DOWN));
        dpadEl.dataset.dirLeft = String(Boolean(mask & GBA_KEYS.LEFT));
        dpadEl.dataset.dirRight = String(Boolean(mask & GBA_KEYS.RIGHT));
      };
      this.touchResetters.push(() => {
        dpadPointers.clear();
        recomputeDpad();
      });
      const maskFromPoint = (clientX, clientY) => {
        const rect = dpadEl.getBoundingClientRect();
        const cx = rect.left + rect.width / 2;
        const cy = rect.top + rect.height / 2;
        const dx = (clientX - cx) / (rect.width / 2);
        const dy = (clientY - cy) / (rect.height / 2);
        const deadzone = 0.22;
        let mask = 0;
        if (dx > deadzone) mask |= GBA_KEYS.RIGHT;
        else if (dx < -deadzone) mask |= GBA_KEYS.LEFT;
        if (dy > deadzone) mask |= GBA_KEYS.DOWN;
        else if (dy < -deadzone) mask |= GBA_KEYS.UP;
        return mask;
      };

      dpadEl.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        dpadEl.setPointerCapture?.(e.pointerId);
        dpadPointers.set(e.pointerId, maskFromPoint(e.clientX, e.clientY));
        recomputeDpad();
        if (navigator.vibrate) navigator.vibrate(6);
      });
      dpadEl.addEventListener("pointermove", (e) => {
        if (!dpadPointers.has(e.pointerId)) return;
        dpadPointers.set(e.pointerId, maskFromPoint(e.clientX, e.clientY));
        recomputeDpad();
      });
      const releaseDpad = (e) => {
        if (dpadPointers.delete(e.pointerId)) {
          recomputeDpad();
        }
      };
      dpadEl.addEventListener("pointerup", releaseDpad);
      dpadEl.addEventListener("pointercancel", releaseDpad);
      dpadEl.addEventListener("lostpointercapture", releaseDpad);
    }

    if (deckEl) {
      deckEl.addEventListener("contextmenu", (e) => e.preventDefault());
    }
  }

  pollGamepadMask() {
    if (!navigator.getGamepads || !document.hasFocus()) return 0;
    const pads = navigator.getGamepads();
    let mask = 0;
    let foundName = "";

    for (const pad of pads) {
      if (!pad || !pad.connected) continue;
      if (!foundName) foundName = pad.id || "Gamepad";

      const btn = (idx) => Boolean(pad.buttons[idx]?.pressed);
      const axisX = pad.axes[0] ?? 0;
      const axisY = pad.axes[1] ?? 0;
      const dz = 0.35;

      const south = btn(0);
      const east = btn(1);
      const west = btn(2);
      const north = btn(3);

      if (this.swapGamepadAB) {
        if (east || north) mask |= GBA_KEYS.A;
        if (south || west) mask |= GBA_KEYS.B;
      } else {
        if (south || north) mask |= GBA_KEYS.A;
        if (east || west) mask |= GBA_KEYS.B;
      }

      if (btn(4) || (pad.buttons[6]?.value ?? 0) > 0.5) mask |= GBA_KEYS.L;
      if (btn(5) || (pad.buttons[7]?.value ?? 0) > 0.5) mask |= GBA_KEYS.R;
      if (btn(8)) mask |= GBA_KEYS.SELECT;
      if (btn(9)) mask |= GBA_KEYS.START;

      if (btn(12) || axisY < -dz) mask |= GBA_KEYS.UP;
      if (btn(13) || axisY > dz) mask |= GBA_KEYS.DOWN;
      if (btn(14) || axisX < -dz) mask |= GBA_KEYS.LEFT;
      if (btn(15) || axisX > dz) mask |= GBA_KEYS.RIGHT;
    }

    if (foundName !== this.activeGamepadName) {
      this.activeGamepadName = foundName;
      this.onGamepadChange?.(foundName);
    }
    return mask;
  }

  getCombinedMask() {
    let mask = this.touchMask | this.faceTouchMask | this.dpadTouchMask | this.pollGamepadMask();
    for (const code of this.heldCodes) {
      const bit = this.codeToBit.get(code);
      if (bit !== undefined) mask |= bit;
    }

    // Prevent simultaneous opposing D-Pad directions from glitching games.
    if ((mask & (GBA_KEYS.LEFT | GBA_KEYS.RIGHT)) === (GBA_KEYS.LEFT | GBA_KEYS.RIGHT)) {
      mask &= ~(GBA_KEYS.LEFT | GBA_KEYS.RIGHT);
    }
    if ((mask & (GBA_KEYS.UP | GBA_KEYS.DOWN)) === (GBA_KEYS.UP | GBA_KEYS.DOWN)) {
      mask &= ~(GBA_KEYS.UP | GBA_KEYS.DOWN);
    }
    return mask & 0x03ff;
  }
}
