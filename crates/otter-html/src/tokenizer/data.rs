use alloc::string::{String, ToString};

// Windows-1252 replacement table for 0x80-0x9F
pub(crate) const WINDOWS_1252_TABLE: &[u32] = &[
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D, 0x017D, 0x008F,
    0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// Decode a numeric character reference (decimal or hex)
pub fn decode_numeric_entity(codepoint: u32) -> Option<char> {
    // Handle Windows-1252 replacement for 0x80-0x9F
    let final_codepoint = if (0x80..=0x9F).contains(&codepoint) {
        WINDOWS_1252_TABLE[(codepoint - 0x80) as usize]
    } else {
        codepoint
    };

    // Reject surrogates, NULL, and codepoints > 0x10FFFF
    if final_codepoint == 0
        || (0xD800..=0xDFFF).contains(&final_codepoint)
        || final_codepoint > 0x10FFFF
    {
        return Some('\u{FFFD}'); // Replacement character
    }

    char::from_u32(final_codepoint)
}

/// Resolve a named entity reference
pub fn decode_named_entity(name: &str) -> Option<String> {
    use alloc::format;
    use crate::NAMED_ENTITIES;

    // The NAMED_ENTITIES table includes the & prefix, so we need to add it
    let with_amp = format!("&{}", name);
    for &(entity_name, entity_char) in NAMED_ENTITIES {
        if entity_name == with_amp {
            return Some(entity_char.to_string());
        }
    }
    None
}
