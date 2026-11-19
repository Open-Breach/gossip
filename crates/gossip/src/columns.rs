//! Fitting a value into a column: padded when it is shorter, cut with an
//! ellipsis when it is longer. Widths are terminal cells, not bytes and not
//! characters: an emoji takes two.

use gossip_core::{Column, Cut};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// What marks the place where a value was cut. One cell wide.
pub const ELLIPSIS: char = '…';

/// How many terminal cells a text takes.
///
/// This is the one measure of the output: the columns are padded with it,
/// and the mark of an event, an emoji or `EVNT`, is measured with it too.
pub fn cells(text: &str) -> usize {
    pieces(text).iter().map(|piece| piece.cells).sum()
}

/// A piece of a text that cannot be cut: a character with whatever follows
/// it and takes no cell of its own, such as the selector that makes `⚠️` an
/// emoji, or the parts of an emoji joined by a zero-width joiner.
struct Piece {
    start: usize,
    end: usize,
    cells: usize,
}

fn pieces(text: &str) -> Vec<Piece> {
    const JOINER: char = '\u{200d}';
    let mut pieces: Vec<Piece> = Vec::new();
    let mut joined = false;
    for (start, character) in text.char_indices() {
        let end = start + character.len_utf8();
        let attaches = joined || UnicodeWidthChar::width(character) == Some(0);
        match pieces.last_mut() {
            Some(piece) if attaches => piece.end = end,
            _ => pieces.push(Piece {
                start,
                end,
                cells: 0,
            }),
        }
        joined = character == JOINER;
    }
    for piece in &mut pieces {
        piece.cells = UnicodeWidthStr::width(&text[piece.start..piece.end]);
    }
    pieces
}

/// A value as it stands in its column: padded to the width of the column
/// when it is shorter, cut on the side the column says when it is longer.
/// None when the column is hidden.
pub fn fit(text: &str, column: Column) -> Option<String> {
    let width = usize::from(column.width);
    if width == 0 {
        return None;
    }
    let taken = cells(text);
    if taken <= width || column.cut == Cut::None {
        return Some(pad(text.to_string(), taken, width));
    }
    // The ellipsis takes one cell of the column.
    let room = width - 1;
    let (head, tail) = match column.cut {
        Cut::Start => (0, room),
        Cut::Middle => (room.div_ceil(2), room / 2),
        Cut::End | Cut::None => (room, 0),
    };
    Some(keep_ends(text, head, tail, width))
}

/// The start and the end of a text with an ellipsis for what was between
/// them: at most `head` cells of the start and `tail` cells of the end,
/// padded to `width`. A character that is two cells wide is kept whole or
/// not at all.
pub fn keep_ends(text: &str, head: usize, tail: usize, width: usize) -> String {
    let pieces = pieces(text);
    let mut front = 0;
    let mut head_cells = 0;
    while front < pieces.len() && head_cells + pieces[front].cells <= head {
        head_cells += pieces[front].cells;
        front += 1;
    }
    let mut back = pieces.len();
    let mut tail_cells = 0;
    while back > front && tail_cells + pieces[back - 1].cells <= tail {
        tail_cells += pieces[back - 1].cells;
        back -= 1;
    }
    let head_end = pieces.get(front).map_or(text.len(), |piece| piece.start);
    let tail_start = pieces.get(back).map_or(text.len(), |piece| piece.start);
    let cut = format!("{}{ELLIPSIS}{}", &text[..head_end], &text[tail_start..]);
    pad(cut, head_cells + 1 + tail_cells, width)
}

