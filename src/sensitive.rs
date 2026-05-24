pub fn looks_sensitive(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();
    let sensitive_markers = [
        "password",
        "passwd",
        "pwd=",
        "api_key",
        "apikey",
        "access_key",
        "secret",
        "private_key",
        "client_secret",
        "access_token",
        "refresh_token",
        "authorization: bearer",
        "bearer ",
        "ssh-rsa ",
        "-----begin private key-----",
    ];
    if sensitive_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return true;
    }

    contains_luhn_candidate(trimmed) || looks_like_token(trimmed)
}

fn looks_like_token(text: &str) -> bool {
    let compact = text.trim();
    if compact.len() < 36 || compact.len() > 256 {
        return false;
    }
    if compact.contains(char::is_whitespace) {
        return false;
    }
    let mut has_lower = false;
    let mut has_upper = false;
    let mut has_digit = false;
    let mut has_symbol = false;
    for ch in compact.chars() {
        if ch.is_ascii_lowercase() {
            has_lower = true;
        } else if ch.is_ascii_uppercase() {
            has_upper = true;
        } else if ch.is_ascii_digit() {
            has_digit = true;
        } else if matches!(ch, '_' | '-' | '.' | ':' | '/' | '+' | '=') {
            has_symbol = true;
        } else {
            return false;
        }
    }
    has_digit && ((has_lower && has_upper) || has_symbol)
}

fn contains_luhn_candidate(text: &str) -> bool {
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
        } else {
            if is_luhn_candidate(&digits) {
                return true;
            }
            digits.clear();
        }
    }
    is_luhn_candidate(&digits)
}

fn is_luhn_candidate(digits: &str) -> bool {
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    luhn_valid(digits)
}

fn luhn_valid(digits: &str) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for ch in digits.chars().rev() {
        let Some(mut value) = ch.to_digit(10) else {
            return false;
        };
        if double {
            value *= 2;
            if value > 9 {
                value -= 9;
            }
        }
        sum += value;
        double = !double;
    }
    sum.is_multiple_of(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_obvious_secret_markers() {
        assert!(looks_sensitive("password = hunter2"));
        assert!(looks_sensitive("Authorization: Bearer abc.def.ghi"));
    }

    #[test]
    fn detects_luhn_card_number() {
        assert!(looks_sensitive("4111111111111111"));
    }

    #[test]
    fn ignores_normal_text() {
        assert!(!looks_sensitive("meeting notes for the clipboard manager"));
    }
}
