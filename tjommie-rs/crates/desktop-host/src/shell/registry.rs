//! The little of the Windows registry the shell code needs, behind a trait so login-item, speech-permission and
//! diagnostics logic can be tested without touching the real registry. Writes only ever go to the current user's hive.

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_SET_VALUE, REG_BINARY,
    REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

pub trait Registry: Send + Sync {
    fn read_string(&self, hive: Hive, key: &str, name: &str) -> Option<String>;
    fn read_binary(&self, hive: Hive, key: &str, name: &str) -> Option<Vec<u8>>;
    /// Current user only.
    fn write_string(&self, key: &str, name: &str, value: &str) -> Result<(), String>;
    /// Current user only. A value that isn't there is not an error.
    fn delete_value(&self, key: &str, name: &str) -> Result<(), String>;
}

/// The real registry.
pub struct Windows;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn root(hive: Hive) -> HKEY {
    match hive {
        Hive::CurrentUser => HKEY_CURRENT_USER,
        Hive::LocalMachine => HKEY_LOCAL_MACHINE,
    }
}

fn query(hive: Hive, key: &str, name: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let key = wide(key);
    let name = wide(name);
    // SAFETY: the key handle is opened and closed here; the buffers outlive the calls.
    unsafe {
        let mut handle = HKEY::default();
        if RegOpenKeyExW(
            root(hive),
            PCWSTR(key.as_ptr()),
            None,
            KEY_READ,
            &mut handle,
        ) != ERROR_SUCCESS
        {
            return None;
        }
        let mut kind = REG_VALUE_TYPE(0);
        let mut size = 0u32;
        let name = PCWSTR(name.as_ptr());
        let sized = RegQueryValueExW(handle, name, None, Some(&mut kind), None, Some(&mut size));
        let mut data = vec![0u8; size as usize];
        let read = sized == ERROR_SUCCESS
            && RegQueryValueExW(
                handle,
                name,
                None,
                Some(&mut kind),
                Some(data.as_mut_ptr()),
                Some(&mut size),
            ) == ERROR_SUCCESS;
        let _ = RegCloseKey(handle);
        if !read {
            return None;
        }
        data.truncate(size as usize);
        Some((kind, data))
    }
}

impl Registry for Windows {
    fn read_string(&self, hive: Hive, key: &str, name: &str) -> Option<String> {
        let (kind, data) = query(hive, key, name)?;
        if kind != REG_SZ {
            return None;
        }
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|pair| {
                u16::from_le_bytes([
                    pair.first().copied().unwrap_or(0),
                    pair.get(1).copied().unwrap_or(0),
                ])
            })
            .collect();
        let end = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        String::from_utf16(units.get(..end)?).ok()
    }

    fn read_binary(&self, hive: Hive, key: &str, name: &str) -> Option<Vec<u8>> {
        let (kind, data) = query(hive, key, name)?;
        (kind == REG_BINARY).then_some(data)
    }

    fn write_string(&self, key: &str, name: &str, value: &str) -> Result<(), String> {
        let key = wide(key);
        let name = wide(name);
        let bytes: Vec<u8> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
        // SAFETY: the key handle is created and closed here; the buffers outlive the calls.
        unsafe {
            let mut handle = HKEY::default();
            let created = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut handle,
                None,
            );
            if created != ERROR_SUCCESS {
                return Err(format!(
                    "The registry key could not be opened ({})",
                    created.0
                ));
            }
            let written = RegSetValueExW(handle, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&bytes));
            let _ = RegCloseKey(handle);
            if written == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!(
                    "The registry value could not be written ({})",
                    written.0
                ))
            }
        }
    }

    fn delete_value(&self, key: &str, name: &str) -> Result<(), String> {
        let key = wide(key);
        let name = wide(name);
        // SAFETY: the key handle is opened and closed here.
        unsafe {
            let mut handle = HKEY::default();
            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(key.as_ptr()),
                None,
                KEY_SET_VALUE,
                &mut handle,
            ) != ERROR_SUCCESS
            {
                return Ok(()); // no key, so no value
            }
            let deleted = RegDeleteValueW(handle, PCWSTR(name.as_ptr()));
            let _ = RegCloseKey(handle);
            // ERROR_FILE_NOT_FOUND: the value wasn't there.
            if deleted == ERROR_SUCCESS || deleted.0 == 2 {
                Ok(())
            } else {
                Err(format!(
                    "The registry value could not be deleted ({})",
                    deleted.0
                ))
            }
        }
    }
}

/// Deletes a whole current-user key, for tests that made one.
#[cfg(test)]
pub(crate) fn delete_tree(key: &str) {
    use windows::Win32::System::Registry::RegDeleteTreeW;
    let key = wide(key);
    // SAFETY: removes only the test key the caller made.
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(key.as_ptr()));
    }
}

/// A registry in memory, for tests. Counts writes so a test can assert that nothing was touched.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Fake {
    pub strings: std::sync::Mutex<std::collections::HashMap<(String, String), String>>,
    pub binaries: std::sync::Mutex<std::collections::HashMap<(String, String), Vec<u8>>>,
    pub writes: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl Fake {
    pub fn set_binary(&self, key: &str, name: &str, value: &[u8]) {
        if let Ok(mut map) = self.binaries.lock() {
            map.insert((key.into(), name.into()), value.to_vec());
        }
    }
    pub fn set_string(&self, key: &str, name: &str, value: &str) {
        if let Ok(mut map) = self.strings.lock() {
            map.insert((key.into(), name.into()), value.into());
        }
    }
    pub fn writes(&self) -> usize {
        self.writes.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Registry for Fake {
    fn read_string(&self, _hive: Hive, key: &str, name: &str) -> Option<String> {
        self.strings
            .lock()
            .ok()?
            .get(&(key.into(), name.into()))
            .cloned()
    }
    fn read_binary(&self, _hive: Hive, key: &str, name: &str) -> Option<Vec<u8>> {
        self.binaries
            .lock()
            .ok()?
            .get(&(key.into(), name.into()))
            .cloned()
    }
    fn write_string(&self, key: &str, name: &str, value: &str) -> Result<(), String> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.set_string(key, name, value);
        Ok(())
    }
    fn delete_value(&self, key: &str, name: &str) -> Result<(), String> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Ok(mut map) = self.strings.lock() {
            map.remove(&(key.to_string(), name.to_string()));
        }
        if let Ok(mut map) = self.binaries.lock() {
            map.remove(&(key.to_string(), name.to_string()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_real_registry_round_trips_under_a_private_test_key() {
        let key = format!(r"Software\TjommieRsTest\{}", uuid::Uuid::new_v4());
        let registry = Windows;
        assert_eq!(registry.read_string(Hive::CurrentUser, &key, "v"), None);
        registry
            .write_string(&key, "v", r#""C:\Program Files\x y\a.exe" --hidden"#)
            .unwrap();
        assert_eq!(
            registry
                .read_string(Hive::CurrentUser, &key, "v")
                .as_deref(),
            Some(r#""C:\Program Files\x y\a.exe" --hidden"#)
        );
        // A string is not a binary value.
        assert_eq!(registry.read_binary(Hive::CurrentUser, &key, "v"), None);
        registry.delete_value(&key, "v").unwrap();
        registry.delete_value(&key, "v").unwrap(); // already gone
        assert_eq!(registry.read_string(Hive::CurrentUser, &key, "v"), None);
        delete_tree(&key);
        delete_tree(r"Software\TjommieRsTest");
        assert_eq!(registry.read_string(Hive::CurrentUser, &key, "v"), None);
    }
}
