//! RSA « clé publique seule » de l'authentification ArtInChip.
//!
//! La clé publique RSA-2048 est celle des écrans ArtInChip (identique dans les
//! pilotes AiCast libres). Aucun secret côté hôte.

use num_bigint::BigUint;

use crate::error::{Error, Result};

/// Taille d'un bloc RSA, en octets.
pub const BLOCK_LEN: usize = 256;
const EXPONENT: u32 = 65537;
const MODULUS_HEX: &[u8] = b"c9b76dbc1d6e340e1720287ec498b528958ed0661a97894d896ebdcd23082451\
b36f6c24885057dadc45687e59621d1361dc268f306a8a7cd3b168486ac5b7ae\
5f5037972fef7cc56a53b72788400d7cf583b8651d507404a0c3dbcd0d50a506\
01bc03bfe28c1d3a6bb3e07f4f84146b4b384413673787cc3e4d87d140165358\
ca02f3188e1fd847e30b61b974e1908b94e7d7f7342344155a9fb68b011420bf\
94116a102502806c60af3ae15ac12aaf75951097b785cfe5dfec0ed64cee419c\
a4b8b68b8c47b5172ddfb33e6e466f2dfff7695a4056ca99cb93f635711ee873\
2fe10f983fa40acf6c2e937c566075859fef3cc9d8efdfde2d376f2b8e5e7a87";

fn public_op(block: &[u8]) -> [u8; BLOCK_LEN] {
    let n = BigUint::parse_bytes(MODULUS_HEX, 16).expect("module RSA valide");
    let out = BigUint::from_bytes_be(block)
        .modpow(&BigUint::from(EXPONENT), &n)
        .to_bytes_be();
    let mut padded = [0u8; BLOCK_LEN];
    padded[BLOCK_LEN - out.len()..].copy_from_slice(&out);
    padded
}

/// Chiffrement PKCS#1 v1.5 (type 2) avec le bourrage fourni.
///
/// `padding` doit contenir au moins `BLOCK_LEN - 3 - msg.len()` octets ; les
/// octets nuls y sont remplacés par `0x01`.
pub fn encrypt_with_padding(msg: &[u8], padding: &[u8]) -> Result<[u8; BLOCK_LEN]> {
    if msg.len() > BLOCK_LEN - 11 {
        return Err(Error::OutOfRange("message RSA trop long"));
    }
    let pad_len = BLOCK_LEN - 3 - msg.len();
    if padding.len() < pad_len {
        return Err(Error::OutOfRange("bourrage RSA trop court"));
    }
    let mut em = Vec::with_capacity(BLOCK_LEN);
    em.extend_from_slice(&[0x00, 0x02]);
    em.extend(padding[..pad_len].iter().map(|b| (*b).max(1)));
    em.push(0x00);
    em.extend_from_slice(msg);
    Ok(public_op(&em))
}

/// Chiffrement PKCS#1 v1.5 avec un bourrage aléatoire tiré de `/dev/urandom`.
pub fn encrypt(msg: &[u8]) -> Result<[u8; BLOCK_LEN]> {
    let mut padding = [0u8; BLOCK_LEN];
    random_bytes(&mut padding)?;
    encrypt_with_padding(msg, &padding)
}

/// Applique la clé publique à un bloc « signé » et retire le bourrage PKCS#1 type 1.
pub fn public_recover(block: &[u8]) -> Result<Vec<u8>> {
    if block.len() != BLOCK_LEN {
        return Err(Error::Auth("bloc signé de taille inattendue"));
    }
    let em = public_op(block);
    if em[0] != 0x00 || em[1] != 0x01 {
        return Err(Error::Auth("bourrage PKCS#1 type 1 attendu"));
    }
    let sep = em[2..]
        .iter()
        .position(|b| *b != 0xFF)
        .map(|i| i + 2)
        .ok_or(Error::Auth("séparateur PKCS#1 absent"))?;
    if em[sep] != 0x00 {
        return Err(Error::Auth("séparateur PKCS#1 invalide"));
    }
    Ok(em[sep + 1..].to_vec())
}

/// Remplit `buf` d'octets aléatoires.
pub fn random_bytes(buf: &mut [u8]) -> Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(buf)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        let s: String = s.split_whitespace().collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    // Vecteur calculé par legacy/python/lcd.py : message 00..1f, bourrage 0xAB.
    #[test]
    fn encrypt_matches_python() {
        let msg: Vec<u8> = (0u8..32).collect();
        let out = encrypt_with_padding(&msg, &[0xAB; BLOCK_LEN]).unwrap();
        let expected = unhex(
            "4383913566211a1de56f0b8100602855c816fd38945346de22a2bb2633da9187764be05680b385e9
             f4fa7eeeb242f9c91879e621ecf577cf787b6c34c60d1fcbd3b6e023dc79483df0f93176d5541ccb
             fe51b71c0db40652bd804e7aab17b2020f84d267f25ae24777092ea668271fba5fdc8aaef5a3332a
             e3048b975e279fdf3bc847a4634e3427134a88399cdb452b286a02c084555713694e9af157d68144
             a5521d0a6be283b968b74eb620414eee2dd4d21d83b35570157b6ac619c1b309bccb37f21c584a49
             c711e9e19554630eed9bbcbf393782e7392f39d18719e73c176125b68a0170d18cea04992b4c49fe
             96b177796e01836351cea3dcc65fa1df",
        );
        assert_eq!(out.to_vec(), expected);
    }

    // Bloc réellement renvoyé par l'écran en phase 2 (14/11/2026).
    #[test]
    fn public_recover_real_device_block() {
        let block = unhex(
            "2a753dfa002907f6588120b18c7316978ab284786f836c23d1b0161deab5fa3844cfad22a58714fb
             a340281cdc07f7bc86c0d32fe83a4091e687291e520fc79d0cac0f3f331c675ad712c368bddc3bbf
             5e22c1a106a75d61db5731f348100959782eeba9dfa2b49a8eeebc861e0655274e4df84a948b3633
             9e42f8d88bbd6cbbe21056251075a7360b80f96f65d6335dcf04990b41084a6e19c0ac41080274cc
             74291d328907ffa57eec10605cde745aff78790f4edec550325dbd201c4b454d7fa6cfc7d6553d2f
             0e784ff9447386e18dd72e30691165b1276515e83e744d29abe0672253587242de43adfcdb4a40bc
             f32ad4fea1b98a15be8a8fef097bd063",
        );
        let plain = public_recover(&block).unwrap();
        assert_eq!(plain.len(), 153);
        assert_eq!(&plain[..4], &[0xDD, 0xDD, 0xDE, 0xDE]);
        assert_eq!(plain[152], 0xED);
    }

    #[test]
    fn public_recover_rejects_bad_blocks() {
        assert!(public_recover(&[0u8; 12]).is_err());
        assert!(public_recover(&[0x42u8; BLOCK_LEN]).is_err());
    }

    #[test]
    fn padding_never_contains_zero() {
        let out_a = encrypt_with_padding(b"x", &[0u8; BLOCK_LEN]).unwrap();
        let out_b = encrypt_with_padding(b"x", &[1u8; BLOCK_LEN]).unwrap();
        assert_eq!(out_a, out_b);
        assert!(encrypt_with_padding(&[0u8; 250], &[1u8; BLOCK_LEN]).is_err());
    }

    #[test]
    fn random_encrypt_differs_each_time() {
        assert_ne!(encrypt(b"abc").unwrap(), encrypt(b"abc").unwrap());
    }
}
