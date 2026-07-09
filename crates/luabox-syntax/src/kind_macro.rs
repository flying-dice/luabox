//! The `syntax_kinds!` macro shared by both grammars ([`crate::lua`] and
//! [`crate::shape`]): defines a kind enum plus a safe `u16` round-trip for
//! the rowan `Language` impl (no `unsafe`, no hand-maintained match).

macro_rules! syntax_kinds {
    (
        $(#[$enum_attr:meta])*
        $enum_name:ident {
            $($(#[$attr:meta])* $name:ident,)*
        }
    ) => {
        $(#[$enum_attr])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[repr(u16)]
        #[allow(non_camel_case_types)]
        pub enum $enum_name {
            $($(#[$attr])* $name,)*
        }

        impl $enum_name {
            fn from_u16(raw: u16) -> Option<Self> {
                $(if raw == Self::$name as u16 {
                    return Some(Self::$name);
                })*
                None
            }
        }
    };
}
