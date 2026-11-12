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
    this.dpadTouchMask = 0;
    this.activeGamepadName = "";
    this.rebindingButton = null;
    this.onRebindComplete = null;
    this.onGamepadChange = null;
    this.onShortcut = null;
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
      this.heldCodes.clear();
      this.touchMask = 0;
      this.dpadTouchMask = 0;
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

    const mappedBit = this.codeToBit.get(e.code);
    const uiFocused = this.isUiInteractiveFocused();

    // Prevent Enter/Space/Backspace from clicking focused toolbar controls while playing unless a button is explicitly focused.
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

    if (mappedBit !== undefined) {
      // Allow keyboard navigation inside UI controls when a button/input is focused,
      // except Arrow keys when canvas/body has focus.
      if (uiFocused && (e.code === "Enter" || e.code === "Space" || e.code === "Backspace")) {
        return;
      }
      e.preventDefault();
      this.heldCodes.add(e.code);
    }
  }

  handleKeyUp(e) {
    if (this.heldCodes.has(e.code)) {
      this.heldCodes.delete(e.code);
    }
  }

  bindTouchDeck(deckEl, dpadEl, buttonEls) {
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

    for (const btn of buttonEls) {
      const activePointers = new Set();
      btn.addEventListener("pointerdown", (e) => {
        e.preventDefault();
        btn.setPointerCapture?.(e.pointerId);
        activePointers.add(e.pointerId);
        updateButtonPress(btn, true);
      });
      const release = (e) => {
        activePointers.delete(e.pointerId);
        if (activePointers.size === 0) {
          updateButtonPress(btn, false);
        }
      };
      btn.addEventListener("pointerup", release);
      btn.addEventListener("pointercancel", release);
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
    let mask = this.touchMask | this.dpadTouchMask | this.pollGamepadMask();
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
