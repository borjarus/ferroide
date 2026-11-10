//! Property-based tests (plan Stage 3, Definition of Done): the rope buffer
//! against a `String` model, undo/redo, anchors, the incremental display
//! map, encodings, search, and random editing sessions.

use std::time::{Duration, Instant};

use ferro_text::{
    Bias, Buffer, Edit, EditKind, Rope, SelectionSet,
    display_map::{DisplayMap, DisplayMapConfig},
    edit::{apply_to_string, map_offset, normalize},
    encoding::{self, TextEncoding},
    file::{self, FileFormat},
    line_ending::{self, LineEnding},
    search::SearchQuery,
    unicode::next_grapheme_boundary,
};
use proptest::prelude::*;

fn text() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            "[a-z ]{0,8}",
            Just("\n".to_string()),
            Just("\t".to_string()),
            Just("zażółć".to_string()),
            Just("漢字".to_string()),
            Just("👨‍👩‍👧".to_string()),
            Just("e\u{301}".to_string()),
        ],
        0..20,
    )
    .prop_map(|parts| parts.concat())
}

/// Raw edits: positions as fractions of the text, snapped to characters.
fn raw_edits() -> impl Strategy<Value = Vec<(f64, f64, String)>> {
    proptest::collection::vec((0.0..=1.0f64, 0.0..=0.3f64, text()), 1..5)
}

