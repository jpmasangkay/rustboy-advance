/**
 * IndexedDB persistence for battery saves (SRAM/Flash/EEPROM), save-state slots,
 * and optional user-provided GBA BIOS image.
 */

const DB_NAME = "rustboy_advance_db";
const DB_VERSION = 1;
const STORE_SRAM = "sram";
const STORE_STATES = "savestates";
const STORE_ASSETS = "assets";
const BIOS_ASSET_KEY = "gba_bios_bin";

function openDb() {
  return new Promise((resolve, reject) => {
    if (!("indexedDB" in globalThis)) {
      reject(new Error("IndexedDB is unavailable in this browser context."));
      return;
    }
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_SRAM)) {
        db.createObjectStore(STORE_SRAM, { keyPath: "key" });
      }
      if (!db.objectStoreNames.contains(STORE_STATES)) {
        const states = db.createObjectStore(STORE_STATES, { keyPath: "id" });
        states.createIndex("by_game", "storageKey", { unique: false });
      }
      if (!db.objectStoreNames.contains(STORE_ASSETS)) {
        db.createObjectStore(STORE_ASSETS, { keyPath: "key" });
      }
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("Failed to open IndexedDB."));
  });
}

function txRequest(db, storeName, mode, op) {
  return new Promise((resolve, reject) => {
    const tx = db.transaction(storeName, mode);
    const store = tx.objectStore(storeName);
    const req = op(store);
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error ?? new Error("IndexedDB request failed."));
  });
}

export class EmulatorStorage {
  constructor() {
    this.dbPromise = null;
  }

  async db() {
    if (!this.dbPromise) {
      this.dbPromise = openDb().catch((err) => {
        this.dbPromise = null;
        throw err;
      });
    }
    return this.dbPromise;
  }

  async loadSram(storageKey) {
    const db = await this.db();
    const record = await txRequest(db, STORE_SRAM, "readonly", (s) => s.get(storageKey));
    return record?.data ? new Uint8Array(record.data) : null;
  }

  async saveSram(storageKey, meta, bytes) {
    const db = await this.db();
    const payload = {
      key: storageKey,
      title: meta.title,
      gameCode: meta.gameCode,
      crc32: meta.crc32Hex,
      saveType: meta.saveType,
      data: new Uint8Array(bytes),
      updatedAt: Date.now(),
    };
    await txRequest(db, STORE_SRAM, "readwrite", (s) => s.put(payload));
  }

  async listSaveStates(storageKey) {
    const db = await this.db();
    return new Promise((resolve, reject) => {
      const tx = db.transaction(STORE_STATES, "readonly");
      const idx = tx.objectStore(STORE_STATES).index("by_game");
      const req = idx.getAll(IDBKeyRange.only(storageKey));
      req.onsuccess = () => {
        const list = (req.result || []).sort((a, b) => a.slot - b.slot);
        resolve(list);
      };
      req.onerror = () => reject(req.error ?? new Error("Failed to list save states."));
    });
  }

  async saveStateSlot(storageKey, slot, bytes, thumbnailDataUrl, title) {
    const db = await this.db();
    const record = {
      id: `${storageKey}::slot_${slot}`,
      storageKey,
      slot,
      title,
      savedAt: Date.now(),
      thumbnail: thumbnailDataUrl || "",
      data: new Uint8Array(bytes),
    };
    await txRequest(db, STORE_STATES, "readwrite", (s) => s.put(record));
    return record;
  }

  async loadStateSlot(storageKey, slot) {
    const db = await this.db();
    const record = await txRequest(db, STORE_STATES, "readonly", (s) =>
      s.get(`${storageKey}::slot_${slot}`),
    );
    return record?.data ? new Uint8Array(record.data) : null;
  }

  async deleteStateSlot(storageKey, slot) {
    const db = await this.db();
    await txRequest(db, STORE_STATES, "readwrite", (s) =>
      s.delete(`${storageKey}::slot_${slot}`),
    );
  }

  async saveBios(bytes) {
    const db = await this.db();
    await txRequest(db, STORE_ASSETS, "readwrite", (s) =>
      s.put({ key: BIOS_ASSET_KEY, data: new Uint8Array(bytes), updatedAt: Date.now() }),
    );
  }

  async loadBios() {
    const db = await this.db();
    const record = await txRequest(db, STORE_ASSETS, "readonly", (s) => s.get(BIOS_ASSET_KEY));
    return record?.data ? new Uint8Array(record.data) : null;
  }

  async clearBios() {
    const db = await this.db();
    await txRequest(db, STORE_ASSETS, "readwrite", (s) => s.delete(BIOS_ASSET_KEY));
  }
}