fn pad(mut text: String, taken: usize, width: usize) -> String {
    text.extend(std::iter::repeat_n(' ', width.saturating_sub(taken)));
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(width: u16, cut: Cut) -> Column {
        Column { width, cut }
    }

    fn fitted(text: &str, width: u16, cut: Cut) -> String {
        fit(text, column(width, cut)).expect("the column is not hidden")
    }

    const POD: &str = "fc-api-7f698fb95f-vxfgs";

    #[test]
    fn a_shorter_value_is_padded_to_the_width() {
        for cut in [Cut::None, Cut::Start, Cut::End, Cut::Middle] {
            assert_eq!(fitted("api", 10, cut), "api       ");
            assert_eq!(fitted("", 5, cut), "     ");
            // One that fits exactly is left as it is.
            assert_eq!(fitted("envoy", 5, cut), "envoy");
        }
    }

    #[test]
    fn a_width_of_zero_hides_the_column() {
        for cut in [Cut::None, Cut::Start, Cut::End, Cut::Middle] {
            assert_eq!(fit("api", column(0, cut)), None);
            assert_eq!(fit("", column(0, cut)), None);
        }
    }

    #[test]
    fn a_longer_value_is_cut_on_the_side_the_column_says() {
        // The examples of the spec, section 5.
        assert_eq!(
            fitted("fc-api-7f698fb9c7-vxfgs", 10, Cut::Start),
            "…9c7-vxfgs"
        );
        assert_eq!(fitted(POD, 13, Cut::End), "fc-api-7f698…");
        assert_eq!(fitted(POD, 12, Cut::Middle), "fc-api…vxfgs");

        assert_eq!(fitted(POD, 20, Cut::Start), "…pi-7f698fb95f-vxfgs");
        assert_eq!(fitted(POD, 20, Cut::End), "fc-api-7f698fb95f-v…");
        // The middle: ten cells of the start, nine of the end.
        assert_eq!(fitted(POD, 20, Cut::Middle), "fc-api-7f6…95f-vxfgs");
        // An even width has one cell more for the start.
        assert_eq!(fitted("abcdefghij", 6, Cut::Middle), "abc…ij");
        assert_eq!(fitted("abcdefghij", 5, Cut::Middle), "ab…ij");

        // Every result is as wide as its column.
        for cut in [Cut::Start, Cut::End, Cut::Middle] {
            for width in 1..=22 {
                let text = fitted(POD, width, cut);
                assert_eq!(cells(&text), usize::from(width), "{cut} {width}: {text}");
                assert!(text.contains(ELLIPSIS), "{cut} {width}: {text}");
            }
            // And one that fits is whole.
            assert_eq!(fitted(POD, 23, cut), POD);
        }
    }

    #[test]
    fn a_column_one_or_two_cells_wide_still_marks_the_cut() {
        assert_eq!(fitted(POD, 1, Cut::Start), "…");
        assert_eq!(fitted(POD, 1, Cut::End), "…");
        assert_eq!(fitted(POD, 1, Cut::Middle), "…");
        assert_eq!(fitted(POD, 2, Cut::Start), "…s");
        assert_eq!(fitted(POD, 2, Cut::End), "f…");
        assert_eq!(fitted(POD, 2, Cut::Middle), "f…");
    }

    #[test]
    fn a_value_that_is_never_cut_pushes_the_line_to_the_right() {
        assert_eq!(fitted(POD, 10, Cut::None), POD);
        assert_eq!(fitted("14:02:11.482", 8, Cut::None), "14:02:11.482");
    }

    #[test]
    fn an_emoji_is_two_cells_wide() {
        assert_eq!(cells("💀"), 2);
        assert_eq!(cells("api"), 3);
        assert_eq!(cells("api 🚀"), 6);
        // One that is an emoji only by the selector after it, too.
        assert_eq!(cells("⚠️"), 2);
        assert_eq!(cells("ℹ️"), 2);
        assert_eq!(cells("…"), 1);
        assert_eq!(cells(""), 0);

        // Padded by cells, not by characters or bytes.
        assert_eq!(fitted("💀", 10, Cut::End), "💀        ");
        assert_eq!(fitted("⚠️", 10, Cut::End), "⚠️        ");
        assert_eq!(fitted("💀", 2, Cut::End), "💀");
    }

    #[test]
    fn a_cut_never_splits_an_emoji() {
        let text = "🚀🚀🚀🚀🚀";
        // Four cells of room hold two of them.
        assert_eq!(fitted(text, 5, Cut::End), "🚀🚀…");
        assert_eq!(fitted(text, 5, Cut::Start), "…🚀🚀");
        assert_eq!(fitted(text, 5, Cut::Middle), "🚀…🚀");
        // Three cells of room hold one, and the cell left over is padding.
        assert_eq!(fitted(text, 4, Cut::End), "🚀… ");
        assert_eq!(fitted(text, 4, Cut::Start), "…🚀 ");
        assert_eq!(fitted(text, 6, Cut::Middle), "🚀…🚀 ");
        // No room for one at all.
        assert_eq!(fitted(text, 2, Cut::End), "… ");
        for width in 1..=9 {
            for cut in [Cut::Start, Cut::End, Cut::Middle] {
                let text = fitted(text, width, cut);
                assert_eq!(cells(&text), usize::from(width), "{cut} {width}: {text:?}");
            }
        }

        // Mixed with letters.
        assert_eq!(fitted("api💀worker", 6, Cut::End), "api💀…");
        assert_eq!(fitted("api💀worker", 5, Cut::End), "api… ");
        assert_eq!(fitted("api💀worker", 10, Cut::Middle), "api💀…rker");
        assert_eq!(fitted("api💀worker", 8, Cut::Middle), "api…ker ");
        assert_eq!(fitted("api💀worker", 7, Cut::Middle), "api…ker");

        // The selector that makes an emoji of a sign stays with its sign,
        // and so do the parts of a joined emoji.
        assert_eq!(fitted("⚠️⚠️⚠️", 5, Cut::End), "⚠️⚠️…");
        assert_eq!(fitted("⚠️⚠️⚠️", 5, Cut::Start), "…⚠️⚠️");
        let family = "👨\u{200d}👩\u{200d}👧";
        let cut = fitted(&format!("{family}{family}{family}"), 3, Cut::End);
        assert!(cut == format!("{family}…") || cut == "…  ", "{cut:?}");
    }

    #[test]
    fn the_ends_of_a_text_are_kept_as_asked() {
        assert_eq!(keep_ends(POD, 6, 5, 20), "fc-api…vxfgs        ");
        assert_eq!(keep_ends(POD, 6, 5, 12), "fc-api…vxfgs");
        assert_eq!(keep_ends(POD, 0, 5, 6), "…vxfgs");
        assert_eq!(keep_ends(POD, 6, 0, 7), "fc-api…");
        // More than there is: the ends do not overlap.
        assert_eq!(keep_ends("abc", 2, 5, 6), "ab…c  ");
    }
}
