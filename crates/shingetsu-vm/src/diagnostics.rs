//! Shared diagnostic helpers used by derive-generated converters and
//! (eventually) the type checker.
//!
//! Currently provides Jaro-Winkler "did you mean" suggestions for
//! unknown field/key names, modeled after `wezterm-dynamic`'s
//! `Error::possible_matches` so messages remain familiar to users
//! migrating from that ecosystem.

use bstr::BStr;
use std::fmt::Write as _;

/// Jaro-Winkler similarity threshold for a name to be considered a
/// "close match" worth suggesting.  Tuned to match wezterm-dynamic's
/// behavior for struct field suggestions.
pub const FIELD_SUGGEST_THRESHOLD: f64 = 0.8;

/// Maximum number of suggestions listed inline.  Past this the
/// `did you mean` line is replaced by a single "too many close
/// matches" hint to keep error messages readable when the user is
/// poking at a struct with many similar field names.
pub const FIELD_SUGGEST_MAX: usize = 5;

/// Maximum number of "other alternatives" listed inline.  Beyond
/// this we recommend consulting docs rather than dumping a long
/// field list into a one-line error.
pub const FIELD_SUGGEST_LIST_LIMIT: usize = 5;

/// Return the close-match candidates for `used` from `possible`,
/// sorted by descending Jaro-Winkler similarity.  Names with
/// similarity at or below [`FIELD_SUGGEST_THRESHOLD`] are excluded.
///
/// Inputs are byte slices to handle non-UTF8 lua keys; the
/// similarity comparison uses the lossy UTF-8 view of each name.
pub fn close_matches<'a>(used: &str, possible: &'a [&[u8]]) -> Vec<&'a [u8]> {
    let mut scored: Vec<(f64, &[u8])> = possible
        .iter()
        .map(|name| {
            let s = String::from_utf8_lossy(name);
            (strsim::jaro_winkler(used, s.as_ref()), *name)
        })
        .filter(|(score, _)| *score > FIELD_SUGGEST_THRESHOLD)
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, n)| n).collect()
}

/// Render a wezterm-dynamic-style "did you mean ..." message for an
/// unknown name.  Returns an empty string when there's nothing useful
/// to say (no close matches and no alternatives to list).
///
/// The output format intentionally matches wezterm-dynamic's
/// `Error::possible_matches` so migration tooling and existing user
/// expectations carry over unchanged.
pub fn render_field_suggestion(used: &str, possible: &[&[u8]]) -> String {
    let suggestions = close_matches(used, possible);

    let mut others: Vec<&[u8]> = possible
        .iter()
        .copied()
        .filter(|name| !suggestions.iter().any(|s| s == name))
        .collect();
    others.sort_unstable();

    let mut msg = String::new();
    let too_many_suggestions = suggestions.len() > FIELD_SUGGEST_MAX;
    let display_suggestions: &[&[u8]] = if too_many_suggestions {
        &[]
    } else {
        suggestions.as_slice()
    };

    match display_suggestions.len() {
        0 if too_many_suggestions => {
            msg.push_str(
                "Many fields share a similar name; consult the documentation \
                 for the full list.",
            );
        }
        0 => {}
        1 => {
            write!(msg, "Did you mean `{}`?", BStr::new(display_suggestions[0])).ok();
        }
        _ => {
            msg.push_str("Did you mean one of ");
            for (i, s) in display_suggestions.iter().enumerate() {
                if i > 0 {
                    msg.push_str(", ");
                }
                write!(msg, "`{}`", BStr::new(s)).ok();
            }
            msg.push('?');
        }
    }

    // If we already replaced suggestions with the truncation hint, don't
    // also dump a 45-name "others" list.
    if too_many_suggestions {
        return msg;
    }

    if !others.is_empty() {
        if others.len() > FIELD_SUGGEST_LIST_LIMIT && !suggestions.is_empty() {
            msg.push_str(
                " There are too many alternatives to list here; consult the documentation!",
            );
        } else if others.len() <= FIELD_SUGGEST_LIST_LIMIT {
            // Choose phrasing based on the number of *others* (the items
            // we're about to enumerate), not the suggestion count.  Same
            // shape regardless of whether we offered close matches.
            let prefix = match (suggestions.is_empty(), others.len()) {
                (true, 1) => "The only valid field is ",
                (true, _) => "Possible alternatives are ",
                (false, 1) => " The other option is ",
                (false, _) => " Other alternatives are ",
            };
            msg.push_str(prefix);
            for (i, name) in others.iter().enumerate() {
                if i > 0 {
                    msg.push_str(", ");
                }
                write!(msg, "`{}`", BStr::new(name)).ok();
            }
        }
    }

    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_match_picks_typo() {
        let possible: Vec<&[u8]> = vec![b"font_size", b"font_family", b"line_height"];
        let matches = close_matches("font_sze", &possible);
        k9::assert_equal!(matches, vec![&b"font_size"[..]]);
    }

    #[test]
    fn no_close_match_returns_empty_vec() {
        let possible: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let matches = close_matches("xyzzy", &possible);
        let empty: Vec<&[u8]> = vec![];
        k9::assert_equal!(matches, empty);
    }

    #[test]
    fn render_single_match_with_few_alternatives() {
        // `fooo` has high similarity to both `foo` and `foo_x`, so both
        // surface as suggestions.  Bare `bar` / `baz` go to alternatives.
        let possible: Vec<&[u8]> = vec![b"foo", b"bar", b"baz", b"foo_x"];
        let msg = render_field_suggestion("fooo", &possible);
        k9::assert_equal!(
            msg,
            "Did you mean one of `foo`, `foo_x`? Other alternatives are `bar`, `baz`"
        );
    }

    #[test]
    fn render_multi_match_lists_all() {
        let possible: Vec<&[u8]> = vec![b"foo_bar", b"foo_baz", b"unrelated"];
        let msg = render_field_suggestion("foo_bay", &possible);
        k9::assert_equal!(
            msg,
            "Did you mean one of `foo_bar`, `foo_baz`? The other option is `unrelated`"
        );
    }

    #[test]
    fn render_no_match_with_single_alternative_uses_singular_phrasing() {
        let possible: Vec<&[u8]> = vec![b"only_one"];
        let msg = render_field_suggestion("xyzzy", &possible);
        k9::assert_equal!(msg, "The only valid field is `only_one`");
    }

    #[test]
    fn render_no_match_lists_alternatives_when_short() {
        let possible: Vec<&[u8]> = vec![b"alpha", b"beta", b"gamma"];
        let msg = render_field_suggestion("xyzzy", &possible);
        k9::assert_equal!(msg, "Possible alternatives are `alpha`, `beta`, `gamma`");
    }

    #[test]
    fn render_long_field_list_truncates_with_doc_pointer() {
        // 50-field struct fixture per the migration plan: confirm
        // we don't dump 50 names into the message even when many
        // names share a similar prefix.
        let names: Vec<Vec<u8>> = (0..50)
            .map(|i| format!("field_{i:02}").into_bytes())
            .collect();
        let possible: Vec<&[u8]> = names.iter().map(|v| v.as_slice()).collect();
        let msg = render_field_suggestion("field_07x", &possible);
        k9::assert_equal!(
            msg,
            "Many fields share a similar name; consult the documentation for the full list."
        );
    }

    #[test]
    fn render_returns_empty_when_nothing_useful() {
        let possible: Vec<&[u8]> = vec![];
        let msg = render_field_suggestion("anything", &possible);
        k9::assert_equal!(msg, "");
    }
}
