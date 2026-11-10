//! `newId` and `nowIso` from `db.ts`.

/// A random version-4 UUID, like `crypto.randomUUID()`.
pub fn new_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the operating system's random source works");
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// `new Date().toISOString()`: UTC with milliseconds, so stored times sort as text.
pub fn now_iso() -> String {
    jiff::Timestamp::now()
        .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_version_4_uuids_and_times_are_iso_with_milliseconds() {
        let id = new_id();
        assert_eq!((id.len(), &id[14..15]), (36, "4"));
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"), "{id}");
        assert_ne!(new_id(), id);
        let now = now_iso();
        assert_eq!(
            (now.len(), &now[10..11], &now[19..20], &now[23..]),
            (24, "T", ".", "Z"),
            "{now}"
        );
    }
}
