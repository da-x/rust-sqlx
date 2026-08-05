use super::MigrateError;

/// Parse a `SQUASH EPOCH` header from migration SQL.
///
/// Expected form on the first non-empty line:
///
/// ```sql
/// -- SQUASH EPOCH <hex1>,<hex2>,...,<hexN>
/// ```
///
/// Each hex blob is a lowercase (or uppercase) encoding of a migration checksum
/// (SHA-384, 48 bytes / 96 hex chars), in ascending applied-version order.
///
/// Returns `Ok(None)` when no epoch header is present.
pub fn parse_squash_epoch(sql: &str) -> Result<Option<Vec<Vec<u8>>>, MigrateError> {
    let first_line = sql.lines().find(|line| !line.trim().is_empty());
    let Some(first_line) = first_line else {
        return Ok(None);
    };

    let trimmed = first_line.trim();
    let Some(rest) = trimmed
        .strip_prefix("--")
        .map(str::trim_start)
        .and_then(|s| {
            // Accept "SQUASH EPOCH" case-insensitively for the keyword only.
            let upper = s.as_bytes();
            const PREFIX: &[u8] = b"SQUASH EPOCH";
            if upper.len() >= PREFIX.len()
                && upper[..PREFIX.len()].eq_ignore_ascii_case(PREFIX)
            {
                Some(s[PREFIX.len()..].trim_start())
            } else {
                None
            }
        })
    else {
        return Ok(None);
    };

    if rest.is_empty() {
        return Err(MigrateError::SquashEpochParse(
            "expected one or more hex checksums after SQUASH EPOCH".into(),
        ));
    }

    let mut checksums = Vec::new();
    for part in rest.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(MigrateError::SquashEpochParse(
                "empty checksum entry in SQUASH EPOCH list".into(),
            ));
        }
        checksums.push(decode_hex(part)?);
    }

    Ok(Some(checksums))
}

/// Format applied migration checksums as a `SQUASH EPOCH` header line (without trailing newline).
pub fn format_squash_epoch_header(checksums: &[impl AsRef<[u8]>]) -> String {
    let mut line = String::from("-- SQUASH EPOCH ");
    for (i, checksum) in checksums.iter().enumerate() {
        if i > 0 {
            line.push(',');
        }
        encode_hex_into(checksum.as_ref(), &mut line);
    }
    line
}

fn decode_hex(s: &str) -> Result<Vec<u8>, MigrateError> {
    if s.len() % 2 != 0 {
        return Err(MigrateError::SquashEpochParse(format!(
            "hex checksum has odd length ({})",
            s.len()
        )));
    }

    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = from_hex_digit(bytes[i])?;
        let lo = from_hex_digit(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn from_hex_digit(b: u8) -> Result<u8, MigrateError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(MigrateError::SquashEpochParse(format!(
            "invalid hex digit: {:?}",
            b as char
        ))),
    }
}

fn encode_hex_into(bytes: &[u8], out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_none_without_header() {
        assert!(parse_squash_epoch("CREATE TABLE t (id INT);")
            .unwrap()
            .is_none());
        assert!(parse_squash_epoch("").unwrap().is_none());
        assert!(parse_squash_epoch("\n\n  \nCREATE TABLE t;")
            .unwrap()
            .is_none());
    }

    #[test]
    fn parse_and_format_roundtrip() {
        let checksums = [vec![0xde, 0xad], vec![0xbe, 0xef, 0x00]];
        let header = format_squash_epoch_header(&checksums);
        assert_eq!(header, "-- SQUASH EPOCH dead,beef00");

        let sql = format!("{header}\nCREATE TABLE t (id INT);\n");
        let parsed = parse_squash_epoch(&sql).unwrap().unwrap();
        assert_eq!(parsed, checksums);
    }

    #[test]
    fn parse_rejects_bad_hex() {
        assert!(parse_squash_epoch("-- SQUASH EPOCH xyz\n").is_err());
        assert!(parse_squash_epoch("-- SQUASH EPOCH ab,\n").is_err());
        assert!(parse_squash_epoch("-- SQUASH EPOCH\n").is_err());
    }
}
