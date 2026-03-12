use std::ops::{BitOr, BitOrAssign};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Operations(u8);

impl Operations {
    pub const NONE: Self = Self(0);
    pub const PTH: Self = Self(0b0001);
    pub const NPTH: Self = Self(0b0010);
    pub const OUTLINE: Self = Self(0b0100);

    pub const FIRST: Self = Self::PTH;
    pub const FINAL: Self = Self(Self::NPTH.0 | Self::OUTLINE.0);
    pub const ALL: Self = Self(Self::FIRST.0 | Self::FINAL.0);

    pub fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn is_none(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Operations {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Operations {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
