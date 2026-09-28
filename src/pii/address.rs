const KEYWORDS: &[&str] = &[
    "no",
    "road",
    "rd",
    "street",
    "st",
    "lane",
    "marg",
    "cross",
    "main",
    "nagar",
    "sector",
    "block",
    "phase",
    "floor",
    "flat",
    "house",
    "plot",
    "door",
    "apartment",
    "apartments",
    "apts",
    "society",
    "colony",
    "layout",
    "extension",
    "stage",
    "circle",
    "chowk",
    "bazar",
    "bazaar",
    "market",
    "near",
    "opp",
    "opposite",
    "behind",
    "beside",
    "tower",
    "towers",
    "building",
    "complex",
    "enclave",
    "residency",
    "vihar",
    "puram",
    "halli",
    "west",
    "east",
    "north",
    "south",
    "gali",
    "mohalla",
    "village",
    "post",
    "dist",
    "district",
    "taluk",
    "tehsil",
    "of",
    "the",
    "and",
    "survey",
    "villa",
];

const STRONG: &[&str] = &[
    "road",
    "rd",
    "street",
    "st",
    "lane",
    "marg",
    "cross",
    "nagar",
    "sector",
    "block",
    "phase",
    "floor",
    "flat",
    "house",
    "plot",
    "door",
    "apartment",
    "apartments",
    "apts",
    "society",
    "colony",
    "layout",
    "extension",
    "stage",
    "circle",
    "chowk",
    "bazar",
    "bazaar",
    "tower",
    "towers",
    "building",
    "complex",
    "enclave",
    "residency",
    "vihar",
    "puram",
    "gali",
    "mohalla",
    "village",
    "survey",
    "villa",
    "midc",
    "salai",
    "park",
];

const MAX_BACK: usize = 260;

fn core(t: &str) -> &str {
    t.trim_matches(|c: char| !c.is_alphanumeric())
}

fn addr_token(t: &str) -> bool {
    let c = core(t);
    if c.is_empty() {
        return true;
    }
    if c.chars().any(|ch| ch.is_ascii_digit()) {
        return true;
    }
    if c.chars().next().is_some_and(char::is_uppercase) {
        return true;
    }
    KEYWORDS.contains(&c.to_lowercase().as_str())
}

fn sentence_end(t: &str) -> bool {
    let c = core(t);
    t.trim_end_matches([')', '"', '\''])
        .ends_with(['.', '!', '?'])
        && c.chars().count() > 3
        && !KEYWORDS.contains(&c.to_lowercase().as_str())
}

fn is_pin_at(b: &[u8], s: usize) -> Option<usize> {
    let d = |i: usize| b.get(i).is_some_and(u8::is_ascii_digit);
    if s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'+' || b[s - 1] == b'_') {
        return None;
    }
    if !(b'1'..=b'9').contains(&b[s]) {
        return None;
    }
    let end = if (s..s + 6).all(d) && !d(s + 6) {
        s + 6
    } else if (s..s + 3).all(d) && b.get(s + 3) == Some(&b' ') && (s + 4..s + 7).all(d) && !d(s + 7)
    {
        s + 7
    } else {
        return None;
    };
    if b.get(end)
        .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
    {
        return None;
    }
    let before = &b[..s];
    let trimmed = before.iter().rposition(|c| !matches!(c, b' ' | b'-'));
    if let Some(p) = trimmed {
        if p + 1 < s && b[p].is_ascii_digit() {
            return None;
        }
    }
    let after = &b[end..];
    let k = after
        .iter()
        .position(|c| !matches!(c, b' ' | b'-'))
        .unwrap_or(after.len());
    if k > 0 && after.get(k).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    Some(end)
}

fn tokens(s: &str, base: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut at = None;
    for (i, c) in s.char_indices() {
        if c.is_whitespace() {
            if let Some(st) = at.take() {
                out.push((base + st, base + i));
            }
        } else if at.is_none() {
            at = Some(i);
        }
    }
    if let Some(st) = at {
        out.push((base + st, base + s.len()));
    }
    out
}

fn left_start(text: &str, pin: usize) -> Option<(usize, usize, bool)> {
    let mut lo = pin.saturating_sub(MAX_BACK);
    while !text.is_char_boundary(lo) {
        lo += 1;
    }
    let mut end = pin;
    let pre = &text[lo..end];
    let trimmed = pre.trim_end_matches([' ', '-', ':', '\t']);
    end = lo + trimmed.len();
    let low = trimmed.to_ascii_lowercase();
    for lead in ["pincode", "pin code", "pin"] {
        if low.ends_with(lead) {
            let cut = end - lead.len();
            if cut == lo || !text[..cut].ends_with(|c: char| c.is_alphanumeric()) {
                end = lo + text[lo..cut].trim_end_matches([' ', '-', ':', ',']).len();
            }
            break;
        }
    }
    let window = &text[lo..end];
    let hard = window
        .rfind([':', ';', '!', '?', '(', '"', '\u{201c}'])
        .map(|i| i + 1)
        .unwrap_or(0);
    let hard = match window[hard..].rfind("\n\n") {
        Some(i) => hard + i + 2,
        None => hard,
    };
    let mut segs: Vec<(usize, usize)> = Vec::new();
    let mut seg_end = window.len();
    for (i, c) in window.char_indices().rev() {
        if i < hard {
            break;
        }
        if c == ',' || c == '\n' {
            segs.push((i + c.len_utf8(), seg_end));
            seg_end = i;
        }
    }
    segs.push((hard, seg_end));
    let mut start: Option<usize> = None;
    let mut n_tok = 0;
    let mut n_seg = 0;
    let mut strong = false;
    for (a, b) in segs.into_iter().take(8) {
        let toks = tokens(&window[a..b], lo + a);
        if toks.is_empty() {
            continue;
        }
        let mut keep = toks.len();
        for (j, &(s, e)) in toks.iter().enumerate().rev() {
            let t = &text[s..e];
            if !addr_token(t) || (j + 1 < toks.len() && sentence_end(t)) || toks.len() - j > 7 {
                break;
            }
            keep = j;
        }
        while keep < toks.len()
            && matches!(
                core(&text[toks[keep].0..toks[keep].1])
                    .to_lowercase()
                    .as_str(),
                "of" | "the" | "and" | "near"
            )
        {
            keep += 1;
        }
        let taken = &toks[keep.min(toks.len())..];
        if taken.is_empty() {
            break;
        }
        for &(s, e) in taken {
            let c = core(&text[s..e]);
            if STRONG.contains(&c.to_lowercase().as_str())
                || c.chars().any(|ch| ch.is_ascii_digit())
            {
                strong = true;
            }
        }
        n_tok += taken.len();
        n_seg += 1;
        start = Some(taken[0].0);
        if keep > 0 {
            break;
        }
    }
    let start = start?;
    Some((start, n_tok, n_seg >= 2 || strong))
}

