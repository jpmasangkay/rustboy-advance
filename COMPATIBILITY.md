# Game & Test Suite Compatibility

Rustboy Advance tracks compatibility across two tiers:
1. **Automated Hardware & Test-ROM Suites** (executed locally and in CI via `cargo run -p gba-testrunner` and `cargo test -p gb-core`). See [`TEST_RESULTS.md`](TEST_RESULTS.md) for the exact per-ROM matrix and failure diagnosis.
2. **Commercial GBA Database (`GAME_DB` & `REAL_BIOS_FALLBACK_GAMES`)** in [`crates/gba-core/src/gamedb.rs`](crates/gba-core/src/gamedb.rs).

> **Verification Policy:** No copyrighted commercial ROMs are included or executed in this repository's CI. The commercial table below documents the **verified cartridge database configuration** (save-type override, RTC/sensor wiring, and real-BIOS fallback recommendation). Live commercial gameplay status is listed honestly as **Pending Hardware/Community Verification** unless verified by community test reports on an official release tag.

---

## Automated Test Suite Summary (Observed Locally)

| Suite / Runner | Command | Observed Result | Notes |
| :--- | :--- | :--- | :--- |
| **`gba-testrunner` (CI Headless CRC / Pass-Code)** | `cargo run -p gba-testrunner -- --allow-failures --report TEST_RESULTS.md` | **43 passed, 9 failed, 2 skipped (missing ROM), 2 skipped (missing golden)** (`56` total) | Includes `jsmolka/gba-tests`, `FuzzARM`, `ARMWrestler`, `NanoBoyAdvance hw-test`, `Tonc`, `mGBA` (optional), `AGS` (optional); see [`TEST_RESULTS.md`](TEST_RESULTS.md) |
| **`gb-core` Unit + Harness Tests** | `cargo test -p gb-core` | **13 passed, 0 failed** | Includes CPU/APU/PPU/MBC unit checks and `mooneye_ld_bb_harness_and_optional_suite` (optional external `test-roms/gb/mooneye/` directory runs when populated) |

---

## Commercial Title Hardware Database & Real-BIOS Guidance (`60+` Titles)

Legend for **HLE / BIOS Recommendation**:
- **HLE Supported**: Works with built-in HLE BIOS (or optional real 16 KB GBA BIOS).
- **Real BIOS Recommended**: Listed in `REAL_BIOS_FALLBACK_GAMES`; triggers a non-blocking startup notification suggesting a user-supplied real 16 KB GBA BIOS image when booted under HLE BIOS.

