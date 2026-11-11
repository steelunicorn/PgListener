//! Шифрование сохранённых паролей: AES-256-GCM, ключ из мастер-пароля через Argon2id.
//! На Windows ключ можно запомнить на этом компьютере через DPAPI (привязка к учётной записи).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Result};

/// Префикс зашифрованного значения в базе.
const PREFIX: &str = "enc1:";
const NONCE_LEN: usize = 12;

#[derive(Clone)]
pub struct Key([u8; 32]);

impl Key {
    pub fn derive(password: &str, salt: &[u8]) -> Result<Self> {
        let mut out = [0u8; 32];
        argon2::Argon2::default()
            .hash_password_into(password.as_bytes(), salt, &mut out)
            .map_err(|e| anyhow!("argon2: {e}"))?;
        Ok(Self(out))
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Some(Self(b.try_into().ok()?))
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new_from_slice(&self.0).expect("32-byte key")
    }

    pub fn encrypt(&self, plain: &str) -> String {
        if plain.is_empty() {
            return String::new();
        }
        let nonce_bytes = random::<NONCE_LEN>();
        let nonce = Nonce::try_from(&nonce_bytes[..]).expect("nonce size");
        let ct = self
            .cipher()
            .encrypt(&nonce, plain.as_bytes())
            .expect("AES-GCM encrypt");
        let mut data = nonce_bytes.to_vec();
        data.extend_from_slice(&ct);
        format!("{PREFIX}{}", to_hex(&data))
    }

    /// Расшифровывает значение; незашифрованные (старые) значения возвращаются как есть.
    pub fn decrypt(&self, value: &str) -> Result<String> {
        let Some(hex) = value.strip_prefix(PREFIX) else {
            return Ok(value.to_string());
        };
        let data = from_hex(hex).ok_or_else(|| anyhow!("повреждённое значение"))?;
        if data.len() < NONCE_LEN {
            return Err(anyhow!("повреждённое значение"));
        }
        let (n, ct) = data.split_at(NONCE_LEN);
        let nonce = Nonce::try_from(n).map_err(|_| anyhow!("повреждённое значение"))?;
        let plain = self
            .cipher()
            .decrypt(&nonce, ct)
            .map_err(|_| anyhow!("неверный ключ"))?;
        Ok(String::from_utf8(plain)?)
    }
}

pub fn is_encrypted(value: &str) -> bool {
    value.starts_with(PREFIX)
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).expect("системный генератор случайных чисел");
    b
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn from_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Защищает данные ключом текущей учётной записи Windows.
#[cfg(windows)]
pub fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, true)
}

#[cfg(windows)]
pub fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    dpapi(data, false)
}

#[cfg(windows)]
fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: входной буфер живёт до конца вызова; выходной освобождается через LocalFree.
    unsafe {
        let ok = if protect {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 || output.pbData.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData as _);
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let k = Key::derive("секрет", b"0123456789abcdef").unwrap();
        let e = k.encrypt("пароль");
        assert!(is_encrypted(&e));
        assert_eq!(k.decrypt(&e).unwrap(), "пароль");
        assert_eq!(k.decrypt("plain").unwrap(), "plain");
        let other = Key::derive("другой", b"0123456789abcdef").unwrap();
        assert!(other.decrypt(&e).is_err());
        assert_eq!(k.encrypt(""), "");
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip() {
        let blob = dpapi_protect(b"key-bytes").unwrap();
        assert_ne!(blob, b"key-bytes");
        assert_eq!(dpapi_unprotect(&blob).unwrap(), b"key-bytes");
    }
}
