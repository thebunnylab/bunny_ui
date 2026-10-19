//! Keep Objective-C BOOL's ABI and runtime encodings together at the `AppKit` boundary.
//!
//! Apple silicon uses C bool; Intel macOS uses signed char. This is distinct
//! from C99 bool and CoreFoundation Boolean, which must not use this adapter.
//! See Apple's "Addressing architectural differences in your macOS code".
use std::ffi::CStr;

#[cfg(target_arch = "aarch64")]
type Storage = bool;
#[cfg(not(target_arch = "aarch64"))]
type Storage = i8;

/// One native Objective-C BOOL, converted at the FFI boundary.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjcBool(Storage);

impl ObjcBool {
    /// Objective-C YES.
    pub const YES: Self = Self::new(true);
    /// Objective-C NO.
    pub const NO: Self = Self::new(false);

    /// Encodes a Rust predicate without relying on integer truthiness.
    pub const fn new(value: bool) -> Self {
        #[cfg(target_arch = "aarch64")]
        {
            Self(value)
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            Self(value as i8)
        }
    }

    /// Decodes the native result before application logic consumes it.
    pub const fn get(self) -> bool {
        #[cfg(target_arch = "aarch64")]
        {
            self.0
        }
        #[cfg(not(target_arch = "aarch64"))]
        {
            self.0 != 0
        }
    }
}

/// BOOL result with no explicit argument.
pub const GETTER: &CStr = if cfg!(target_arch = "aarch64") {
    c"B@:"
} else {
    c"c@:"
};
/// BOOL result with one object argument.
pub const OBJECT: &CStr = if cfg!(target_arch = "aarch64") {
    c"B@:@"
} else {
    c"c@:@"
};
/// BOOL result with one selector argument.
pub const SELECTOR: &CStr = if cfg!(target_arch = "aarch64") {
    c"B@::"
} else {
    c"c@::"
};
/// Void result with one BOOL argument.
pub const SETTER: &CStr = if cfg!(target_arch = "aarch64") {
    c"v@:B"
} else {
    c"v@:c"
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_bool_representation_and_encodings_agree() {
        assert_eq!(size_of::<ObjcBool>(), 1);
        assert_eq!(align_of::<ObjcBool>(), 1);
        assert!(ObjcBool::YES.get());
        assert!(!ObjcBool::NO.get());
        let expected = if cfg!(target_arch = "aarch64") {
            b'B'
        } else {
            b'c'
        };
        for encoding in [GETTER, OBJECT, SELECTOR] {
            assert_eq!(encoding.to_bytes()[0], expected);
        }
        assert_eq!(SETTER.to_bytes()[3], expected);
    }

    #[cfg(not(target_arch = "aarch64"))]
    #[test]
    fn intel_signed_char_accepts_nonzero_native_truth_values() {
        assert!(ObjcBool(-1).get());
        assert!(ObjcBool(2).get());
    }
}