| # | Title | Game Codes (`0xAC..0xB0`) | Save Type (`GAME_DB` / Override) | RTC & Cartridge Hardware | HLE / BIOS Guidance | Commercial Play Status |
| :---: | :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | **Pokémon Ruby** | `AXVE`, `AXVP`, `AXVJ`, `AXVD`, `AXVF`, `AXVI`, `AXVS` | `Flash 128K` | **RTC** | HLE Supported | Pending Hardware/Community Verification |
| 2 | **Pokémon Sapphire** | `AXPE`, `AXPP`, `AXPJ`, `AXPD`, `AXPF`, `AXPI`, `AXPS` | `Flash 128K` | **RTC** | HLE Supported | Pending Hardware/Community Verification |
| 3 | **Pokémon Emerald** | `BPEE`, `BPEP`, `BPEJ`, `BPED`, `BPEF`, `BPEI`, `BPES` | `Flash 128K` | **RTC** | HLE Supported | Pending Hardware/Community Verification |
| 4 | **Pokémon FireRed** | `BPRE`, `BPRP`, `BPRJ`, `BPRD`, `BPRF`, `BPRI`, `BPRS` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 5 | **Pokémon LeafGreen** | `BPGE`, `BPGP`, `BPGJ`, `BPGD`, `BPGF`, `BPGI`, `BPGS` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 6 | **Pokémon Mystery Dungeon: Red Rescue Team** | `B24E`, `B24P`, `B24J` | `Flash 128K` | Standard | **Real BIOS Recommended** (`IntrWait` / `SoundDriver` timing) | Pending Hardware/Community Verification |
| 7 | **Pokémon Pinball: Ruby & Sapphire** | `BPPE`, `BPPP`, `BPPJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 8 | **The Legend of Zelda: The Minish Cap** | `BZME`, `BZMP`, `BZMJ` | `EEPROM 8K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 9 | **The Legend of Zelda: A Link to the Past / Four Swords** | `AZLE`, `AZLP`, `AZLJ` | `EEPROM 8K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 10 | **Golden Sun** | `AGSE`, `AGSP`, `AGSJ`, `AGSD`, `AGSF`, `AGSI`, `AGSS` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 11 | **Golden Sun: The Lost Age** | `AGFE`, `AGFP`, `AGFJ`, `AGFD`, `AGFF`, `AGFI`, `AGFS` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 12 | **Mario Kart: Super Circuit** | `AMKE`, `AMKP`, `AMKJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 13 | **Metroid Fusion** | `AMTE`, `AMTP`, `AMTJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 14 | **Metroid: Zero Mission** | `BMXE`, `BMXP`, `BMXJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 15 | **Kirby: Nightmare in Dream Land** | `A7KE`, `A7KP`, `A7KJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 16 | **Kirby & The Amazing Mirror** | `B8KE`, `B8KP`, `B8KJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 17 | **Castlevania: Circle of the Moon** | `AAME`, `AAMP`, `AAMJ` | `EEPROM 8K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 18 | **Castlevania: Harmony of Dissonance** | `ACHE`, `ACHP`, `ACHJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 19 | **Castlevania: Aria of Sorrow** | `A2CE`, `A2CP`, `A2CJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 20 | **F-Zero: Maximum Velocity** | `AFZE`, `AFZP`, `AFZJ` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 21 | **F-Zero: GP Legend** | `BFZE`, `BFZP`, `BFZJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 22 | **F-Zero Climax** | `BFTJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 23 | **Super Mario Advance** | `AMAE`, `AMAP`, `AMAJ` | `EEPROM 8K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 24 | **Super Mario World: Super Mario Advance 2** | `AA2E`, `AA2P`, `AA2J` | `EEPROM 8K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 25 | **Yoshi's Island: Super Mario Advance 3** | `A3AE`, `A3AP`, `A3AJ` | `EEPROM 8K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 26 | **Super Mario Advance 4: Super Mario Bros. 3** | `AX4E`, `AX4P`, `AX4J` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 27 | **Mario & Luigi: Superstar Saga** | `A88E`, `A88P`, `A88J` | `EEPROM 8K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 28 | **Advance Wars** | `AWRE`, `AWRP`, `AWRJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 29 | **Advance Wars 2: Black Hole Rising** | `AW2E`, `AW2P`, `AW2J` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 30 | **Fire Emblem** | `AE7E`, `AE7P`, `AE7J` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 31 | **Fire Emblem: The Sacred Stones** | `BE8E`, `BE8P`, `BE8J` | `SRAM 32K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 32 | **Sonic Advance** | `ASOE`, `ASOP`, `ASOJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 33 | **Sonic Advance 2** | `A2NE`, `A2NP`, `A2NJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 34 | **Sonic Advance 3** | `B3SE`, `B3SP`, `B3SJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 35 | **Mother 3** | `A3UJ` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 36 | **Boktai: The Sun Is in Your Hand** | `U3IE`, `U3IP`, `U3IJ` | `EEPROM 8K` | **RTC + Solar** | HLE Supported | Pending Hardware/Community Verification |
| 37 | **Boktai 2: Solar Boy Django** | `U32E`, `U32P`, `U32J` | `EEPROM 8K` | **RTC + Solar** | HLE Supported | Pending Hardware/Community Verification |
| 38 | **Shin Bokura no Taiyou (Boktai 3)** | `U33J` | `EEPROM 8K` | **RTC + Solar** | HLE Supported | Pending Hardware/Community Verification |
| 39 | **Yoshi Topsy-Turvy / Universal Gravitation** | `KYGE`, `KYGP`, `KYGJ` | `EEPROM 8K` | **Tilt** | HLE Supported | Pending Hardware/Community Verification |
| 40 | **WarioWare: Twisted!** | `RZWE`, `RZWJ` | `SRAM 32K` | **Gyro + Rumble** | HLE Supported | Pending Hardware/Community Verification |
| 41 | **Drill Dozer** | `V49E`, `V49P`, `V49J` | `SRAM 32K` | **Rumble** | HLE Supported | Pending Hardware/Community Verification |
| 42 | **Mega Man Battle Network 5: Team ProtoMan** | `BRBE`, `BRBP`, `BRBJ` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 43 | **Mega Man Battle Network 5: Team Colonel** | `BRKE`, `BRKP`, `BRKJ` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 44 | **Mega Man Battle Network 6: Cybeast Gregar** | `B6GE`, `B6GP`, `B6GJ` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 45 | **Mega Man Battle Network 6: Cybeast Falzar** | `B6FE`, `B6FP`, `B6FJ` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 46 | **Rockman EXE 4.5 Real Operation** | `BR4J` | `Flash 128K` | **RTC** | HLE Supported | Pending Hardware/Community Verification |
| 47 | **Dragon Quest Monsters: Caravan Heart** | `A4XJ` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 48 | **Shin Megami Tensei (GBA)** | `ASMJ` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 49 | **Shin Megami Tensei II (GBA)** | `AG2J` | `Flash 128K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 50 | **Doom II** | `A9DE` | `Flash 64K` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 51 | **Duke Nukem Advance** | `AD9E`, `AD9P` | `EEPROM 512B` | Standard | HLE Supported | Pending Hardware/Community Verification |
| 52 | **Classic NES: Super Mario Bros.** | `FSME`, `FSMP`, `FSMJ` | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 53 | **Classic NES: The Legend of Zelda** | `FZLE`, `FZLJ` (`FZLP`) | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 54 | **Classic NES: Metroid** | `FMRE`, `FMRJ` (`FMRP`) | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 55 | **Classic NES: Donkey Kong** | `FDKE`, `FDKJ` (`FDKP`) | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 56 | **Classic NES: Ice Climber** | `FICE`, `FICJ` (`FICP`) | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 57 | **Classic NES: Excitebike** | `FEBE`, `FEBJ` | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 58 | **Classic NES: Pac-Man** | `FPME`, `FPMJ` | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 59 | **Classic NES: Xevious** | `FXVE`, `FXVJ` | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 60 | **Classic NES: Bomberman** | `FBME`, `FBMJ` (`FBMP`) | `EEPROM 512B` | Standard | **Real BIOS Recommended** (BIOS open-bus checks) | Pending Hardware/Community Verification |
| 61 | **Pinobee: Wings of Adventure** | `APNE`, `APNP`, `APNJ` | Auto-detect | Standard | **Real BIOS Recommended** (post-boot/SWI BIOS open bus) | Pending Hardware/Community Verification |
| 62 | **Dragon Ball Z: The Legacy of Goku II** | `ALFE`, `ALFP`, `ALFJ` | Auto-detect | Standard | **Real BIOS Recommended** (BIOS checksum & open bus) | Pending Hardware/Community Verification |
| 63 | **Dragon Ball Z: Buu's Fury** | `BG3E` | Auto-detect | Standard | **Real BIOS Recommended** (BIOS checksum & open bus) | Pending Hardware/Community Verification |
| 64 | **The Urbz: Sims in the City** | `BUBE`, `BUBP` | Auto-detect | Standard | **Real BIOS Recommended** (SWI / SoundDriver timing) | Pending Hardware/Community Verification |
| 65 | **The Sims 2** | `B42E`, `B42P` | Auto-detect | Standard | **Real BIOS Recommended** (SWI / SoundDriver timing) | Pending Hardware/Community Verification |
| 66 | **Yu-Gi-Oh! WCT 2006** | `BY6E`, `BY6P`, `BY6J` | Auto-detect | Standard | **Real BIOS Recommended** (SWI timing) | Pending Hardware/Community Verification |
| 67 | **Iridion II** | `AI2E`, `AI2P` | Auto-detect | Standard | **Real BIOS Recommended** (intro BIOS state & SWI timing) | Pending Hardware/Community Verification |

---

## Reporting Compatibility Issues

If you encounter a save-detection failure, graphic artifact, audio glitch, or HLE BIOS edge case in a legally dumped commercial or homebrew ROM:
1. Check **Game Settings** in the desktop UI (or `GAME_DB` in `crates/gba-core/src/gamedb.rs`) to test an explicit save-type override (`SRAM 32K`, `Flash 64K`, `Flash 128K`, `EEPROM 512B`, `EEPROM 8K`).
2. Test with a real 16 KB GBA BIOS (`--bios <path>` or Settings -> Emulation).
3. Include the ROM's 4-character game code (`0xAC..0xB0`), CRC32, and test conditions when filing an issue.

