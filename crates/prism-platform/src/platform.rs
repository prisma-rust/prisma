//! Compile-time and runtime target platform query helpers.

use std::fmt;

/// Supported runtime target platforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetPlatform {
    Android,
    Windows,
    MacOs,
    Linux,
    Other,
}

impl TargetPlatform {
    /// Returns the platform this binary was compiled for.
    pub const fn current() -> Self {
        if cfg!(target_os = "android") {
            Self::Android
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }

    /// True on Android mobile targets.
    pub const fn is_android(self) -> bool {
        matches!(self, Self::Android)
    }

    /// True on mobile targets (currently Android).
    pub const fn is_mobile(self) -> bool {
        matches!(self, Self::Android)
    }

    /// True on desktop targets (Windows, macOS, Linux).
    pub const fn is_desktop(self) -> bool {
        matches!(self, Self::Windows | Self::MacOs | Self::Linux)
    }

    /// Canonical lowercase identifier.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Windows => "windows",
            Self::MacOs => "macos",
            Self::Linux => "linux",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for TargetPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Returns the current target platform.
pub const fn current_platform() -> TargetPlatform {
    TargetPlatform::current()
}

/// Returns `true` when running on Android.
pub const fn is_android() -> bool {
    TargetPlatform::current().is_android()
}

/// Returns `true` when running on a desktop OS (Windows, macOS, Linux).
pub const fn is_desktop() -> bool {
    TargetPlatform::current().is_desktop()
}

/// Returns `true` when running on a mobile OS.
pub const fn is_mobile() -> bool {
    TargetPlatform::current().is_mobile()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_platform_is_consistent() {
        let p = current_platform();
        assert_eq!(is_android(), p.is_android());
        assert_eq!(is_desktop(), p.is_desktop());
        assert_eq!(is_mobile(), p.is_mobile());
        assert!(!p.as_str().is_empty());
    }
}
