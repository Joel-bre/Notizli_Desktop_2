//! Pairing links and tokens.

/// URL scheme the Notizli pair page opens.
pub const SCHEME: &str = "notizli-sh";

/// A pairing token as issued by notizli.ch: `ccp_pair_` + base64url.
pub fn is_pairing_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix("ccp_pair_") else { return false };
    (8..=512).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
}

/// The pairing token in a `notizli-sh://pair?token=…` link, if the link is
/// exactly that and the token is well-formed. Anything else is ignored.
pub fn token_from_link(link: &str) -> Option<String> {
    let link = link.trim();
    let (scheme, rest) = link.split_once(':')?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return None;
    }
    let rest = rest.trim_start_matches('/');
    let (target, query) = rest.split_once('?')?;
    if !target.trim_end_matches('/').eq_ignore_ascii_case("pair") {
        return None;
    }
    let token = query.split('&').find_map(|kv| kv.strip_prefix("token="))?;
    let token = percent_decode(token)?;
    is_pairing_token(&token).then_some(token)
}

/// What the user pasted: a bare token, or a whole pairing link.
pub fn token_from_paste(text: &str) -> Option<String> {
    let t = text.trim();
    if is_pairing_token(t) {
        return Some(t.to_string());
    }
    token_from_link(t)
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// "Desktop (Windows)" / "Desktop (macOS)".
pub fn device_label() -> String {
    let os = match std::env::consts::OS {
        "windows" => "Windows",
        "macos" => "macOS",
        other => other,
    };
    format!("Desktop ({os})")
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "ccp_pair_AbCdEf0123456789-_xyz";

    #[test]
    fn accepts_real_links() {
        assert_eq!(token_from_link(&format!("notizli-sh://pair?token={T}")).as_deref(), Some(T));
        assert_eq!(token_from_link(&format!("notizli-sh://pair/?token={T}&x=1")).as_deref(), Some(T));
        assert_eq!(token_from_link(&format!("NOTIZLI-SH:pair?token={}", T.replace('_', "%5F"))).as_deref(), Some(T));
    }

    #[test]
    fn rejects_anything_else() {
        for bad in [
            format!("https://notizli.ch/pair?token={T}"),
            format!("notizli-sh://unpair?token={T}"),
            "notizli-sh://pair?token=ccp_abc".to_string(),
            "notizli-sh://pair?token=ccp_pair_<script>".to_string(),
            "notizli-sh://pair".to_string(),
            format!("notizli-sh://pair?tok={T}"),
        ] {
            assert_eq!(token_from_link(&bad), None, "{bad}");
        }
    }

    #[test]
    fn paste_takes_a_token_or_a_link() {
        assert_eq!(token_from_paste(&format!("  {T}\n")).as_deref(), Some(T));
        assert_eq!(token_from_paste(&format!("notizli-sh://pair?token={T}")).as_deref(), Some(T));
        assert_eq!(token_from_paste("hello"), None);
    }
}
