//! `whatsapp:`-style links: what a click on `whatsapp://send?phone=…` or `https://wa.me/…` asks
//! for. A link only opens a chat with a phone number and, when it carries `text`, pre-fills the
//! composer with it; it never sends anything (one message per explicit user action).
//!
//! Accepted forms (scheme and host in any case, a `#fragment` ignored):
//!
//! - `whatsapp://send?phone=<number>&text=<text>` and `whatsapp://send/?phone=…`
//! - `https://wa.me/<number>?text=<text>` (a trailing `/` after the number is fine)
//! - `https://api.whatsapp.com/send?phone=<number>&text=<text>` and `…/send/?phone=…`
//!
//! The number may be written with `+`, spaces, dashes and parentheses; what is left must be 7 to
//! 15 digits. `text` is percent-decoded as a form value (`+` is a space) and must be UTF-8; an
//! empty `text` is no draft. Other parameters are ignored, and the first `phone` or `text` wins.
//! A group invite (`https://chat.whatsapp.com/<code>`) is [`LinkError::GroupInvite`]; everything
//! else is [`LinkError::NotAWhatsAppLink`].

/// The longest draft a link may carry, in characters: WhatsApp's own message limit.
pub const MAX_DRAFT_CHARS: usize = 65_536;

/// The chat a link opens and the draft it pre-fills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    /// The phone-number chat JID, `<digits>@s.whatsapp.net`.
    pub jid: String,
    /// Text for the composer; never sent by itself.
    pub draft: Option<String>,
}

/// Why a link was not opened.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error("not a WhatsApp chat link")]
    NotAWhatsAppLink,
    #[error("group invite links are not supported yet")]
    GroupInvite,
    #[error("the link names no phone number")]
    MissingPhone,
    #[error("the link's phone number is not 7 to 15 digits")]
    BadPhone,
    #[error("the link's text is not valid percent-encoded UTF-8")]
    BadEncoding,
    #[error("the link's text is longer than {MAX_DRAFT_CHARS} characters")]
    TextTooLong,
}

/// Parse one link (see the module docs for what is accepted).
pub fn parse_link(link: &str) -> Result<LinkTarget, LinkError> {
    let link = link.trim();
    let link = link.split_once('#').map_or(link, |(before, _)| before);
    let (scheme, rest) = link.split_once(':').ok_or(LinkError::NotAWhatsAppLink)?;
    let rest = rest.strip_prefix("//").ok_or(LinkError::NotAWhatsAppLink)?;
    let (place, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, path) = place.split_once('/').unwrap_or((place, ""));
    let scheme = scheme.to_ascii_lowercase();
    let host = host.to_ascii_lowercase();
    let params = Params::new(query);

    let phone = match (scheme.as_str(), host.as_str()) {
        ("whatsapp", "send") if path.is_empty() => params.raw("phone"),
        ("https", "api.whatsapp.com") if path == "send" || path == "send/" => params.raw("phone"),
        ("https", "wa.me") => {
            let number = path.strip_suffix('/').unwrap_or(path);
            if number.is_empty() || number.contains('/') {
                return Err(LinkError::NotAWhatsAppLink);
            }
            Some(number)
        }
        ("https", "chat.whatsapp.com") => return Err(LinkError::GroupInvite),
        ("whatsapp", "chat") if params.raw("code").is_some() => {
            return Err(LinkError::GroupInvite);
        }
        _ => return Err(LinkError::NotAWhatsAppLink),
    };
    let phone = phone.ok_or(LinkError::MissingPhone)?;
    let digits = normalise_phone(&decode(phone).map_err(|_| LinkError::BadPhone)?)?;
    let draft = match params.raw("text").map(decode).transpose()? {
        Some(text) if text.chars().count() > MAX_DRAFT_CHARS => {
            return Err(LinkError::TextTooLong);
        }
        Some(text) if !text.is_empty() => Some(text),
        _ => None,
    };
    Ok(LinkTarget {
        jid: format!("{digits}@s.whatsapp.net"),
        draft,
    })
}

