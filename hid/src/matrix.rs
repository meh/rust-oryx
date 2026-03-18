/// Moonlander MK1 key matrix ↔ RGB LED index mapping.
///
/// Left half rows 0–5, right half rows 6–11.
/// LED indices 0–71 (72 keys total, 36 per half).
///
/// The Moonlander has two different indexing schemes:
/// - Key matrix: 5 rows × 7 columns (rows 0-4, cols 0-6 for main block), row-major
/// - LED matrix: 7 rows × 5 columns, column-major indexing (col * 5 + row)
///
/// Convert a key matrix position to its RGB LED index.
///
/// Takes (col, row) where row is 0-4 for main block, 0-6 for thumb row.
/// Returns LED index using column-major formula (col * 5 + row).
pub fn key_to_led(col: u8, row: u8) -> Option<u8> {
    let idx = match (row, col) {
        // Left half: 5×7 main block (5 rows, 7 cols), column-major (col * 5 + row)
        (r @ 0..=4, c @ 0..=6) => c as usize * 5 + r as usize,
        // Left row 5 (thumb): 4 keys
        (5, 3) => 32,
        (5, c @ 0..=2) => 33 + c as usize,
        // Right half: 5×7 main block
        (r @ 6..=10, c @ 0..=6) => 36 + c as usize * 5 + (r as usize - 6),
        // Right row 11 (thumb): 4 keys
        (11, 3) => 68,
        (11, c @ 4..=6) => 69 + (c as usize - 4),
        _ => return None,
    };
    Some(idx as u8)
}

/// Convert an RGB LED index to its key matrix position `(col, row)`.
pub fn led_to_key(led: u8) -> Option<(u8, u8)> {
    let idx = led as usize;

    if idx > 71 {
        return None;
    }

    // Left half: main block indices 0-34 (7 cols × 5 rows = 35 keys)
    if idx <= 34 {
        let col = (idx / 5) as u8;
        let row = (idx % 5) as u8;
        Some((col, row))
    } else if idx <= 35 {
        // Left thumb cluster: indices 32-35
        if idx == 32 {
            Some((3, 5))
        } else {
            let col = (idx - 33) as u8;
            Some((col, 5))
        }
    } else if idx <= 69 {
        // Right half: main block indices 36-69
        let idx = idx - 36;
        let col = (idx / 5) as u8;
        let row = (idx % 5) as u8 + 6;
        Some((col, row))
    } else if idx <= 71 {
        // Right thumb cluster: indices 68-71
        if idx == 68 {
            Some((3, 11))
        } else {
            let col = (idx - 69) as u8 + 4;
            Some((col, 11))
        }
    } else {
        None
    }
}

/// Convert a key matrix position to its row-major key index.
///
/// This is used for the oryx-look visual layout which matches the physical keyboard.
/// Takes (col, row) where row is 0-6 for main block, col is 0-6.
/// Returns key index using row-major formula (row * 7 + col).
pub fn pos_to_led(col: u8, row: u8) -> Option<u8> {
    let idx = match (row, col) {
        // Left half: 3×7 main block
        (r @ 0..=2, c @ 0..=6) => r as usize * 7 + c as usize,
        // Left row 3: 6 keys (no col 6)
        (3, c @ 0..=5) => 21 + c as usize,
        // Left row 4: 5 keys (no col 5 or 6)
        (4, c @ 0..=4) => 27 + c as usize,
        // Left thumb cluster: indices 32-35
        (5, 3) => 32,
        (5, c @ 0..=2) => 33 + c as usize,
        // Right half: 3×7 main block
        (r @ 6..=8, c @ 0..=6) => 36 + (r as usize - 6) * 7 + c as usize,
        // Right row 9: 6 keys (no col 0 — tower key slot)
        (9, c @ 1..=6) => 56 + c as usize,
        // Right row 10: 5 keys (no col 0 or 1)
        (10, c @ 2..=6) => 61 + c as usize,
        // Right thumb cluster: indices 68-71
        (11, 3) => 68,
        (11, c @ 4..=6) => 69 + (c as usize - 4),
        _ => return None,
    };
    Some(idx as u8)
}

/// Convert a row-major key index to its key matrix position `(col, row)`.
pub fn led_to_pos(led: u8) -> Option<(u8, u8)> {
    let idx = led as usize;

    if idx > 71 {
        return None;
    }

    // Left half: main block indices 0-20 (3 rows of 7)
    if idx <= 20 {
        let row = (idx / 7) as u8;
        let col = (idx % 7) as u8;
        Some((col, row))
    } else if idx <= 26 {
        // Left row 3: indices 21-26
        let col = (idx - 21) as u8;
        Some((col, 3))
    } else if idx <= 31 {
        // Left row 4: indices 27-31
        let col = (idx - 27) as u8;
        Some((col, 4))
    } else if idx <= 35 {
        // Left thumb cluster: indices 32-35
        if idx == 32 {
            Some((3, 5))
        } else {
            let col = (idx - 33) as u8;
            Some((col, 5))
        }
    } else if idx <= 56 {
        // Right half: main block indices 36-56
        let idx = idx - 36;
        let row = (idx / 7) as u8 + 6;
        let col = (idx % 7) as u8;
        Some((col, row))
    } else if idx <= 62 {
        // Right row 9: indices 57-62
        let col = (idx - 57) as u8 + 1;
        Some((col, 9))
    } else if idx <= 67 {
        // Right row 10: indices 63-67
        let col = (idx - 63) as u8 + 2;
        Some((col, 10))
    } else if idx <= 71 {
        // Right thumb cluster: indices 68-71
        if idx == 68 {
            Some((3, 11))
        } else {
            let col = (idx - 69) as u8 + 4;
            Some((col, 11))
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_leds() {
        for led in 0u8..72 {
            let pos = led_to_pos(led).unwrap_or_else(|| panic!("led {led} has no position"));
            let back = pos_to_led(pos.0, pos.1).expect("pos should map back");
            assert_eq!(back, led, "led {led} roundtrip failed");
        }
    }

    #[test]
    fn roundtrip_all_keys() {
        for led in 0u8..72 {
            let pos = led_to_key(led).unwrap_or_else(|| panic!("led {led} has no position"));
            let back = key_to_led(pos.0, pos.1).expect("pos should map back");
            assert_eq!(back, led, "key {led} roundtrip failed");
        }
    }

    #[test]
    fn no_duplicate_positions() {
        let mut seen = std::collections::HashSet::new();
        for led in 0u8..72 {
            if let Some(pos) = led_to_pos(led) {
                assert!(seen.insert(pos), "duplicate position {pos:?} for led {led}");
            }
        }
    }

    #[test]
    fn no_duplicate_keys() {
        let mut seen = std::collections::HashSet::new();
        for led in 0u8..72 {
            if let Some(pos) = led_to_key(led) {
                assert!(
                    seen.insert(pos),
                    "duplicate key position {pos:?} for led {led}"
                );
            }
        }
    }

    #[test]
    fn out_of_range_returns_none() {
        assert!(led_to_pos(72).is_none());
        assert!(led_to_pos(255).is_none());
    }

    #[test]
    fn matches_physical_keybaord() {
        assert_eq!(led_to_pos(5), Some((5, 0)));
    }
}
