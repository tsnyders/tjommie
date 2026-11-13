//! Strings with a zod refinement or transform that JSON Schema can't express.

/// A string newtype whose `new` applies `$check`: `fn(&str) -> Option<String>` returns the (possibly normalized) value, or
/// `None` with `$message` as the error. Deserializing runs the same check. A `secret` one never shows its value in
/// `Debug` output.
macro_rules! refined_string {
    (@define $(#[$meta:meta])* $name:ident, $check:path, $message:literal) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Eq, Hash, serde::Serialize)]
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
    ($(#[$meta:meta])* secret $name:ident, $check:path, $message:literal) => {
        refined_string!(@define $(#[$meta])* $name, $check, $message);

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}([redacted])", stringify!($name))
            }
        }
    };
    ($(#[$meta:meta])* $name:ident, $check:path, $message:literal) => {
        refined_string!(@define $(#[$meta])* #[derive(Debug)] $name, $check, $message);
    };
}