/// The digits of a written phone number: `+`, spaces, dashes, dots and parentheses removed; 7 to
/// 15 ASCII digits must remain (E.164 allows at most 15).
fn normalise_phone(written: &str) -> Result<String, LinkError> {
    let mut digits = String::new();
    for c in written.chars() {
        match c {
            '0'..='9' => digits.push(c),
            '+' | ' ' | '-' | '.' | '(' | ')' => {}
            _ => return Err(LinkError::BadPhone),
        }
    }
    if (7..=15).contains(&digits.len()) {
        Ok(digits)
    } else {
        Err(LinkError::BadPhone)
    }
}

/// A query string's raw `key=value` pairs, in order.
struct Params<'a> {
    pairs: Vec<(&'a str, &'a str)>,
}

impl<'a> Params<'a> {
    fn new(query: &'a str) -> Self {
        let pairs = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
            .collect();
        Self { pairs }
    }

    /// The first value of `key`, still percent-encoded.
    fn raw(&self, key: &str) -> Option<&'a str> {
        self.pairs
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
    }
}

/// Percent-decode a form value: `%XX` is a byte, `+` a space; the bytes must be UTF-8.
fn decode(value: &str) -> Result<String, LinkError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes.get(i + 1..i + 3).ok_or(LinkError::BadEncoding)?;
                let hex = std::str::from_utf8(hex).map_err(|_| LinkError::BadEncoding)?;
                if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(LinkError::BadEncoding);
                }
                let byte = u8::from_str_radix(hex, 16).map_err(|_| LinkError::BadEncoding)?;
                out.push(byte);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| LinkError::BadEncoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(digits: &str, draft: Option<&str>) -> Result<LinkTarget, LinkError> {
        Ok(LinkTarget {
            jid: format!("{digits}@s.whatsapp.net"),
            draft: draft.map(str::to_owned),
        })
    }

    #[test]
    fn every_accepted_form_opens_the_chat_with_its_draft() {
        for (link, want) in [
            (
                "whatsapp://send?phone=15551230001&text=Hello",
                target("15551230001", Some("Hello")),
            ),
            (
                "whatsapp://send/?phone=15551230001&text=Hello",
                target("15551230001", Some("Hello")),
            ),
            (
                "https://wa.me/15551230001?text=Hello",
                target("15551230001", Some("Hello")),
            ),
            (
                "https://wa.me/15551230001/?text=Hello",
                target("15551230001", Some("Hello")),
            ),
            (
                "https://api.whatsapp.com/send?phone=15551230001&text=Hello",
                target("15551230001", Some("Hello")),
            ),
            (
                "https://api.whatsapp.com/send/?phone=15551230001&text=Hello",
                target("15551230001", Some("Hello")),
            ),
            (
                "WhatsApp://SEND?phone=15551230001",
                target("15551230001", None),
            ),
            ("HTTPS://WA.ME/15551230001", target("15551230001", None)),
        ] {
            assert_eq!(parse_link(link), want, "{link}");
        }
    }

    #[test]
    fn wa_me_with_no_text_is_no_draft() {
        assert_eq!(
            parse_link("https://wa.me/15551230002"),
            target("15551230002", None)
        );
        assert_eq!(
            parse_link("https://wa.me/15551230002?text="),
            target("15551230002", None)
        );
    }

    #[test]
    fn the_phone_is_normalised_to_digits() {
        for (link, digits) in [
            ("whatsapp://send?phone=%2B15551230003", "15551230003"),
            ("whatsapp://send?phone=+1+555+123+0003", "15551230003"),
            ("whatsapp://send?phone=+1%20(555)%20123-0003", "15551230003"),
            ("https://wa.me/+1-555-123-0003", "15551230003"),
            ("https://wa.me/1555123", "1555123"),
            ("https://wa.me/155512300031234", "155512300031234"),
        ] {
            assert_eq!(parse_link(link), target(digits, None), "{link}");
        }
    }

    #[test]
    fn a_phone_that_is_not_7_to_15_digits_is_rejected() {
        for link in [
            "https://wa.me/155512",
            "https://wa.me/1555123000312345",
            "whatsapp://send?phone=1555abc0001",
            "whatsapp://send?phone=",
            "whatsapp://send?phone=%ZZ",
            "https://wa.me/1555%2A1230001",
        ] {
            assert_eq!(parse_link(link), Err(LinkError::BadPhone), "{link}");
        }
    }

    #[test]
    fn a_link_with_no_phone_is_rejected() {
        for link in [
            "whatsapp://send?text=Hello",
            "whatsapp://send",
            "https://api.whatsapp.com/send?text=Hello",
        ] {
            assert_eq!(parse_link(link), Err(LinkError::MissingPhone), "{link}");
        }
    }

    #[test]
    fn percent_encoded_text_and_emoji_are_decoded() {
        for (link, draft) in [
            (
                "https://wa.me/15551230004?text=Hola%20%F0%9F%98%80",
                "Hola 😀",
            ),
            ("https://wa.me/15551230004?text=a+b%2Bc", "a b+c"),
            ("https://wa.me/15551230004?text=%C3%B1and%C3%BA", "ñandú"),
            (
                "whatsapp://send?text=line%0Atwo&phone=15551230004",
                "line\ntwo",
            ),
            ("https://wa.me/15551230004?text=x%3Dy%26z", "x=y&z"),
            ("https://wa.me/15551230004?text=%f0%9f%91%8d", "👍"),
        ] {
            assert_eq!(
                parse_link(link),
                target("15551230004", Some(draft)),
                "{link}"
            );
        }
    }

    #[test]
    fn bad_encoding_in_the_text_is_rejected() {
        for link in [
            "https://wa.me/15551230005?text=%",
            "https://wa.me/15551230005?text=%4",
            "https://wa.me/15551230005?text=%G1",
            "https://wa.me/15551230005?text=%FF%FE",
            "https://wa.me/15551230005?text=%+1",
        ] {
            assert_eq!(parse_link(link), Err(LinkError::BadEncoding), "{link}");
        }
    }

    #[test]
    fn unknown_parameters_and_a_fragment_are_ignored_and_the_first_value_wins() {
        assert_eq!(
            parse_link("https://wa.me/15551230006?lang=es&text=Hi&text=Bye&type=phone_number#x"),
            target("15551230006", Some("Hi"))
        );
        assert_eq!(
            parse_link("whatsapp://send?app_absent=0&phone=15551230006&phone=15551230007"),
            target("15551230006", None)
        );
    }

    #[test]
    fn the_text_is_capped_at_whatsapps_limit() {
        let at_limit = "é".repeat(MAX_DRAFT_CHARS);
        let link = format!(
            "https://wa.me/15551230008?text={}",
            "%C3%A9".repeat(MAX_DRAFT_CHARS)
        );
        assert_eq!(
            parse_link(&link),
            target("15551230008", Some(&at_limit)),
            "the limit counts characters, not bytes"
        );
        let over = format!(
            "https://wa.me/15551230008?text={}",
            "a".repeat(MAX_DRAFT_CHARS + 1)
        );
        assert_eq!(parse_link(&over), Err(LinkError::TextTooLong));
    }

    #[test]
    fn group_invites_have_their_own_error() {
        for link in [
            "https://chat.whatsapp.com/AbCdEfGhIjK",
            "https://CHAT.whatsapp.com/AbCdEfGhIjK",
            "whatsapp://chat?code=AbCdEfGhIjK",
        ] {
            assert_eq!(parse_link(link), Err(LinkError::GroupInvite), "{link}");
        }
    }

    #[test]
    fn everything_else_is_not_a_whatsapp_link() {
        for link in [
            "",
            "hello",
            "https://example.com/send?phone=15551230009",
            "https://wa.me.example.com/15551230009",
            "http://wa.me/15551230009",
            "ftp://wa.me/15551230009",
            "https://wa.me/",
            "https://wa.me/message/ABCDEF",
            "https://whatsapp.com/channel/0029Va",
            "https://api.whatsapp.com/other?phone=15551230009",
            "whatsapp://call?phone=15551230009",
            "whatsapp://send/extra?phone=15551230009",
            "whatsapp:send?phone=15551230009",
            "mailto:someone@example.com",
        ] {
            assert_eq!(
                parse_link(link),
                Err(LinkError::NotAWhatsAppLink),
                "{link:?}"
            );
        }
    }

    #[test]
    fn a_link_never_targets_anything_but_a_phone_number_chat() {
        let parsed = parse_link("https://wa.me/15551230010?text=hi").expect("parses");
        assert!(parsed.jid.ends_with("@s.whatsapp.net"));
        assert_eq!(parsed.jid, "15551230010@s.whatsapp.net");
    }
}
