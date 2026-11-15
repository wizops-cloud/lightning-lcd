//! Encodages purs : trame HID « BY » et en-têtes du canal bulk.

/// Taille d'un rapport HID de l'écran (sans report ID).
pub const HID_REPORT_LEN: usize = 1024;

/// Magic d'une image sur le canal bulk.
pub const FRAME_MAGIC: u32 = 0xA1C6_2B01;
/// Magic de la phase 1 de l'authentification (l'écran prouve son identité).
pub const AUTH_DEV_MAGIC: u32 = 0xA1C6_2B10;
/// Magic de la phase 2 de l'authentification (l'hôte renvoie le bloc signé).
pub const AUTH_HOST_MAGIC: u32 = 0xA1C6_2B11;
/// Format média JPEG annoncé par l'écran.
pub const FORMAT_JPEG: u16 = 0x10;

const DELIM: u8 = 0x5A;
const ESCAPE: u8 = 0x5B;

/// Encode un texte dans une trame BY :
/// `5A | longueur BE16 (= texte + 5) | texte | somme8 | 5A`.
///
/// Entre les deux délimiteurs, `5A` devient `5B 01` et `5B` devient `5B 02`.
/// La somme porte sur la longueur et le texte.
pub fn by_frame(text: &[u8]) -> Vec<u8> {
    let len = u16::try_from(text.len() + 5).expect("texte trop long pour une trame BY");
    let mut body = Vec::with_capacity(text.len() + 3);
    body.extend_from_slice(&len.to_be_bytes());
    body.extend_from_slice(text);
    let sum = body.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
    body.push(sum);

    let mut out = Vec::with_capacity(body.len() + 8);
    out.push(DELIM);
    for b in body {
        match b {
            DELIM => out.extend_from_slice(&[ESCAPE, 0x01]),
            ESCAPE => out.extend_from_slice(&[ESCAPE, 0x02]),
            other => out.push(other),
        }
    }
    out.push(DELIM);
    out
}

/// Rapport HID complet à écrire sur hidraw : report ID nul, trame, bourrage à zéro.
/// Renvoie `None` si la trame ne tient pas dans un rapport.
pub fn by_report(text: &[u8]) -> Option<Vec<u8>> {
    let frame = by_frame(text);
    if frame.len() > HID_REPORT_LEN {
        return None;
    }
    let mut report = vec![0u8; 1 + HID_REPORT_LEN];
    report[1..1 + frame.len()].copy_from_slice(&frame);
    Some(report)
}

/// Extrait le texte d'un rapport reçu (trame BY suivie de bourrage).
/// La somme de contrôle n'est pas vérifiée.
pub fn by_deframe(report: &[u8]) -> Option<Vec<u8>> {
    if report.first() != Some(&DELIM) {
        return None;
    }
    let end = report[1..].iter().position(|b| *b == DELIM)? + 1;
    let mut body = Vec::with_capacity(end);
    let mut iter = report[1..end].iter();
    while let Some(&b) = iter.next() {
        if b == ESCAPE {
            match iter.next()? {
                0x01 => body.push(DELIM),
                0x02 => body.push(ESCAPE),
                _ => return None,
            }
        } else {
            body.push(b);
        }
    }
    if body.len() < 3 {
        return None;
    }
    Some(body[2..body.len() - 1].to_vec())
}

/// Texte d'une commande `POST <cmd>` avec corps JSON.
pub fn post_request(cmd: &str, seq: u32, json: &str) -> String {
    format!(
        "POST {cmd} 1\r\nSeqNumber={seq}\r\nContentType=json\r\nContentLength={}\r\n\r\n{json}",
        json.len()
    )
}

/// Texte de la commande `POST conn` (lecture de l'état), datée en ms depuis l'epoch.
pub fn conn_request(epoch_ms: u128) -> String {
    format!("POST conn 1\r\nSeqNumber=1\r\nContentType=json\r\nDate={epoch_ms}\r\n\r\n")
}

/// En-tête de 20 octets du canal bulk : `magic, longueur, frame_id, format, 0, magic`
/// (little-endian).
pub fn bulk_header(magic: u32, length: u32, frame_id: u16, format: u16) -> [u8; 20] {
    let mut h = [0u8; 20];
    h[0..4].copy_from_slice(&magic.to_le_bytes());
    h[4..8].copy_from_slice(&length.to_le_bytes());
    h[8..10].copy_from_slice(&frame_id.to_le_bytes());
    h[10..12].copy_from_slice(&format.to_le_bytes());
    h[16..20].copy_from_slice(&magic.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    // Vecteurs produits par legacy/python/lcd.py (validé sur l'écran réel).
    #[test]
    fn by_frame_matches_python_with_escapes() {
        assert_eq!(
            hex(&by_frame(b"POST conn 1\r\nZ[x")),
            "5a0015504f535420636f6e6e20310d0a5b015b0278be5a"
        );
    }

    #[test]
    fn extended_display_request_matches_python() {
        let text = post_request("extendedDisplay", 110, r#"{"enable":true}"#);
        assert_eq!(
            hex(&by_frame(text.as_bytes())),
            "5a0061504f535420657874656e646564446973706c617920310d0a5365714e756d6265723d3131300d0a\
             436f6e74656e74547970653d6a736f6e0d0a436f6e74656e744c656e6774683d31350d0a0d0a\
             7b22656e61626c65223a747275657d045a"
        );
    }

    #[test]
    fn checksum_byte_is_escaped_too() {
        // Somme = 0x00 + 0x06 + 0x54 = 0x5A : l'octet de contrôle doit être échappé.
        assert_eq!(by_frame(b"T"), [0x5A, 0x00, 0x06, b'T', 0x5B, 0x01, 0x5A]);
        assert_eq!(by_deframe(&by_frame(b"T")).unwrap(), b"T");
    }

    #[test]
    fn deframe_roundtrip_ignores_padding() {
        let text = b"1 200\r\nAckNumber=2\r\n\r\n{\"a\":\"Z[\"}";
        let report = by_report(text).unwrap();
        assert_eq!(report.len(), 1025);
        assert_eq!(report[0], 0);
        assert_eq!(by_deframe(&report[1..]).unwrap(), text);
    }

    #[test]
    fn deframe_rejects_garbage() {
        assert!(by_deframe(&[0u8; 1024]).is_none());
        assert!(by_deframe(&[0x5A, 0x00]).is_none());
        assert!(by_deframe(&[0x5A, 0x5B, 0x07, 0x5A]).is_none());
    }

    #[test]
    fn report_too_long_is_rejected() {
        assert!(by_report(&[b'a'; 1020]).is_none());
        assert!(by_report(&[b'a'; 1019]).is_some());
    }

    #[test]
    fn bulk_headers_match_python() {
        assert_eq!(
            hex(&bulk_header(FRAME_MAGIC, 16630, 3, FORMAT_JPEG)),
            "012bc6a1f64000000300100000000000012bc6a1"
        );
        assert_eq!(
            hex(&bulk_header(AUTH_DEV_MAGIC, 256, 0, 0)),
            "102bc6a1000100000000000000000000102bc6a1"
        );
    }

    #[test]
    fn conn_request_text() {
        assert_eq!(
            conn_request(1794654018562),
            "POST conn 1\r\nSeqNumber=1\r\nContentType=json\r\nDate=1794654018562\r\n\r\n"
        );
    }
}
