/// Moonlander MK1 key matrix ↔ RGB LED index mapping.
///
/// Left half rows 0–5, right half rows 6–11.
/// LED indices 0–71 (72 keys total, 36 per half).

/// Convert a Moonlander MK1 key matrix position to its RGB LED index.
///
/// Returns `None` for positions that don't correspond to a physical key
/// (e.g. the missing tower-key slot on each half).
pub fn pos_to_led(col: u8, row: u8) -> Option<u8> {
    let idx = match (row, col) {
        // Left half: 3×7 main block
        (r @ 0..=2, c @ 0..=6) => r as usize * 7 + c as usize,
        // Left row 3: 6 keys (no col 6)
        (3, c @ 0..=5) => 21 + c as usize,
        // Left row 4: 5 keys (no col 5 or 6)
        (4, c @ 0..=4) => 27 + c as usize,
        // Left thumb cluster: wide key is index 32, small keys 33–35
        (5, 3) => 32,
        (5, c @ 0..=2) => 33 + c as usize,
        // Right half: 3×7 main block
        (r @ 6..=8, c @ 0..=6) => 36 + (r as usize - 6) * 7 + c as usize,
        // Right row 9: 6 keys (no col 0 — tower key slot)
        (9, c @ 1..=6) => 56 + c as usize,
        // Right row 10: 5 keys (no col 0 or 1)
        (10, c @ 2..=6) => 61 + c as usize,
        // Right thumb cluster: wide key is index 68, small keys 69–71
        (11, 3) => 68,
        (11, c @ 4..=6) => 65 + c as usize,
        _ => return None,
    };
    Some(idx as u8)
}

/// Convert a Moonlander MK1 RGB LED index to its key matrix position `(col, row)`.
///
/// Returns `None` for indices outside the valid range 0–71.
pub fn led_to_pos(led: u8) -> Option<(u8, u8)> {
    // Build inverse by scanning all valid (row, col) positions.
    // Called infrequently (startup only), so a linear scan is fine.
    for row in 0u8..=11 {
        for col in 0u8..=6 {
            if pos_to_led(col, row) == Some(led) {
                return Some((col, row));
            }
        }
    }
    None
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
    fn no_duplicate_positions() {
        let mut seen = std::collections::HashSet::new();
        for led in 0u8..72 {
            if let Some(pos) = led_to_pos(led) {
                assert!(seen.insert(pos), "duplicate position {pos:?} for led {led}");
            }
        }
    }

    #[test]
    fn out_of_range_returns_none() {
        assert!(led_to_pos(72).is_none());
        assert!(led_to_pos(255).is_none());
    }
}
