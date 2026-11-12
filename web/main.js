import initWasm, { Emulator } from "./pkg/gba_frontend_web.js";
import { EmulatorStorage } from "./storage.js";
import { InputManager, KEY_LABELS, formatKeyCode } from "./input.js";
import { createBuiltInTestRom } from "./test-rom.js";

const GBA_WIDTH = 240;
const GBA_HEIGHT = 160;
const GBA_FPS = 16_777_216 / 280_896; // ~59.72750057 Hz
const GBA_FRAME_MS = 1000 / GBA_FPS;
const SRAM_AUTOSAVE_MS = 5000;
const PREFS_STORAGE_KEY = "rustboy.prefs.v1";

const DEFAULT_PREFS = {
  colorProfile: 1,
  integerScale: true,
  lcdGrid: false,
  lcdGhosting: false,
  touchControls: null, // null = auto-detect coarse pointer
  volume: 100,
  muted: false,
  speed: 1,
};

function loadPrefs() {
  try {
    const raw = localStorage.getItem(PREFS_STORAGE_KEY);
    if (!raw) return { ...DEFAULT_PREFS };
    return { ...DEFAULT_PREFS, ...JSON.parse(raw) };
  } catch {
    return { ...DEFAULT_PREFS };
  }
}

function savePrefs(prefs) {
  try {
    localStorage.setItem(PREFS_STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Ignore storage failures in restricted contexts.
  }
}

function setupDialogLightDismissFallback() {
  if ("closedBy" in HTMLDialogElement.prototype) return;
  const outside = (e, dialog) => {
    const r = dialog.getBoundingClientRect();
    return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
  };
  for (const dialog of document.querySelectorAll('dialog[closedby="any"]')) {
    let pressedOutside = false;
    dialog.addEventListener("pointerdown", (e) => {
      pressedOutside = outside(e, dialog);
    });
    dialog.addEventListener("pointerup", (e) => {
      if (pressedOutside && outside(e, dialog)) {
        dialog.close();
      }
    });
  }
}

function setupToolbarRovingTabindexFallback() {
  if ("focusGroup" in HTMLElement.prototype) return;
  const bar = document.querySelector('[role="toolbar"]');
  if (!bar) return;
  const getItems = () => Array.from(bar.querySelectorAll("button"));
  const items = getItems();
  items.forEach((b, i) => {
    b.tabIndex = i === 0 ? 0 : -1;
  });
  bar.addEventListener("focusin", ({ target }) => {
    const cur = getItems();
    if (cur.includes(target)) {
      cur.forEach((b) => {
        b.tabIndex = b === target ? 0 : -1;
      });
    }
  });
  bar.addEventListener("keydown", (e) => {
    const cur = getItems().filter((b) => !b.hidden);
    const idx = cur.indexOf(document.activeElement);
    if (idx < 0) return;
    const rtl = getComputedStyle(bar).direction === "rtl";
    const step =
      e.key === "ArrowRight"
        ? rtl
          ? -1
          : 1
        : e.key === "ArrowLeft"
          ? rtl
            ? 1
            : -1
          : e.key === "Home"
            ? -idx
            : e.key === "End"
              ? cur.length - 1 - idx
              : 0;
    if (step !== 0) {
      e.preventDefault();
      const next = cur[(idx + step + cur.length) % cur.length];
      next.focus();
    }
  });
}

class RustboyWebApp {
  constructor() {
    this.prefs = loadPrefs();
    this.storage = new EmulatorStorage();
    this.input = new InputManager();

    this.wasm = null;
    this.emu = null;
    this.audioCtx = null;
    this.audioNode = null;
    this.audioWorkletReady = false;

    this.userPaused = false;
    this.bgPaused = false;
    this.lastFrameTs = 0;
    this.accMs = 0;
    this.rafId = 0;

    this.fpsFrames = 0;
    this.fpsWindowStart = performance.now();
    this.lastAutosaveTs = performance.now();
    this.toastTimer = 0;

    this.canvas = document.getElementById("gba-canvas");
    this.ctx = this.canvas.getContext("2d", { alpha: false });
    this.imageData = this.ctx.createImageData(GBA_WIDTH, GBA_HEIGHT);
    this.frameBufferView = null;
    this.cachedWasmBuffer = null;

    this.bindElements();
    this.applySavedPrefsToUi();
    this.bindEvents();
    this.renderKeybindingsTable();
  }

  bindElements() {
    this.el = {
      stateLoading: document.getElementById("state-loading"),
      loadingCaption: document.getElementById("loading-caption"),
      stateEmpty: document.getElementById("state-empty"),
      stateRunning: document.getElementById("state-running"),
      dragOverlay: document.getElementById("drag-overlay"),
      screenStage: document.getElementById("screen-stage"),
      stageShell: document.getElementById("stage-shell"),
      canvasWrap: document.getElementById("canvas-wrap"),
      lcdGridOverlay: document.getElementById("lcd-grid-overlay"),
      pausedScrim: document.getElementById("paused-scrim"),

      powerIndicator: document.getElementById("power-indicator"),
      powerLabel: document.getElementById("power-label"),
      cartBadge: document.getElementById("cart-badge"),

      errorBanner: document.getElementById("error-banner"),
      errorTitle: document.getElementById("error-title"),
      errorMessage: document.getElementById("error-message"),
      btnDismissError: document.getElementById("btn-dismiss-error"),

      btnLoadRom: document.getElementById("btn-load-rom"),
      btnPause: document.getElementById("btn-pause"),
      btnReset: document.getElementById("btn-reset"),
      btnFastForward: document.getElementById("btn-fast-forward"),
      speedLabel: document.getElementById("speed-label"),
      btnMute: document.getElementById("btn-mute"),
      btnUnlockAudio: document.getElementById("btn-unlock-audio"),
      btnStates: document.getElementById("btn-states"),
      btnSettings: document.getElementById("btn-settings"),
      btnFullscreen: document.getElementById("btn-fullscreen"),

      btnEmptyLoad: document.getElementById("btn-empty-load"),
      btnEmptyDemo: document.getElementById("btn-empty-demo"),
      btnEmptyBios: document.getElementById("btn-empty-bios"),
      btnScrimResume: document.getElementById("btn-scrim-resume"),

      quickColorProfile: document.getElementById("quick-color-profile"),
      btnQuickGrid: document.getElementById("btn-quick-grid"),
      btnQuickInteger: document.getElementById("btn-quick-integer"),
      btnQuickTouch: document.getElementById("btn-quick-touch"),

      touchDeck: document.getElementById("touch-deck"),
      touchDpad: document.getElementById("touch-dpad"),

      statFps: document.getElementById("stat-fps"),
      statAudio: document.getElementById("stat-audio"),
      statBios: document.getElementById("stat-bios"),
      statCrc: document.getElementById("stat-crc"),
      statSave: document.getElementById("stat-save"),
      statGamepad: document.getElementById("stat-gamepad"),

      srStatus: document.getElementById("sr-status"),
      toast: document.getElementById("toast"),

      statesDialog: document.getElementById("states-dialog"),
      statesGameLabel: document.getElementById("states-game-label"),
      slotsList: document.getElementById("slots-list"),
      btnExportStateFile: document.getElementById("btn-export-state-file"),
      btnImportStateFile: document.getElementById("btn-import-state-file"),

      settingsDialog: document.getElementById("settings-dialog"),
      setColorProfile: document.getElementById("set-color-profile"),
      setSpeed: document.getElementById("set-speed"),
      setVolume: document.getElementById("set-volume"),
      volReadout: document.getElementById("vol-readout"),
      setIntegerScale: document.getElementById("set-integer-scale"),
      setLcdGrid: document.getElementById("set-lcd-grid"),
      setLcdGhosting: document.getElementById("set-lcd-ghosting"),
      setTouchControls: document.getElementById("set-touch-controls"),
      setSwapAb: document.getElementById("set-swap-ab"),

      btnExportSav: document.getElementById("btn-export-sav"),
      btnImportSav: document.getElementById("btn-import-sav"),
      btnLoadBios: document.getElementById("btn-load-bios"),
      btnClearBios: document.getElementById("btn-clear-bios"),
      biosStatusText: document.getElementById("bios-status-text"),

      keybindingsGrid: document.getElementById("keybindings-grid"),
      btnResetKeys: document.getElementById("btn-reset-keys"),

      romFileInput: document.getElementById("rom-file-input"),
      biosFileInput: document.getElementById("bios-file-input"),
      savFileInput: document.getElementById("sav-file-input"),
      stateFileInput: document.getElementById("state-file-input"),
    };
  }

  applySavedPrefsToUi() {
    this.el.quickColorProfile.value = String(this.prefs.colorProfile);
    this.el.setColorProfile.value = String(this.prefs.colorProfile);
    this.el.setSpeed.value = String(this.prefs.speed);
    this.el.setVolume.value = String(this.prefs.volume);
    this.el.volReadout.textContent = `${this.prefs.volume}%`;
    this.el.setIntegerScale.checked = Boolean(this.prefs.integerScale);
    this.el.btnQuickInteger.setAttribute("aria-pressed", String(Boolean(this.prefs.integerScale)));
    this.el.setLcdGrid.checked = Boolean(this.prefs.lcdGrid);
    this.el.btnQuickGrid.setAttribute("aria-pressed", String(Boolean(this.prefs.lcdGrid)));
    this.el.setLcdGhosting.checked = Boolean(this.prefs.lcdGhosting);
    this.el.btnMute.setAttribute("aria-pressed", String(Boolean(this.prefs.muted)));
    this.el.setSwapAb.checked = this.input.swapGamepadAB;

    const coarse = window.matchMedia("(pointer: coarse)").matches;
    const showTouch = this.prefs.touchControls ?? coarse;
    this.setTouchDeckVisible(showTouch, false);
    this.updateSpeedUi();
  }

  bindEvents() {
    setupDialogLightDismissFallback();
    setupToolbarRovingTabindexFallback();

    this.input.attachListeners();
    this.input.onGamepadChange = (name) => {
      this.el.statGamepad.textContent = name ? name.slice(0, 26) : "Keyboard";
    };
    this.input.onShortcut = (action) => {
      if (!this.emu?.rom_loaded) return;
      if (action === "toggle-pause") this.togglePause();
      else if (action === "toggle-fast-forward") this.toggleFastForward();
      else if (action === "toggle-mute") this.toggleMute();
    };

    const touchButtons = this.el.touchDeck.querySelectorAll(".touch-btn");
    this.input.bindTouchDeck(this.el.touchDeck, this.el.touchDpad, touchButtons);

    // File pickers
    this.el.btnLoadRom.addEventListener("click", () => this.el.romFileInput.click());
    this.el.btnEmptyLoad.addEventListener("click", () => this.el.romFileInput.click());
    this.el.btnEmptyDemo.addEventListener("click", () => this.loadBuiltInDemo());
    this.el.btnEmptyBios.addEventListener("click", () => this.el.biosFileInput.click());
    this.el.btnLoadBios.addEventListener("click", () => this.el.biosFileInput.click());
    this.el.btnClearBios.addEventListener("click", () => this.clearBios());
    this.el.btnExportSav.addEventListener("click", () => this.downloadSramFile());
    this.el.btnImportSav.addEventListener("click", () => this.el.savFileInput.click());

    this.el.romFileInput.addEventListener("change", (e) => {
      const file = e.target.files?.[0];
      e.target.value = "";
      if (file) this.handleDroppedOrPickedFile(file);
    });
    this.el.biosFileInput.addEventListener("change", (e) => {
      const file = e.target.files?.[0];
      e.target.value = "";
      if (file) this.loadBiosFile(file);
    });
    this.el.savFileInput.addEventListener("change", (e) => {
      const file = e.target.files?.[0];
      e.target.value = "";
      if (file) this.importSramFile(file);
    });
    this.el.stateFileInput.addEventListener("change", (e) => {
      const file = e.target.files?.[0];
      e.target.value = "";
      if (file) this.importStateFile(file);
    });

    // Drag & Drop onto viewport
    let dragDepth = 0;
    const dropTarget = this.el.stageShell;
    dropTarget.addEventListener("dragenter", (e) => {
      e.preventDefault();
      dragDepth += 1;
      this.el.dragOverlay.hidden = false;
    });
    dropTarget.addEventListener("dragover", (e) => {
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
    });
    dropTarget.addEventListener("dragleave", () => {
      dragDepth = Math.max(0, dragDepth - 1);
      if (dragDepth === 0) this.el.dragOverlay.hidden = true;
    });
    dropTarget.addEventListener("drop", (e) => {
      e.preventDefault();
      dragDepth = 0;
      this.el.dragOverlay.hidden = true;
      const file = e.dataTransfer?.files?.[0];
      if (file) this.handleDroppedOrPickedFile(file);
    });

    // Toolbar actions (respecting aria-disabled)
    this.el.btnPause.addEventListener("click", () => {
      if (this.isDisabled(this.el.btnPause)) return;
      this.togglePause();
    });
    this.el.btnScrimResume.addEventListener("click", () => this.togglePause(false));
    this.el.btnReset.addEventListener("click", () => {
      if (this.isDisabled(this.el.btnReset)) return;
      this.resetConsole();
    });
    this.el.btnFastForward.addEventListener("click", () => {
      if (this.isDisabled(this.el.btnFastForward)) return;
      this.toggleFastForward();
    });
    this.el.btnMute.addEventListener("click", () => this.toggleMute());
    this.el.btnUnlockAudio.addEventListener("click", () => this.resumeAudio());
    this.el.btnStates.addEventListener("click", () => {
      if (this.isDisabled(this.el.btnStates)) return;
      this.openStatesDialog();
    });
    this.el.btnSettings.addEventListener("click", () => this.el.settingsDialog.showModal());
    this.el.btnFullscreen.addEventListener("click", () => this.toggleFullscreen());
    this.el.btnDismissError.addEventListener("click", () => this.clearError());

    // Quick controls & Settings bindings
    const onProfileChange = (val) => {
      const id = Number(val);
      this.prefs.colorProfile = id;
      this.el.quickColorProfile.value = String(id);
      this.el.setColorProfile.value = String(id);
      savePrefs(this.prefs);
      if (this.emu) {
        this.emu.set_color_profile(id);
        this.presentFrame();
      }
    };
    this.el.quickColorProfile.addEventListener("change", (e) => onProfileChange(e.target.value));
    this.el.setColorProfile.addEventListener("change", (e) => onProfileChange(e.target.value));

    const onIntegerToggle = (checked) => {
      this.prefs.integerScale = checked;
      this.el.setIntegerScale.checked = checked;
      this.el.btnQuickInteger.setAttribute("aria-pressed", String(checked));
      savePrefs(this.prefs);
      this.updateCanvasSize();
    };
    this.el.btnQuickInteger.addEventListener("click", () =>
      onIntegerToggle(!this.prefs.integerScale),
    );
    this.el.setIntegerScale.addEventListener("change", (e) => onIntegerToggle(e.target.checked));

    const onGridToggle = (checked) => {
      this.prefs.lcdGrid = checked;
      this.el.setLcdGrid.checked = checked;
      this.el.btnQuickGrid.setAttribute("aria-pressed", String(checked));
      savePrefs(this.prefs);
      this.updateCanvasSize();
    };
    this.el.btnQuickGrid.addEventListener("click", () => onGridToggle(!this.prefs.lcdGrid));
    this.el.setLcdGrid.addEventListener("change", (e) => onGridToggle(e.target.checked));

    this.el.setLcdGhosting.addEventListener("change", (e) => {
      this.prefs.lcdGhosting = e.target.checked;
      savePrefs(this.prefs);
      this.emu?.set_lcd_ghosting(this.prefs.lcdGhosting);
    });

    const onTouchToggle = (checked) => this.setTouchDeckVisible(checked, true);
    this.el.btnQuickTouch.addEventListener("click", () =>
      onTouchToggle(this.el.touchDeck.hidden),
    );
    this.el.setTouchControls.addEventListener("change", (e) => onTouchToggle(e.target.checked));

    this.el.setSpeed.addEventListener("change", (e) => {
      this.setSpeed(Number(e.target.value));
    });

    this.el.setVolume.addEventListener("input", (e) => {
      const vol = Number(e.target.value);
      this.prefs.volume = vol;
      this.el.volReadout.textContent = `${vol}%`;
      savePrefs(this.prefs);
      this.emu?.set_volume(vol);
      this.audioNode?.port.postMessage({ type: "volume", volume: vol / 100 });
    });

    this.el.setSwapAb.addEventListener("change", (e) => {
      this.input.setSwapGamepadAB(e.target.checked);
    });
    this.el.btnResetKeys.addEventListener("click", () => {
      this.input.resetBindings();
      this.renderKeybindingsTable();
      this.notify("Reset keyboard bindings to defaults");
    });

    // Dialog close buttons & returnValue reset
    for (const btn of document.querySelectorAll("[data-close-dialog]")) {
      btn.addEventListener("click", () => {
        document.getElementById(btn.dataset.closeDialog)?.close();
      });
    }
    for (const dlg of [this.el.statesDialog, this.el.settingsDialog]) {
      dlg.addEventListener("close", () => {
        dlg.returnValue = "";
        this.input.rebindingButton = null;
      });
    }

    this.el.btnExportStateFile.addEventListener("click", () => this.exportStateFile());
    this.el.btnImportStateFile.addEventListener("click", () => this.el.stateFileInput.click());

    // Resize, fullscreen & visibility lifecycle
    const ro = new ResizeObserver(() => this.updateCanvasSize());
    ro.observe(this.el.screenStage);
    document.addEventListener("fullscreenchange", () => {
      const isFs = document.fullscreenElement === this.el.stageShell;
      this.el.btnFullscreen.setAttribute("aria-pressed", String(isFs));
      this.updateCanvasSize();
    });

    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden") {
        this.bgPaused = true;
        this.audioNode?.port.postMessage({ type: "clear" });
        this.flushSramIfDirty();
      } else {
        this.bgPaused = false;
        this.lastFrameTs = performance.now();
        this.accMs = 0;
      }
    });
    window.addEventListener("pagehide", () => this.flushSramIfDirty());

    // Resume WebAudio transparently on first gesture anywhere on controls/canvas
    const unlockOnGesture = () => {
      if (this.audioCtx?.state === "suspended") {
        this.resumeAudio();
      }
    };
    window.addEventListener("pointerdown", unlockOnGesture, { passive: true });
    window.addEventListener("keydown", unlockOnGesture, { passive: true });
  }

  isDisabled(btn) {
    return btn.getAttribute("aria-disabled") === "true" || btn.disabled;
  }

  setDisabled(btn, disabled) {
    btn.setAttribute("aria-disabled", String(disabled));
  }

  setTouchDeckVisible(visible, persist) {
    this.el.touchDeck.hidden = !visible;
    this.el.setTouchControls.checked = visible;
    this.el.btnQuickTouch.setAttribute("aria-pressed", String(visible));
    if (persist) {
      this.prefs.touchControls = visible;
      savePrefs(this.prefs);
    }
    this.updateCanvasSize();
  }

  showError(title, message) {
    this.el.errorTitle.textContent = title;
    this.el.errorMessage.textContent = message;
    this.el.errorBanner.hidden = false;
    this.announce(`${title}: ${message}`);
  }

  clearError() {
    this.el.errorBanner.hidden = true;
  }

  announce(text) {
    this.el.srStatus.textContent = text;
  }

  notify(text) {
    this.announce(text);
    this.el.toast.textContent = text;
    this.el.toast.hidden = false;
    clearTimeout(this.toastTimer);
    this.toastTimer = setTimeout(() => {
      this.el.toast.hidden = true;
    }, 2400);
  }

  async init() {
    try {
      this.wasm = await initWasm();
      this.emu = new Emulator();
      this.emu.set_color_profile(this.prefs.colorProfile);
      this.emu.set_lcd_ghosting(this.prefs.lcdGhosting);
      this.emu.set_volume(this.prefs.volume);
      this.emu.set_speed(this.prefs.speed);

      const savedBios = await this.storage.loadBios().catch(() => null);
      if (savedBios) {
        try {
          this.emu.load_bios(savedBios);
          this.updateBiosUi();
        } catch {
          await this.storage.clearBios().catch(() => {});
        }
      }

      this.el.statAudio.textContent = `${(this.emu.sample_rate() / 1000).toFixed(2)} kHz`;
      this.el.stateLoading.hidden = true;
      this.el.stateEmpty.hidden = false;
      this.updatePowerState();
      this.updateCanvasSize();
      this.startLoop();
    } catch (err) {
      this.el.stateLoading.hidden = true;
      this.el.stateEmpty.hidden = false;
      const hint =
        window.location.protocol === "file:"
          ? " Open via a local HTTP server (e.g. `npx serve web`) rather than file://."
          : " Ensure `web/pkg/` was built with wasm-bindgen.";
      this.showError(
        "WebAssembly Initialization Failed",
        `${err?.message || err}.${hint}`,
      );
      this.el.powerIndicator.dataset.state = "error";
      this.el.powerLabel.textContent = "ERROR";
    }
  }

  async ensureAudioStarted() {
    if (!this.emu) return;
    if (!this.audioCtx) {
      try {
        const Ctx = window.AudioContext || window.webkitAudioContext;
        if (!Ctx) {
          this.el.statAudio.textContent = "Unsupported";
          return;
        }
        this.audioCtx = new Ctx({ latencyHint: "interactive" });
        await this.audioCtx.audioWorklet.addModule("./audio-worklet.js");
        this.audioNode = new AudioWorkletNode(this.audioCtx, "gba-audio-processor", {
          numberOfInputs: 0,
          numberOfOutputs: 1,
          outputChannelCount: [2],
        });
        this.audioNode.connect(this.audioCtx.destination);
        this.audioNode.port.postMessage({
          type: "config",
          inputSampleRate: this.emu.sample_rate(),
        });
        this.audioNode.port.postMessage({ type: "mute", muted: this.prefs.muted });
        this.audioNode.port.postMessage({ type: "volume", volume: this.prefs.volume / 100 });
        this.audioWorkletReady = true;
        this.audioCtx.addEventListener("statechange", () => this.updateAudioUi());
      } catch (err) {
        this.el.statAudio.textContent = "Audio Unavailable";
        this.showError(
          "AudioWorklet Unavailable",
          `Running muted: ${err?.message || err}. Secure context (HTTPS or localhost) is required for WebAudio worklets.`,
        );
      }
    }
    if (this.audioCtx?.state === "suspended") {
      await this.audioCtx.resume().catch(() => {});
    }
    this.updateAudioUi();
  }

  async resumeAudio() {
    if (!this.audioCtx) {
      await this.ensureAudioStarted();
      return;
    }
    try {
      await this.audioCtx.resume();
    } catch {
      // Ignore until valid user gesture.
    }
    this.updateAudioUi();
  }

  updateAudioUi() {
    if (!this.emu) return;
    const rateK = `${(this.emu.sample_rate() / 1000).toFixed(2)} kHz`;
    if (!this.audioCtx) {
      this.el.statAudio.textContent = `${rateK} · Idle`;
      this.el.btnUnlockAudio.hidden = true;
      return;
    }
    const blocked = this.audioCtx.state === "suspended" && this.emu.rom_loaded;
    this.el.btnUnlockAudio.hidden = !blocked;
    if (this.prefs.muted) {
      this.el.statAudio.textContent = `${rateK} · Muted`;
    } else if (blocked) {
      this.el.statAudio.textContent = `${rateK} · Blocked`;
    } else {
      this.el.statAudio.textContent = `${rateK} · Active`;
    }
  }

  async handleDroppedOrPickedFile(file) {
    const lower = file.name.toLowerCase();
    if (lower.endsWith(".sav") || lower.endsWith(".srm")) {
      await this.importSramFile(file);
      return;
    }
    if (lower.endsWith(".state") || lower.endsWith(".gbas")) {
      await this.importStateFile(file);
      return;
    }
    await this.loadRomFile(file);
  }

  async loadRomFile(file) {
    if (!this.emu) return;
    this.clearError();
    this.el.stateEmpty.hidden = true;
    this.el.stateRunning.hidden = true;
    this.el.stateLoading.hidden = false;
    this.el.loadingCaption.textContent = `Loading ${file.name}…`;

    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      await this.bootRomBytes(bytes, file.name);
    } catch (err) {
      this.el.stateLoading.hidden = true;
      if (!this.emu.rom_loaded) {
        this.el.stateEmpty.hidden = false;
      } else {
        this.el.stateRunning.hidden = false;
      }
      this.showError("ROM Load Failed", err?.message || String(err));
    }
  }

  async loadBuiltInDemo() {
    if (!this.emu) return;
    this.clearError();
    try {
      const rom = createBuiltInTestRom();
      await this.bootRomBytes(rom, "Built-in Demo");
    } catch (err) {
      this.showError("Failed to Boot Demo ROM", err?.message || String(err));
    }
  }

  async bootRomBytes(bytes, label) {
    await this.flushSramIfDirty();
    this.emu.load_rom(bytes);

    // Hydrate battery save from IndexedDB if available
    const key = this.emu.storage_key;
    try {
      const saved = await this.storage.loadSram(key);
      if (saved && saved.length > 0) {
        this.emu.import_sram(saved);
      }
    } catch (err) {
      this.notify(`IndexedDB save read skipped: ${err?.message || err}`);
    }

    this.userPaused = false;
    this.bgPaused = false;
    this.lastFrameTs = performance.now();
    this.accMs = 0;

    this.el.stateLoading.hidden = true;
    this.el.stateEmpty.hidden = true;
    this.el.stateRunning.hidden = false;
    this.el.pausedScrim.hidden = true;

    this.setDisabled(this.el.btnPause, false);
    this.setDisabled(this.el.btnReset, false);
    this.setDisabled(this.el.btnFastForward, false);
    this.setDisabled(this.el.btnStates, false);

    this.el.cartBadge.textContent = `${this.emu.title} · ${this.emu.game_code}`;
    this.el.statCrc.textContent = this.emu.rom_crc32_hex;
    this.el.statSave.textContent = `${this.emu.save_type} · Synced`;
    this.updateBiosUi();
    this.updatePowerState();
    this.updateCanvasSize();

    this.emu.run_frame();
    this.presentFrame();
    this.canvas.focus();

    await this.ensureAudioStarted();
    this.audioNode?.port.postMessage({ type: "clear" });
    this.notify(`Loaded ${this.emu.title || label}`);
  }

  async loadBiosFile(file) {
    if (!this.emu) return;
    this.clearError();
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      this.emu.load_bios(bytes);
      await this.storage.saveBios(bytes).catch(() => {});
      this.updateBiosUi();
      this.audioNode?.port.postMessage({ type: "clear" });
      if (this.emu.rom_loaded) this.presentFrame();
      this.notify("Loaded 16 KiB LLE GBA BIOS");
    } catch (err) {
      this.showError("Invalid BIOS File", err?.message || String(err));
    }
  }

  async clearBios() {
    if (!this.emu) return;
    this.emu.clear_bios();
    await this.storage.clearBios().catch(() => {});
    this.updateBiosUi();
    this.audioNode?.port.postMessage({ type: "clear" });
    if (this.emu.rom_loaded) this.presentFrame();
    this.notify("Switched to built-in HLE BIOS");
  }

  updateBiosUi() {
    const lle = Boolean(this.emu?.bios_loaded);
    this.el.statBios.textContent = lle ? "LLE 16 KiB" : "HLE";
    this.el.biosStatusText.textContent = lle
      ? "Active BIOS: User 16 KiB GBA BIOS (stored locally in IndexedDB)"
      : "Active BIOS: Built-in HLE BIOS";
  }

  togglePause(forceState) {
    if (!this.emu?.rom_loaded) return;
    this.userPaused = typeof forceState === "boolean" ? forceState : !this.userPaused;
    this.el.btnPause.setAttribute("aria-pressed", String(this.userPaused));
    this.el.pausedScrim.hidden = !this.userPaused;
    if (this.userPaused) {
      this.audioNode?.port.postMessage({ type: "clear" });
      this.flushSramIfDirty();
    } else {
      this.lastFrameTs = performance.now();
      this.accMs = 0;
    }
    this.updatePowerState();
  }

  resetConsole() {
    if (!this.emu?.rom_loaded) return;
    this.flushSramIfDirty();
    this.emu.reset();
    this.audioNode?.port.postMessage({ type: "clear" });
    this.presentFrame();
    this.accMs = 0;
    this.notify("Console reset");
  }

  setSpeed(speed) {
    const clamped = Math.max(0.25, Math.min(8, Number(speed) || 1));
    this.prefs.speed = clamped;
    this.el.setSpeed.value = String(clamped);
    savePrefs(this.prefs);
    this.emu?.set_speed(clamped);
    this.updateSpeedUi();
    this.updatePowerState();
  }

  toggleFastForward() {
    this.setSpeed(this.prefs.speed > 1.05 ? 1 : 2);
  }

  updateSpeedUi() {
    const fast = this.prefs.speed > 1.05;
    this.el.btnFastForward.setAttribute("aria-pressed", String(fast));
    this.el.speedLabel.textContent = `${this.prefs.speed}×`;
  }

  toggleMute() {
    this.prefs.muted = !this.prefs.muted;
    this.el.btnMute.setAttribute("aria-pressed", String(this.prefs.muted));
    savePrefs(this.prefs);
    this.audioNode?.port.postMessage({ type: "mute", muted: this.prefs.muted });
    this.updateAudioUi();
  }

  async toggleFullscreen() {
    try {
      if (document.fullscreenElement === this.el.stageShell) {
        await document.exitFullscreen();
      } else {
        await this.el.stageShell.requestFullscreen();
      }
    } catch (err) {
      this.showError("Fullscreen Unavailable", err?.message || String(err));
    }
  }

  updatePowerState() {
    if (!this.emu?.rom_loaded) {
      this.el.powerIndicator.dataset.state = "idle";
      this.el.powerLabel.textContent = "IDLE";
    } else if (this.userPaused) {
      this.el.powerIndicator.dataset.state = "paused";
      this.el.powerLabel.textContent = "PAUSED";
    } else if (this.prefs.speed > 1.05) {
      this.el.powerIndicator.dataset.state = "turbo";
      this.el.powerLabel.textContent = `${this.prefs.speed}× TURBO`;
    } else {
      this.el.powerIndicator.dataset.state = "running";
      this.el.powerLabel.textContent = "RUNNING";
    }
  }

  updateCanvasSize() {
    const stage = this.el.screenStage;
    const availW = Math.max(GBA_WIDTH, stage.clientWidth - 18);
    const availH = Math.max(GBA_HEIGHT, stage.clientHeight - 18);
    const rawScale = Math.min(availW / GBA_WIDTH, availH / GBA_HEIGHT);
    const scale = this.prefs.integerScale ? Math.max(1, Math.floor(rawScale)) : Math.max(1, rawScale);

    const w = Math.round(GBA_WIDTH * scale);
    const h = Math.round(GBA_HEIGHT * scale);

    this.el.canvasWrap.style.setProperty("--canvas-w", `${w}px`);
    this.el.canvasWrap.style.setProperty("--canvas-h", `${h}px`);
    this.el.canvasWrap.style.setProperty("--px-scale", `${Math.max(2, scale)}px`);

    // Hide pixel grid when scale is < 2x so pixels aren't obscured
    this.el.lcdGridOverlay.hidden = !(this.prefs.lcdGrid && scale >= 2);
  }

  presentFrame() {
    if (!this.emu || !this.wasm) return;
    const ptr = this.emu.framebuffer_ptr();
    const len = this.emu.framebuffer_len();
    const buf = this.wasm.memory.buffer;
    if (
      !this.frameBufferView ||
      this.cachedWasmBuffer !== buf ||
      this.frameBufferView.byteOffset !== ptr ||
      this.frameBufferView.length !== len
    ) {
      this.cachedWasmBuffer = buf;
      this.frameBufferView = new Uint8ClampedArray(buf, ptr, len);
    }
    this.imageData.data.set(this.frameBufferView);
    this.ctx.putImageData(this.imageData, 0, 0);
  }

  pushAudioSamples() {
    if (!this.emu || !this.wasm) return;
    const len = this.emu.audio_len();
    if (len === 0) return;

    if (this.audioWorkletReady && this.audioNode && this.audioCtx?.state === "running") {
      const ptr = this.emu.audio_ptr();
      const view = new Int16Array(this.wasm.memory.buffer, ptr, len);
      const copy = new Int16Array(view);
      this.audioNode.port.postMessage({ type: "samples", buffer: copy.buffer }, [copy.buffer]);
    }
    this.emu.clear_audio();
  }

  startLoop() {
    this.lastFrameTs = performance.now();
    const tick = (now) => {
      this.rafId = requestAnimationFrame(tick);
      if (!this.emu?.rom_loaded || this.userPaused || this.bgPaused) {
        this.lastFrameTs = now;
        return;
      }

      const delta = Math.min(Math.max(0, now - this.lastFrameTs), 100);
      this.lastFrameTs = now;

      const speed = this.emu.speed();
      const stepMs = GBA_FRAME_MS / speed;
      this.accMs += delta;

      const mask = this.input.getCombinedMask();
      this.emu.set_keys(mask);

      const maxSteps = Math.max(4, Math.ceil(speed * 2));
      let stepped = 0;
      while (this.accMs >= stepMs && stepped < maxSteps) {
        this.emu.run_frame();
        this.accMs -= stepMs;
        stepped += 1;
        this.fpsFrames += 1;
      }
      if (stepped === maxSteps && this.accMs > stepMs * 2) {
        this.accMs = 0;
      }

      if (stepped > 0) {
        this.presentFrame();
        this.pushAudioSamples();
      }

      if (now - this.fpsWindowStart >= 500) {
        const fps = (this.fpsFrames * 1000) / (now - this.fpsWindowStart);
        this.el.statFps.textContent = `${fps.toFixed(1)} fps`;
        this.fpsFrames = 0;
        this.fpsWindowStart = now;
      }

      if (now - this.lastAutosaveTs >= SRAM_AUTOSAVE_MS) {
        this.lastAutosaveTs = now;
        this.flushSramIfDirty();
      }
    };
    this.rafId = requestAnimationFrame(tick);
  }

  async flushSramIfDirty() {
    if (!this.emu?.rom_loaded || !this.emu.sram_dirty()) return;
    const data = this.emu.export_sram();
    if (data.length === 0) return;

    try {
      await this.storage.saveSram(
        this.emu.storage_key,
        {
          title: this.emu.title,
          gameCode: this.emu.game_code,
          crc32Hex: this.emu.rom_crc32_hex,
          saveType: this.emu.save_type,
        },
        data,
      );
      this.emu.clear_sram_dirty();
      this.el.statSave.textContent = `${this.emu.save_type} · Synced`;
    } catch {
      this.el.statSave.textContent = `${this.emu.save_type} · Unsaved`;
    }
  }

  downloadBytes(filename, bytes) {
    const blob = new Blob([bytes], { type: "application/octet-stream" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = filename;
    document.body.append(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  downloadSramFile() {
    if (!this.emu?.rom_loaded) {
      this.showError("No ROM Loaded", "Load a game before exporting battery save data.");
      return;
    }
    const data = this.emu.export_sram();
    if (data.length === 0) {
      this.notify("Current ROM has no detected battery save memory");
      return;
    }
    const safeTitle = (this.emu.title || "save").replace(/[^a-zA-Z0-9_-]+/g, "_");
    this.downloadBytes(`${safeTitle}.sav`, data);
    this.notify("Exported .sav file");
  }

  async importSramFile(file) {
    if (!this.emu?.rom_loaded) {
      this.showError("No ROM Loaded", "Load the matching .gba ROM before importing a .sav file.");
      return;
    }
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      this.emu.import_sram(bytes);
      await this.storage.saveSram(
        this.emu.storage_key,
        {
          title: this.emu.title,
          gameCode: this.emu.game_code,
          crc32Hex: this.emu.rom_crc32_hex,
          saveType: this.emu.save_type,
        },
        bytes,
      );
      this.emu.clear_sram_dirty();
      this.notify("Imported .sav into battery memory");
    } catch (err) {
      this.showError("Save Import Failed", err?.message || String(err));
    }
  }

  async openStatesDialog() {
    if (!this.emu?.rom_loaded) return;
    this.el.statesGameLabel.textContent = `${this.emu.title} (${this.emu.storage_key})`;
    await this.renderStateSlots();
    this.el.statesDialog.showModal();
  }

  async renderStateSlots() {
    const key = this.emu.storage_key;
    let existing = [];
    try {
      existing = await this.storage.listSaveStates(key);
    } catch {
      existing = [];
    }
    const bySlot = new Map(existing.map((r) => [r.slot, r]));
    this.el.slotsList.innerHTML = "";

    for (let slot = 1; slot <= 4; slot++) {
      const rec = bySlot.get(slot);
      const row = document.createElement("div");
      row.className = "slot-row";

      if (rec?.thumbnail) {
        const img = document.createElement("img");
        img.className = "slot-thumb";
        img.src = rec.thumbnail;
        img.alt = `Slot ${slot} preview`;
        row.append(img);
      } else {
        const placeholder = document.createElement("div");
        placeholder.className = "slot-thumb";
        placeholder.textContent = "EMPTY";
        row.append(placeholder);
      }

      const meta = document.createElement("div");
      meta.className = "slot-meta";
      const title = document.createElement("div");
      title.className = "slot-title";
      title.textContent = `Slot ${slot}`;
      const time = document.createElement("div");
      time.className = "slot-time";
      time.textContent = rec ? new Date(rec.savedAt).toLocaleString() : "No saved state";
      meta.append(title, time);
      row.append(meta);

      const actions = document.createElement("div");
      actions.className = "slot-actions";

      const saveBtn = document.createElement("button");
      saveBtn.type = "button";
      saveBtn.className = "ctrl-btn";
      saveBtn.textContent = "Save";
      saveBtn.addEventListener("click", async () => {
        try {
          const bytes = this.emu.save_state();
          const thumb = this.canvas.toDataURL("image/png");
          await this.storage.saveStateSlot(key, slot, bytes, thumb, this.emu.title);
          await this.renderStateSlots();
          this.notify(`Saved state to Slot ${slot}`);
        } catch (err) {
          this.showError("Save State Failed", err?.message || String(err));
        }
      });
      actions.append(saveBtn);

      if (rec) {
        const loadBtn = document.createElement("button");
        loadBtn.type = "button";
        loadBtn.className = "ctrl-btn ctrl-btn-primary";
        loadBtn.textContent = "Load";
        loadBtn.addEventListener("click", async () => {
          try {
            this.emu.load_state(rec.data);
            this.presentFrame();
            this.audioNode?.port.postMessage({ type: "clear" });
            this.el.statesDialog.close();
            this.notify(`Loaded state from Slot ${slot}`);
          } catch (err) {
            this.showError("Load State Failed", err?.message || String(err));
          }
        });

        const delBtn = document.createElement("button");
        delBtn.type = "button";
        delBtn.className = "ctrl-btn ctrl-btn-ghost";
        delBtn.textContent = "Delete";
        delBtn.setAttribute("aria-label", `Delete Slot ${slot}`);
        delBtn.addEventListener("click", async () => {
          await this.storage.deleteStateSlot(key, slot);
          await this.renderStateSlots();
          this.notify(`Deleted Slot ${slot}`);
        });

        actions.append(loadBtn, delBtn);
      }

      row.append(actions);
      this.el.slotsList.append(row);
    }
  }

  exportStateFile() {
    if (!this.emu?.rom_loaded) return;
    try {
      const bytes = this.emu.save_state();
      const safeTitle = (this.emu.title || "state").replace(/[^a-zA-Z0-9_-]+/g, "_");
      this.downloadBytes(`${safeTitle}.state`, bytes);
      this.notify("Exported .state file");
    } catch (err) {
      this.showError("State Export Failed", err?.message || String(err));
    }
  }

  async importStateFile(file) {
    if (!this.emu?.rom_loaded) {
      this.showError("No ROM Loaded", "Load a ROM before loading a save state file.");
      return;
    }
    try {
      const bytes = new Uint8Array(await file.arrayBuffer());
      this.emu.load_state(bytes);
      this.presentFrame();
      this.audioNode?.port.postMessage({ type: "clear" });
      this.el.statesDialog.close();
      this.notify(`Loaded state from ${file.name}`);
    } catch (err) {
      this.showError("Invalid Save State", err?.message || String(err));
    }
  }

  renderKeybindingsTable() {
    this.el.keybindingsGrid.innerHTML = "";
    for (const [btnKey, label] of Object.entries(KEY_LABELS)) {
      const row = document.createElement("div");
      row.className = "keybind-row";

      const name = document.createElement("span");
      name.textContent = label;

      const btn = document.createElement("button");
      btn.type = "button";
      btn.className = "ctrl-btn keybind-btn";
      btn.textContent = formatKeyCode(this.input.bindings[btnKey]);
      btn.addEventListener("click", () => {
        this.input.rebindingButton = btnKey;
        btn.textContent = "Press key…";
        this.input.onRebindComplete = () => {
          this.renderKeybindingsTable();
        };
      });

      row.append(name, btn);
      this.el.keybindingsGrid.append(row);
    }
  }
}

window.addEventListener("DOMContentLoaded", () => {
  const app = new RustboyWebApp();
  app.init();
});
