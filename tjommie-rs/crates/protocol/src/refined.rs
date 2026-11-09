//! Strings with a zod refinement or transform that JSON Schema can't express.

/// A string newtype whose `new` applies `$check`: `fn(&str) -> Option<String>` returns the (possibly normalized) value, or
/// `None` with `$message` as the error. Deserializing runs the same check.
macro_rules! refined_string {
    ($(#[$meta:meta])* $name:ident, $check:path, $message:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl AsRef<str>) -> Result<Self, $crate::Invalid> {
                $check(value.as_ref()).map(Self).ok_or_else(|| $crate::Invalid($message.into()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}
