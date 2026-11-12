use crate::cartridge::SaveType;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GameDbEntry {
    pub game_code: &'static str,
    pub save_type: SaveType,
    pub has_rtc: bool,
    pub solar: bool,
    pub tilt: bool,
    pub gyro: bool,
    pub rumble: bool,
    pub idle_loop_addr: Option<u32>,
}

impl GameDbEntry {
    const fn basic(game_code: &'static str, save_type: SaveType) -> Self {
        Self {
            game_code,
            save_type,
            has_rtc: false,
            solar: false,
            tilt: false,
            gyro: false,
            rumble: false,
            idle_loop_addr: None,
        }
    }

    const fn with_flags(
        game_code: &'static str,
        save_type: SaveType,
        has_rtc: bool,
        solar: bool,
        tilt: bool,
        gyro: bool,
        rumble: bool,
    ) -> Self {
        Self {
            game_code,
            save_type,
            has_rtc,
            solar,
            tilt,
            gyro,
            rumble,
            idle_loop_addr: None,
        }
    }
}

pub const GAME_DB: &[GameDbEntry] = &[
    // Pokémon Ruby
    GameDbEntry::with_flags("AXVE", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXVP", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXVJ", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXVD", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXVF", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXVI", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXVS", SaveType::Flash128K, true, false, false, false, false),
    // Pokémon Sapphire
    GameDbEntry::with_flags("AXPE", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXPP", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXPJ", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXPD", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXPF", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXPI", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("AXPS", SaveType::Flash128K, true, false, false, false, false),
    // Pokémon Emerald
    GameDbEntry::with_flags("BPEE", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("BPEP", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("BPEJ", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("BPED", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("BPEF", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("BPEI", SaveType::Flash128K, true, false, false, false, false),
    GameDbEntry::with_flags("BPES", SaveType::Flash128K, true, false, false, false, false),
    // Pokémon FireRed
    GameDbEntry::basic("BPRE", SaveType::Flash128K),
    GameDbEntry::basic("BPRP", SaveType::Flash128K),
    GameDbEntry::basic("BPRJ", SaveType::Flash128K),
    GameDbEntry::basic("BPRD", SaveType::Flash128K),
    GameDbEntry::basic("BPRF", SaveType::Flash128K),
    GameDbEntry::basic("BPRI", SaveType::Flash128K),
    GameDbEntry::basic("BPRS", SaveType::Flash128K),
    // Pokémon LeafGreen
    GameDbEntry::basic("BPGE", SaveType::Flash128K),
    GameDbEntry::basic("BPGP", SaveType::Flash128K),
    GameDbEntry::basic("BPGJ", SaveType::Flash128K),
    GameDbEntry::basic("BPGD", SaveType::Flash128K),
    GameDbEntry::basic("BPGF", SaveType::Flash128K),
    GameDbEntry::basic("BPGI", SaveType::Flash128K),
    GameDbEntry::basic("BPGS", SaveType::Flash128K),
    // Pokémon Mystery Dungeon: Red Rescue Team & Pokémon Pinball R&S
    GameDbEntry::basic("B24E", SaveType::Flash128K),
    GameDbEntry::basic("B24P", SaveType::Flash128K),
    GameDbEntry::basic("B24J", SaveType::Flash128K),
    GameDbEntry::basic("BPPE", SaveType::Sram32K),
    GameDbEntry::basic("BPPP", SaveType::Sram32K),
    GameDbEntry::basic("BPPJ", SaveType::Sram32K),
    // Boktai 1 / 2 / 3 (RTC + Solar)
    GameDbEntry::with_flags("U3IE", SaveType::Eeprom8K, true, true, false, false, false),
    GameDbEntry::with_flags("U3IP", SaveType::Eeprom8K, true, true, false, false, false),
    GameDbEntry::with_flags("U3IJ", SaveType::Eeprom8K, true, true, false, false, false),
    GameDbEntry::with_flags("U32E", SaveType::Eeprom8K, true, true, false, false, false),
    GameDbEntry::with_flags("U32P", SaveType::Eeprom8K, true, true, false, false, false),
    GameDbEntry::with_flags("U32J", SaveType::Eeprom8K, true, true, false, false, false),
    GameDbEntry::with_flags("U33J", SaveType::Eeprom8K, true, true, false, false, false),
    // Yoshi Topsy-Turvy / Yoshi's Universal Gravitation (Tilt)
    GameDbEntry::with_flags("KYGE", SaveType::Eeprom8K, false, false, true, false, false),
    GameDbEntry::with_flags("KYGP", SaveType::Eeprom8K, false, false, true, false, false),
    GameDbEntry::with_flags("KYGJ", SaveType::Eeprom8K, false, false, true, false, false),
    // WarioWare: Twisted! (Gyro + Rumble)
    GameDbEntry::with_flags("RZWE", SaveType::Sram32K, false, false, false, true, true),
    GameDbEntry::with_flags("RZWJ", SaveType::Sram32K, false, false, false, true, true),
    // Drill Dozer (Rumble)
    GameDbEntry::with_flags("V49E", SaveType::Sram32K, false, false, false, false, true),
    GameDbEntry::with_flags("V49P", SaveType::Sram32K, false, false, false, false, true),
    GameDbEntry::with_flags("V49J", SaveType::Sram32K, false, false, false, false, true),
    // Golden Sun & Golden Sun: The Lost Age
    GameDbEntry::basic("AGSE", SaveType::Flash64K),
    GameDbEntry::basic("AGSP", SaveType::Flash64K),
    GameDbEntry::basic("AGSJ", SaveType::Flash64K),
    GameDbEntry::basic("AGSD", SaveType::Flash64K),
    GameDbEntry::basic("AGSF", SaveType::Flash64K),
    GameDbEntry::basic("AGSI", SaveType::Flash64K),
    GameDbEntry::basic("AGSS", SaveType::Flash64K),
    GameDbEntry::basic("AGFE", SaveType::Flash64K),
    GameDbEntry::basic("AGFP", SaveType::Flash64K),
    GameDbEntry::basic("AGFJ", SaveType::Flash64K),
    GameDbEntry::basic("AGFD", SaveType::Flash64K),
    GameDbEntry::basic("AGFF", SaveType::Flash64K),
    GameDbEntry::basic("AGFI", SaveType::Flash64K),
    GameDbEntry::basic("AGFS", SaveType::Flash64K),
    // The Legend of Zelda: The Minish Cap & A Link to the Past / Four Swords
    GameDbEntry::basic("BZME", SaveType::Eeprom8K),
    GameDbEntry::basic("BZMP", SaveType::Eeprom8K),
    GameDbEntry::basic("BZMJ", SaveType::Eeprom8K),
    GameDbEntry::basic("AZLE", SaveType::Eeprom8K),
    GameDbEntry::basic("AZLP", SaveType::Eeprom8K),
    GameDbEntry::basic("AZLJ", SaveType::Eeprom8K),
    // Metroid Fusion & Metroid: Zero Mission
    GameDbEntry::basic("AMTE", SaveType::Sram32K),
    GameDbEntry::basic("AMTP", SaveType::Sram32K),
    GameDbEntry::basic("AMTJ", SaveType::Sram32K),
    GameDbEntry::basic("BMXE", SaveType::Sram32K),
    GameDbEntry::basic("BMXP", SaveType::Sram32K),
    GameDbEntry::basic("BMXJ", SaveType::Sram32K),
    // Mario Kart: Super Circuit
    GameDbEntry::basic("AMKE", SaveType::Flash64K),
    GameDbEntry::basic("AMKP", SaveType::Flash64K),
    GameDbEntry::basic("AMKJ", SaveType::Flash64K),
    // Kirby: Nightmare in Dream Land & Kirby & The Amazing Mirror
    GameDbEntry::basic("A7KE", SaveType::Sram32K),
    GameDbEntry::basic("A7KP", SaveType::Sram32K),
    GameDbEntry::basic("A7KJ", SaveType::Sram32K),
    GameDbEntry::basic("B8KE", SaveType::Sram32K),
    GameDbEntry::basic("B8KP", SaveType::Sram32K),
    GameDbEntry::basic("B8KJ", SaveType::Sram32K),
    // Castlevania: Circle of the Moon / Harmony of Dissonance / Aria of Sorrow
    GameDbEntry::basic("AAME", SaveType::Eeprom8K),
    GameDbEntry::basic("AAMP", SaveType::Eeprom8K),
    GameDbEntry::basic("AAMJ", SaveType::Eeprom8K),
    GameDbEntry::basic("ACHE", SaveType::Sram32K),
    GameDbEntry::basic("ACHP", SaveType::Sram32K),
    GameDbEntry::basic("ACHJ", SaveType::Sram32K),
    GameDbEntry::basic("A2CE", SaveType::Sram32K),
    GameDbEntry::basic("A2CP", SaveType::Sram32K),
    GameDbEntry::basic("A2CJ", SaveType::Sram32K),
    // F-Zero: Maximum Velocity / GP Legend / Climax
    GameDbEntry::basic("AFZE", SaveType::Sram32K),
    GameDbEntry::basic("AFZP", SaveType::Sram32K),
    GameDbEntry::basic("AFZJ", SaveType::Sram32K),
    GameDbEntry::basic("BFZE", SaveType::Flash64K),
    GameDbEntry::basic("BFZP", SaveType::Flash64K),
    GameDbEntry::basic("BFZJ", SaveType::Flash64K),
    GameDbEntry::basic("BFTJ", SaveType::Flash64K),
    // Super Mario Advance 1-4 & Mario & Luigi: Superstar Saga
    GameDbEntry::basic("AMAE", SaveType::Eeprom8K),
    GameDbEntry::basic("AMAP", SaveType::Eeprom8K),
    GameDbEntry::basic("AMAJ", SaveType::Eeprom8K),
    GameDbEntry::basic("AA2E", SaveType::Eeprom8K),
    GameDbEntry::basic("AA2P", SaveType::Eeprom8K),
    GameDbEntry::basic("AA2J", SaveType::Eeprom8K),
    GameDbEntry::basic("A3AE", SaveType::Eeprom8K),
    GameDbEntry::basic("A3AP", SaveType::Eeprom8K),
    GameDbEntry::basic("A3AJ", SaveType::Eeprom8K),
    GameDbEntry::basic("AX4E", SaveType::Flash128K),
    GameDbEntry::basic("AX4P", SaveType::Flash128K),
    GameDbEntry::basic("AX4J", SaveType::Flash128K),
    GameDbEntry::basic("A88E", SaveType::Eeprom8K),
    GameDbEntry::basic("A88P", SaveType::Eeprom8K),
    GameDbEntry::basic("A88J", SaveType::Eeprom8K),
    // Advance Wars 1 & 2
    GameDbEntry::basic("AWRE", SaveType::Flash64K),
    GameDbEntry::basic("AWRP", SaveType::Flash64K),
    GameDbEntry::basic("AWRJ", SaveType::Flash64K),
    GameDbEntry::basic("AW2E", SaveType::Flash64K),
    GameDbEntry::basic("AW2P", SaveType::Flash64K),
    GameDbEntry::basic("AW2J", SaveType::Flash64K),
    // Fire Emblem & Fire Emblem: The Sacred Stones
    GameDbEntry::basic("AE7E", SaveType::Sram32K),
    GameDbEntry::basic("AE7P", SaveType::Sram32K),
    GameDbEntry::basic("AE7J", SaveType::Sram32K),
    GameDbEntry::basic("BE8E", SaveType::Sram32K),
    GameDbEntry::basic("BE8P", SaveType::Sram32K),
    GameDbEntry::basic("BE8J", SaveType::Sram32K),
    // Sonic Advance 1-3 & Mother 3
    GameDbEntry::basic("ASOE", SaveType::Flash64K),
    GameDbEntry::basic("ASOP", SaveType::Flash64K),
    GameDbEntry::basic("ASOJ", SaveType::Flash64K),
    GameDbEntry::basic("A2NE", SaveType::Flash64K),
    GameDbEntry::basic("A2NP", SaveType::Flash64K),
    GameDbEntry::basic("A2NJ", SaveType::Flash64K),
    GameDbEntry::basic("B3SE", SaveType::Flash64K),
    GameDbEntry::basic("B3SP", SaveType::Flash64K),
    GameDbEntry::basic("B3SJ", SaveType::Flash64K),
    GameDbEntry::basic("A3UJ", SaveType::Flash64K),
    // Classic NES Series & Famicom Mini (4Kbit EEPROM)
    GameDbEntry::basic("FSME", SaveType::Eeprom512B),
    GameDbEntry::basic("FSMP", SaveType::Eeprom512B),
    GameDbEntry::basic("FSMJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FDKE", SaveType::Eeprom512B),
    GameDbEntry::basic("FDKJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FICE", SaveType::Eeprom512B),
    GameDbEntry::basic("FICJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FEBE", SaveType::Eeprom512B),
    GameDbEntry::basic("FEBJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FZLE", SaveType::Eeprom512B),
    GameDbEntry::basic("FZLJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FPME", SaveType::Eeprom512B),
    GameDbEntry::basic("FPMJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FXVE", SaveType::Eeprom512B),
    GameDbEntry::basic("FXVJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FMPJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FBME", SaveType::Eeprom512B),
    GameDbEntry::basic("FBMJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FMRE", SaveType::Eeprom512B),
    GameDbEntry::basic("FMRJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FLBE", SaveType::Eeprom512B),
    GameDbEntry::basic("FLBJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FDME", SaveType::Eeprom512B),
    GameDbEntry::basic("FDMJ", SaveType::Eeprom512B),
    GameDbEntry::basic("FADE", SaveType::Eeprom512B),
    GameDbEntry::basic("FADJ", SaveType::Eeprom512B),
    // Mega Man Battle Network 5 & 6 (Flash 128K)
    GameDbEntry::basic("BRBE", SaveType::Flash128K),
    GameDbEntry::basic("BRBP", SaveType::Flash128K),
    GameDbEntry::basic("BRBJ", SaveType::Flash128K),
    GameDbEntry::basic("BRKE", SaveType::Flash128K),
    GameDbEntry::basic("BRKP", SaveType::Flash128K),
    GameDbEntry::basic("BRKJ", SaveType::Flash128K),
    GameDbEntry::basic("B6GE", SaveType::Flash128K),
    GameDbEntry::basic("B6GP", SaveType::Flash128K),
    GameDbEntry::basic("B6GJ", SaveType::Flash128K),
    GameDbEntry::basic("B6FE", SaveType::Flash128K),
    GameDbEntry::basic("B6FP", SaveType::Flash128K),
    GameDbEntry::basic("B6FJ", SaveType::Flash128K),
    // Rockman EXE 4.5 Real Operation (Flash 128K + RTC)
    GameDbEntry::with_flags("BR4J", SaveType::Flash128K, true, false, false, false, false),
    // Dragon Quest Monsters: Caravan Heart & Shin Megami Tensei (Flash 128K)
    GameDbEntry::basic("A4XJ", SaveType::Flash128K),
    GameDbEntry::basic("ASMJ", SaveType::Flash128K),
    GameDbEntry::basic("AG2J", SaveType::Flash128K),
    // Doom II (USA: A9DE) -> Flash 64K
    GameDbEntry::basic("A9DE", SaveType::Flash64K),
    // Duke Nukem Advance (USA: AD9E, EUR: AD9P) -> EEPROM 512B
    GameDbEntry::basic("AD9E", SaveType::Eeprom512B),
    GameDbEntry::basic("AD9P", SaveType::Eeprom512B),
];

/// Looks up a game database entry by its 4-character cartridge game code (ROM header `0xAC..0xB0`).
pub fn lookup(game_code: &str) -> Option<&'static GameDbEntry> {
    if game_code.len() != 4 {
        return None;
    }
    GAME_DB.iter().find(|entry| entry.game_code == game_code)
}

/// Looks up a game database entry from a 4-byte cartridge game code slice.
pub fn lookup_bytes(game_code: &[u8]) -> Option<&'static GameDbEntry> {
    let s = core::str::from_utf8(game_code).ok()?;
    lookup(s)
}