fn snap(text: &str, fraction: f64) -> usize {
    let mut at = (text.len() as f64 * fraction) as usize;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Valid, non-overlapping edits for `text`.
fn make_edits(text: &str, raw: &[(f64, f64, String)]) -> Vec<Edit> {
    let mut edits: Vec<Edit> = Vec::new();
    for (start, len, insert) in raw {
        let s = snap(text, *start);
        let e = snap(text, (*start + *len).min(1.0)).max(s);
        if edits.iter().all(|x| x.range.end <= s || e <= x.range.start)
            && !edits
                .iter()
                .any(|x| x.range.is_empty() && (x.range.start == s || x.range.start == e))
        {
            edits.push(Edit::new(s..e, insert.clone()));
        }
    }
    edits
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn buffer_matches_string_model_and_undo_restores(
        initial in text(),
        steps in proptest::collection::vec(raw_edits(), 1..12),
    ) {
        let mut buffer = Buffer::new(&initial);
        let mut model = initial.clone();
        let mut history = vec![initial.clone()];
        let start = Instant::now();
        for (i, raw) in steps.iter().enumerate() {
            let edits = make_edits(&model, raw);
            let edits = normalize(edits, buffer.text()).unwrap();
            if edits.is_empty() {
                continue;
            }
            model = apply_to_string(&model, &edits);
            let sel = SelectionSet::default();
            buffer
                .edit(edits, EditKind::Other, &sel, &sel, start + Duration::from_secs(i as u64))
                .unwrap();
            prop_assert_eq!(buffer.text().to_string(), model.clone());
            history.push(model.clone());
        }
        // Undo walks back through every state; redo forward again.
        for expected in history.iter().rev().skip(1) {
            prop_assert!(buffer.undo().is_some());
            prop_assert_eq!(&buffer.text().to_string(), expected);
        }
        prop_assert!(buffer.undo().is_none());
        for expected in history.iter().skip(1) {
            prop_assert!(buffer.redo().is_some());
            prop_assert_eq!(&buffer.text().to_string(), expected);
        }
    }

    #[test]
    fn anchors_track_their_text(
        initial in text(),
        steps in proptest::collection::vec(raw_edits(), 1..6),
        at in 0.0..=1.0f64,
        right in any::<bool>(),
    ) {
        let bias = if right { Bias::Right } else { Bias::Left };
        let mut buffer = Buffer::new(&initial);
        let offset = snap(&initial, at);
        let anchor = buffer.anchor(offset, bias);
        let mut expected = offset;
        let mut model = initial;
        for raw in &steps {
            let edits = normalize(make_edits(&model, raw), buffer.text()).unwrap();
            expected = map_offset(expected, &edits, bias);
            model = apply_to_string(&model, &edits);
            buffer.edit_untracked(edits).unwrap();
            prop_assert_eq!(buffer.resolve(&anchor), expected);
            prop_assert!(model.is_char_boundary(expected));
        }
    }

    #[test]
    fn incremental_display_map_equals_a_fresh_one(
        initial in text(),
        steps in proptest::collection::vec(raw_edits(), 1..8),
        wrap in proptest::option::of(3usize..20),
        tab_size in 1usize..9,
    ) {
        let config = DisplayMapConfig { tab_size, wrap_column: wrap };
        let mut rope = Rope::from_str(&initial);
        let mut map = DisplayMap::new(&rope, config);
        for raw in &steps {
            let text = rope.to_string();
            let edits = normalize(make_edits(&text, raw), &rope).unwrap();
            let after = Rope::from_str(&apply_to_string(&text, &edits));
            map.sync(&rope, &after, &edits);
            rope = after;
            let fresh = DisplayMap::new(&rope, config);
            prop_assert_eq!(map.row_count(), fresh.row_count());
            prop_assert_eq!(
                map.display_rows(&rope, 0..usize::MAX),
                fresh.display_rows(&rope, 0..usize::MAX)
            );
        }
        // Every caret position (grapheme boundary) maps to a screen
        // position and back to itself — or, after a zero-width grapheme,
        // to another position on the same screen spot.
        let mut offset = 0;
        loop {
            let point = map.to_display(&rope, offset);
            let back = map.to_buffer(&rope, point);
            if back != offset {
                prop_assert_eq!(map.to_display(&rope, back), point);
                let between = rope.slice(back.min(offset)..back.max(offset)).to_string();
                prop_assert_eq!(
                    ferro_text::unicode::display_width(&between, 0, tab_size),
                    0,
                    "{:?}: {} -> {:?} -> {}", rope.to_string(), offset, point, back
                );
            }
            if offset >= rope.len() {
                break;
            }
            offset = next_grapheme_boundary(&rope, offset);
        }
    }

    #[test]
    fn unicode_encodings_round_trip(text in text(), bom in any::<bool>()) {
        for encoding in [
            TextEncoding::new(encoding_rs::UTF_8, bom),
            TextEncoding::new(encoding_rs::UTF_16LE, true),
            TextEncoding::new(encoding_rs::UTF_16BE, true),
        ] {
            let bytes = encoding::encode(&text, encoding).unwrap();
            let decoded = encoding::decode_with(&bytes, encoding);
            prop_assert_eq!(&decoded.text, &text);
            prop_assert!(!decoded.had_errors);
        }
    }

    #[test]
    fn line_endings_round_trip(text in text(), crlf in any::<bool>()) {
        let ending = if crlf { LineEnding::CrLf } else { LineEnding::Lf };
        let format = FileFormat { encoding: TextEncoding::UTF8, line_ending: ending };
        let bytes = file::to_bytes(&text, format).unwrap();
        let loaded = file::load(&bytes);
        prop_assert_eq!(&loaded.text, &text);
        if text.contains('\n') {
            prop_assert_eq!(loaded.format.line_ending, ending);
        }
        let denormalized = line_ending::denormalize(&text, ending);
        let round_trip = line_ending::normalize(&denormalized).into_owned();
        prop_assert_eq!(round_trip, text);
    }

    #[test]
    fn plain_search_matches_str_find(text in text(), needle in "[a-zżół ]{1,3}") {
        let matcher = SearchQuery::plain(needle.clone()).compile().unwrap();
        let expected: Vec<_> = text
            .match_indices(&needle)
            .map(|(i, m)| i..i + m.len())
            .collect();
        prop_assert_eq!(matcher.find_all(&text), expected);
    }

}

proptest! {
    // Each case is a whole editing session; fewer cases, longer sessions.
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn random_editing_sessions_keep_invariants(
        data in proptest::collection::vec(any::<u8>(), 0..400),
    ) {
        ferro_text::fuzz::run(&data);
    }
}
