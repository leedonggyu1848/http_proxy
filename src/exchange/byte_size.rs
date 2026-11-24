use std::fmt;

const KIB: u64 = 1_024;
const MIB: u64 = 1_024 * KIB;
const GIB: u64 = 1_024 * MIB;

/// A byte count formatted with binary IEC units (`B`, `KiB`, `MiB`, `GiB`).
///
/// [`fmt::Display`] formats compact human-readable sizes: exact bytes below
/// `1 KiB`, and one decimal place truncated toward zero for `KiB`, `MiB`, and
/// `GiB`. Truncating with integer arithmetic avoids `f64` precision loss on
/// large `u64` values and keeps values just below a unit boundary (such as
/// `1 MiB - 1`) in the lower unit rather than rounding up to the next unit.
///
/// Use [`ByteSize::exact`] when exact configured limits must not lose
/// precision.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ByteSize(u64);

impl ByteSize {
    /// Wraps a byte count for display.
    #[must_use]
    pub const fn new(bytes: u64) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.0
    }

    /// Returns a formatter that preserves the exact byte count without rounding
    /// or truncation.
    #[must_use]
    pub const fn exact(self) -> ExactByteSize {
        ExactByteSize(self.0)
    }
}

impl From<u64> for ByteSize {
    fn from(bytes: u64) -> Self {
        Self(bytes)
    }
}

impl From<ByteSize> for u64 {
    fn from(size: ByteSize) -> Self {
        size.0
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        let (unit, suffix) = if bytes < KIB {
            return write!(f, "{bytes} B");
        } else if bytes < MIB {
            (KIB, "KiB")
        } else if bytes < GIB {
            (MIB, "MiB")
        } else {
            (GIB, "GiB")
        };
        let whole = bytes / unit;
        let tenths = ((bytes % unit) * 10) / unit;
        write!(f, "{whole}.{tenths} {suffix}")
    }
}

/// Exact byte-count formatter returned by [`ByteSize::exact`].
///
/// Formats non-zero exact multiples of `GiB`, `MiB`, or `KiB` with the largest
/// matching binary unit and no decimal part, and all other values (including
/// `0`) as exact bytes with `B`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExactByteSize(u64);

impl fmt::Display for ExactByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.0;
        if bytes == 0 {
            write!(f, "0 B")
        } else if bytes.is_multiple_of(GIB) {
            write!(f, "{} GiB", bytes / GIB)
        } else if bytes.is_multiple_of(MIB) {
            write!(f, "{} MiB", bytes / MIB)
        } else if bytes.is_multiple_of(KIB) {
            write!(f, "{} KiB", bytes / KIB)
        } else {
            write!(f, "{bytes} B")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_compact_sizes_across_unit_boundaries() {
        let cases = [
            (0, "0 B"),
            (512, "512 B"),
            (1_023, "1023 B"),
            (1_024, "1.0 KiB"),
            (1_500, "1.4 KiB"),
            (1_536, "1.5 KiB"),
            (1_950, "1.9 KiB"),
            (MIB - 1, "1023.9 KiB"),
            (MIB, "1.0 MiB"),
            (3 * MIB / 2, "1.5 MiB"),
            (4 * MIB, "4.0 MiB"),
            (GIB - 1, "1023.9 MiB"),
            (GIB, "1.0 GiB"),
            (2 * GIB, "2.0 GiB"),
            (u64::MAX, "17179869183.9 GiB"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(ByteSize::new(bytes).to_string(), expected, "bytes={bytes}");
        }
    }

    #[test]
    fn formats_exact_sizes_without_losing_precision() {
        let cases = [
            (0, "0 B"),
            (3, "3 B"),
            (1_023, "1023 B"),
            (1_024, "1 KiB"),
            (1_950, "1950 B"),
            (MIB - 1, "1048575 B"),
            (MIB, "1 MiB"),
            (MIB + 1, "1048577 B"),
            (512 * MIB, "512 MiB"),
            (GIB - 1, "1073741823 B"),
            (GIB, "1 GiB"),
            (GIB + 1, "1073741825 B"),
            (2 * GIB, "2 GiB"),
            (u64::MAX, "18446744073709551615 B"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(
                ByteSize::new(bytes).exact().to_string(),
                expected,
                "bytes={bytes}"
            );
        }
    }

    #[test]
    fn round_trips_with_u64() {
        let size = ByteSize::from(1_950);
        assert_eq!(size.bytes(), 1_950);
        assert_eq!(u64::from(size), 1_950);
        assert!(ByteSize::new(1_023) < ByteSize::new(1_024));
    }
}
