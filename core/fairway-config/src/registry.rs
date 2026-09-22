use serde::de::DeserializeOwned;

use crate::{Error, Namespace, Value};

#[doc(hidden)]
pub struct Registration {
    pub(crate) name: &'static str,
    pub(crate) source: &'static str,
    pub(crate) default: fn() -> Value,
    pub(crate) deserialize: fn(toml::Value) -> Result<Value, toml::de::Error>,
}

impl Registration {
    #[doc(hidden)]
    pub const fn new<T: Default + DeserializeOwned + Send + Sync + 'static>(
        namespace: &'static Namespace<T>,
        source: &'static str,
    ) -> Self {
        Self {
            name: namespace.name,
            source,
            default: || Box::new(T::default()),
            deserialize: |value| value.try_into::<T>().map(|value| Box::new(value) as Value),
        }
    }
}

#[doc(hidden)]
#[linkme::distributed_slice]
pub static NAMESPACES: [Registration];

pub(crate) const fn valid_name(name: &str) {
    assert!(
        !name.is_empty(),
        "a configuration namespace cannot be empty"
    );
    let bytes = name.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Decode one scalar in const evaluation; str::chars is not const.
        // The input is &str, so every sequence is already valid UTF-8.
        let first = bytes[i];
        let (mut scalar, width) = match first {
            0..=0x7f => (first as u32, 1),
            0xc0..=0xdf => ((first & 0x1f) as u32, 2),
            0xe0..=0xef => ((first & 0x0f) as u32, 3),
            _ => ((first & 0x07) as u32, 4),
        };
        let mut offset = 1;
        while offset < width {
            scalar = (scalar << 6) | (bytes[i + offset] & 0x3f) as u32;
            offset += 1;
        }
        let character = char::from_u32(scalar).expect("a str contains valid Unicode scalars");
        assert!(
            !character.is_whitespace() && !character.is_control(),
            "a configuration namespace cannot contain whitespace or control characters"
        );
        i += width;
    }
}

pub(crate) fn checked() -> Result<Vec<&'static Registration>, Error> {
    let mut namespaces: Vec<_> = NAMESPACES.iter().collect();
    namespaces.sort_unstable_by_key(|namespace| (namespace.name, namespace.source));
    for pair in namespaces.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(Error::message(format!(
                "configuration namespace {:?} is declared twice:\n  {}\n  {}",
                pair[0].name, pair[0].source, pair[1].source,
            )));
        }
    }
    Ok(namespaces)
}
