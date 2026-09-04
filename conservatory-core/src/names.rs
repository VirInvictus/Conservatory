//! The one home for name-to-sort-key rules (the 2026-08-23 sweep's "unified
//! name sorting"). Two shapes live here, deliberately distinct:
//!
//! - [`derive_sort_name`] is for *things* (albums, artists-as-bands): it moves
//!   a leading article ("The Tuss" -> "Tuss, The") and never inverts person
//!   names, because bands are not people.
//! - [`person_sort_name`] is for *people* (book authors and narrators): it
//!   inverts to last-name-first ("Patrick Rothfuss" -> "Rothfuss, Patrick",
//!   spec §4.5) and never moves articles.

/// Derive a sort name from a display name: move a leading article to the end
/// (`"The Tuss"` -> `"Tuss, The"`). Person-name inversion is deliberately *not*
/// attempted (bands are not people); `sort_name` is editable later. The reader
/// prefers an embedded `ARTISTSORT` tag over this (see the import resolver).
pub fn derive_sort_name(name: &str) -> String {
    let trimmed = name.trim();
    for article in ["The ", "A ", "An "] {
        if let Some(rest) = strip_prefix_ci(trimmed, article) {
            let rest = rest.trim();
            if !rest.is_empty() {
                return format!("{rest}, {}", article.trim());
            }
        }
    }
    trimmed.to_string()
}

/// Derive a Calibre-style person sort name ("Patrick Rothfuss" -> "Rothfuss,
/// Patrick"). A name already in "Last, First" form, or a single token, is left
/// as-is.
pub fn person_sort_name(name: &str) -> String {
    let name = name.trim();
    if name.contains(',') {
        return name.to_string();
    }
    match name.rsplit_once(char::is_whitespace) {
        Some((rest, last)) if !rest.trim().is_empty() && !last.is_empty() => {
            format!("{last}, {}", rest.trim())
        }
        _ => name.to_string(),
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_name_moves_leading_article() {
        assert_eq!(derive_sort_name("The Tuss"), "Tuss, The");
        assert_eq!(derive_sort_name("an Album"), "Album, An");
        assert_eq!(derive_sort_name("Aphex Twin"), "Aphex Twin"); // "A" only as a word
        assert_eq!(derive_sort_name("Boards of Canada"), "Boards of Canada");
    }

    #[test]
    fn person_sort_name_last_first() {
        assert_eq!(person_sort_name("Patrick Rothfuss"), "Rothfuss, Patrick");
        assert_eq!(person_sort_name("Neil Gaiman"), "Gaiman, Neil");
        assert_eq!(person_sort_name("Nick Podehl"), "Podehl, Nick");
        // Already sorted, or a single token, is untouched.
        assert_eq!(person_sort_name("Sanderson, Brandon"), "Sanderson, Brandon");
        assert_eq!(person_sort_name("Madonna"), "Madonna");
        // A three-part name puts the last token first.
        assert_eq!(person_sort_name("Ursula K Le Guin"), "Guin, Ursula K Le");
    }

    #[test]
    fn the_two_shapes_disagree_on_people() {
        // The reason they are separate functions, side by side: the article
        // rule leaves a plain person name alone; the person rule inverts it.
        assert_eq!(derive_sort_name("Patrick Rothfuss"), "Patrick Rothfuss");
        assert_eq!(person_sort_name("Patrick Rothfuss"), "Rothfuss, Patrick");
    }
}
