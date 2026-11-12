/**
 * Generates an original minimal GBA ARM7TDMI test ROM in memory so users can verify
 * video rendering, color profiles, LCD grid, save states, keypad input, and PSG audio
 * immediately without needing an external ROM file.
 */

export function createBuiltInTestRom() {
  const rom = new Uint8Array(0x400);
  const view = new DataView(rom.buffer);

  // Header metadata
  const title = "RUSTBOY DEMO";
  for (let i = 0; i < title.length; i++) rom[0xa0 + i] = title.charCodeAt(i);
  const code = "RBDM";
  for (let i = 0; i < code.length; i++) rom[0xac + i] = code.charCodeAt(i);
  rom[0xb2] = 0x96;
  const saveTag = "SRAM_V110";
  for (let i = 0; i < saveTag.length; i++) rom[0x300 + i] = saveTag.charCodeAt(i);

  const words = [];
  const emit = (w) => {
    words.push(w >>> 0);
    return words.length - 1;
  };
  const patchBranch = (idx, cond, targetIdx) => {
    const off = (targetIdx - (idx + 2)) & 0x00ff_ffff;
    words[idx] = ((cond << 28) | 0x0a00_0000 | off) >>> 0;
  };

  // ARMv4T instruction encoders (registers 0..15)
  const movImm = (rd, imm8, rot = 0) =>
    (0xe3a0_0000 | (rd << 12) | ((rot & 0xf) << 8) | (imm8 & 0xff)) >>> 0;
  const orrImm = (rd, rn, imm8, rot = 0) =>
    (0xe380_0000 | (rn << 16) | (rd << 12) | ((rot & 0xf) << 8) | (imm8 & 0xff)) >>> 0;
  const orrRegLsl16 = (rd, rn, rm) =>
    (0xe180_0800 | (rn << 16) | (rd << 12) | rm) >>> 0;
  const addImm = (rd, rn, imm8, rot = 0) =>
    (0xe280_0000 | (rn << 16) | (rd << 12) | ((rot & 0xf) << 8) | (imm8 & 0xff)) >>> 0;
  const addReg = (rd, rn, rm) => (0xe080_0000 | (rn << 16) | (rd << 12) | rm) >>> 0;
  const andImm = (rd, rn, imm8, rot = 0) =>
    (0xe200_0000 | (rn << 16) | (rd << 12) | ((rot & 0xf) << 8) | (imm8 & 0xff)) >>> 0;
  const tstImm = (rn, imm8, rot = 0) =>
    (0xe310_0000 | (rn << 16) | ((rot & 0xf) << 8) | (imm8 & 0xff)) >>> 0;
  const cmpImm = (rn, imm8) => (0xe350_0000 | (rn << 16) | (imm8 & 0xff)) >>> 0;
  const cmpReg = (rn, rm) => (0xe150_0000 | (rn << 16) | rm) >>> 0;
  const mvnReg = (rd, rm) => (0xe1e0_0000 | (rd << 12) | rm) >>> 0;
  const strWordOff = (rd, rn, off) =>
    (0xe580_0000 | (rn << 16) | (rd << 12) | (off & 0xfff)) >>> 0;
  const strhOff = (rd, rn, off) =>
    (0xe1c0_00b0 | (rn << 16) | (rd << 12) | (((off >> 4) & 0xf) << 8) | (off & 0xf)) >>> 0;
  const ldrhOff = (rd, rn, off) =>
    (0xe1d0_00b0 | (rn << 16) | (rd << 12) | (((off >> 4) & 0xf) << 8) | (off & 0xf)) >>> 0;
  const strbOff = (rd, rn, off) =>
    (0xe5c0_0000 | (rn << 16) | (rd << 12) | (off & 0xfff)) >>> 0;
  const lslImm = (rd, rm, sh) => (0xe1a0_0000 | (rd << 12) | ((sh & 31) << 7) | rm) >>> 0;
  const lsrImm = (rd, rm, sh) => (0xe1a0_0020 | (rd << 12) | ((sh & 31) << 7) | rm) >>> 0;

  // Code begins at 0x0800_00C8 (branch from 0x0800_0000 is 0xEA000030)
  view.setUint32(0, 0xea00_0030, true);

  // R0 = 0x0400_0000 (I/O base)
  emit(movImm(0, 0x04, 4));
  // DISPCNT (0x00) = 0x0403 (Mode 3 + BG2)
  emit(movImm(1, 0x04, 12)); // 0x0400
  emit(orrImm(1, 1, 0x03));
  emit(strhOff(1, 0, 0x00));

  // Enable PSG master audio: SOUNDCNT_X (0x84) = 0x80, SOUNDCNT_L (0x80) = 0xFF77, SOUNDCNT_H (0x82) = 0x0002
  emit(movImm(1, 0x80));
  emit(strhOff(1, 0, 0x84));
  emit(movImm(1, 0xff, 12)); // 0xFF00
  emit(orrImm(1, 1, 0x77));
  emit(strhOff(1, 0, 0x80));
  emit(movImm(1, 0x02));
  emit(strhOff(1, 0, 0x82));

  // R12 = frame counter = 0
  emit(movImm(12, 0));

  // Main frame loop:
  const frameLoop = emit(addImm(12, 12, 1));

  // Wait for VCOUNT == 160 so each gradient step advances at 59.7275 Hz without tearing
  const waitLeaveVblank = emit(ldrhOff(1, 0, 0x06));
  emit(cmpImm(1, 160));
  const brLeaveVblank = emit(0); // BGE waitLeaveVblank
  patchBranch(brLeaveVblank, 0xa, waitLeaveVblank);

  const waitVblank = emit(ldrhOff(1, 0, 0x06));
  emit(cmpImm(1, 160));
  const brWaitVblank = emit(0); // BLT waitVblank
  patchBranch(brWaitVblank, 0xb, waitVblank);

  // Read KEYINPUT at 0x0400_0130 via R2 = R0 + 0x100, then [R2 + 0x30]
  // R11 = (~KEYINPUT) & 0x03FF
  emit(addImm(2, 0, 1, 12)); // 0x0400_0100
  emit(ldrhOff(11, 2, 0x30));
  emit(mvnReg(11, 11));
  emit(movImm(2, 0x03, 12)); // 0x0300
  emit(orrImm(2, 2, 0xff)); // 0x03FF
  emit(0xe00b_b002); // AND R11, R11, R2

  // Gate PSG1 tone on A (bit 0) or B (bit 1)
  emit(tstImm(11, 0x03));
  const brMuteAudio = emit(0); // BEQ muteAudio
  // Only trigger if channel 1 isn't already sounding (SOUNDCNT_X bit 0 == 0)
  emit(ldrhOff(1, 0, 0x84));
  emit(tstImm(1, 0x01));
  const brKeepAudio = emit(0); // BNE setupVram
  emit(movImm(1, 0xf2, 12)); // env vol=15, dec, step=2 -> 0xF200
  emit(orrImm(1, 1, 0x80)); // 50% duty -> 0xF280
  emit(strhOff(1, 0, 0x62)); // SOUND1CNT_H
  emit(movImm(1, 0x87, 12)); // trigger + freq high -> 0x8700
  emit(orrImm(1, 1, 0x20));
  emit(strhOff(1, 0, 0x64)); // SOUND1CNT_X
  const brAudioDone = emit(0); // B setupVram

  const muteAudio = emit(movImm(1, 0x00));
  emit(strhOff(1, 0, 0x62)); // DAC off cleanly silences PSG1

  // Setup VRAM cursors: R2 = 0x0600_0000 (row y), R3 = 0x0600_0000 + 480 (row y+1)
  const setupVram = emit(movImm(2, 0x06, 4));
  patchBranch(brMuteAudio, 0x0, muteAudio);
  patchBranch(brKeepAudio, 0x1, setupVram);
  patchBranch(brAudioDone, 0xe, setupVram);
  emit(addImm(3, 2, 0x78, 15)); // +480 bytes (one Mode 3 scanline)

  // Write a signature byte to SRAM (0x0E00_0000) when Start (bit 3) is pressed
  emit(tstImm(11, 0x08));
  const brNoSram = emit(0);
  emit(movImm(4, 0x0e, 4)); // 0x0E00_0000
  emit(andImm(5, 12, 0xff));
  emit(strbOff(5, 4, 0));
  const afterSram = emit(movImm(4, 240)); // R4 = width
  patchBranch(brNoSram, 0x0, afterSram);

  emit(movImm(6, 160)); // R6 = height
  emit(movImm(5, 0)); // R5 = y = 0

  // Outer Y loop (steps y += 2 so one full pass completes inside 1 GBA frame):
  const yLoop = emit(movImm(7, 0)); // R7 = x = 0

  // Inner X loop (writes 2x2 pixel block with 32-bit STR):
  const xLoop = emit(addReg(8, 7, 12)); // R8 = x + frame
  emit(lsrImm(8, 8, 3));
  emit(andImm(8, 8, 0x1f)); // r5

  emit(addReg(9, 5, 12)); // R9 = y + frame
  emit(lsrImm(9, 9, 3));
  emit(andImm(9, 9, 0x1f));
  emit(lslImm(9, 9, 5)); // g5 << 5

  emit(0xe027_a005); // EOR R10, R7, R5
  emit(lsrImm(10, 10, 3));
  emit(andImm(10, 10, 0x1f));
  emit(lslImm(10, 10, 10)); // b5 << 10

  emit(0xe188_8009); // ORR R8, R8, R9
  emit(0xe188_800a); // ORR R8, R8, R10

  // Bottom bar (y >= 136): dark neutral track, bright emerald when any button held
  emit(cmpImm(5, 136));
  const brPixelStore = emit(0); // BLT storeBlock
  emit(movImm(8, 0x18, 12)); // 0x18C6 dark slate
  emit(orrImm(8, 8, 0xc6));
  emit(cmpImm(11, 0));
  const brStore = emit(0); // BEQ storeBlock
  emit(movImm(8, 0x03, 12)); // 0x03EA bright emerald
  emit(orrImm(8, 8, 0xea));

  const storeBlock = emit(orrRegLsl16(8, 8, 8)); // pack two identical 16-bit pixels
  patchBranch(brPixelStore, 0xb, storeBlock);
  patchBranch(brStore, 0x0, storeBlock);

  emit(strWordOff(8, 2, 0)); // (x..x+1, y)
  emit(strWordOff(8, 3, 0)); // (x..x+1, y+1)

  emit(addImm(2, 2, 4));
  emit(addImm(3, 3, 4));
  emit(addImm(7, 7, 2)); // x += 2
  emit(cmpReg(7, 4));
  const brXLoop = emit(0); // BLT xLoop
  patchBranch(brXLoop, 0xb, xLoop);

  emit(addImm(2, 2, 0x78, 15)); // skip odd row already drawn
  emit(addImm(3, 3, 0x78, 15));
  emit(addImm(5, 5, 2)); // y += 2
  emit(cmpReg(5, 6));
  const brYLoop = emit(0); // BLT yLoop
  patchBranch(brYLoop, 0xb, yLoop);

  const brFrameLoop = emit(0); // B frameLoop
  patchBranch(brFrameLoop, 0xe, frameLoop);

  for (let i = 0; i < words.length; i++) {
    view.setUint32(0xc8 + i * 4, words[i], true);
  }
  return rom;
}
