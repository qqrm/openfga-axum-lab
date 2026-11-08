//! Typed HTTP identifiers reject raw OpenFGA object/userset syntax.
macro_rules! identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                if crate::domain::valid_id(&value) {
                    Ok(Self(value))
                } else {
                    Err(serde::de::Error::custom("invalid identifier"))
                }
            }
        }
    };
}
identifier!(OrganizationId);
identifier!(TaskId);
identifier!(UserId);