const STATES: &[&str] = &[
    "andhra pradesh",
    "arunachal pradesh",
    "assam",
    "bihar",
    "chhattisgarh",
    "goa",
    "gujarat",
    "haryana",
    "himachal pradesh",
    "jharkhand",
    "karnataka",
    "kerala",
    "madhya pradesh",
    "maharashtra",
    "manipur",
    "meghalaya",
    "mizoram",
    "nagaland",
    "odisha",
    "punjab",
    "rajasthan",
    "sikkim",
    "tamil nadu",
    "telangana",
    "tripura",
    "uttar pradesh",
    "uttarakhand",
    "west bengal",
    "delhi",
    "new delhi",
    "jammu and kashmir",
    "ladakh",
    "puducherry",
    "chandigarh",
    "india",
];

fn right_end(text: &str, pin_end: usize) -> usize {
    let mut end = pin_end;
    loop {
        let rest = &text[end..];
        let skip = rest.len() - rest.trim_start_matches([',', ' ', '-']).len();
        if skip == 0 || skip > 3 {
            return end;
        }
        let tail = &rest[skip..];
        let hit = STATES.iter().find(|st| {
            tail.len() >= st.len()
                && tail.is_char_boundary(st.len())
                && tail[..st.len()].eq_ignore_ascii_case(st)
                && !tail[st.len()..].starts_with(|c: char| c.is_alphanumeric())
        });
        match hit {
            Some(st) => end += skip + st.len(),
            None => return end,
        }
    }
}

pub fn find(text: &str) -> Vec<(usize, usize)> {
    let b = text.as_bytes();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let Some(pin_end) = is_pin_at(b, i) else {
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        };
        if let Some((start, n_tok, ok)) = left_start(text, i) {
            if ok && n_tok >= 2 && out.last().is_none_or(|l| l.1 <= start) {
                out.push((start, right_end(text, pin_end)));
            }
        }
        i = pin_end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(t: &str) -> Vec<&str> {
        find(t).into_iter().map(|(s, e)| &t[s..e]).collect()
    }

    #[test]
    fn indian_addresses() {
        assert_eq!(
            one("Ship it to Flat 302, Sai Krupa Apartments, 14th Cross, Indiranagar, Bengaluru 560038."),
            vec!["Flat 302, Sai Krupa Apartments, 14th Cross, Indiranagar, Bengaluru 560038"]
        );
        assert_eq!(
            one("My address is House No. 45, Sector 21, Gurugram, Haryana 122016."),
            vec!["House No. 45, Sector 21, Gurugram, Haryana 122016"]
        );
        assert_eq!(
            one("Deliver to 12/3 Gariahat Road, Ballygunge, Kolkata - 700019."),
            vec!["12/3 Gariahat Road, Ballygunge, Kolkata - 700019"]
        );
        assert_eq!(
            one("Mera naya pata hai 27, Anna Salai, T. Nagar, Chennai 600017."),
            vec!["27, Anna Salai, T. Nagar, Chennai 600017"]
        );
        assert_eq!(
            one("Invoice address is 7 Park Street, Kolkata 700016, West Bengal."),
            vec!["7 Park Street, Kolkata 700016, West Bengal"]
        );
        assert_eq!(
            one("Ghar ka address: Gali No. 4, Laxmi Nagar, Delhi 110092."),
            vec!["Gali No. 4, Laxmi Nagar, Delhi 110092"]
        );
        assert_eq!(
            one("Flat 9\nLake View Road\nKochi PIN: 682020"),
            vec!["Flat 9\nLake View Road\nKochi PIN: 682020"]
        );
    }

    #[test]
    fn not_addresses() {
        assert!(one("Your OTP is 482913, do not share").is_empty());
        assert!(one("Reference No. 562001 was closed").is_empty());
        assert!(one("Pune 411001 is the PIN").is_empty());
        assert!(one("call 98765 43210 or 022 234567").is_empty());
        assert!(one("card 4111 1111 1111 1111 and order 123456789").is_empty());
        assert!(one("revenue grew to 250000 this year").is_empty());
    }
}
